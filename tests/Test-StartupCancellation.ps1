Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('cancellation-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $fixture 'runtime'))|Out-Null
[IO.Directory]::CreateDirectory((Join-Path $fixture 'bin'))|Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'bin\wintun.dll'),'Fixture, never loaded')
# Every process, controller, adapter and DNS operation is mocked. The real
# startup transaction and cancellation boundaries run against fixture JSON.
& $module {
    function script:Test-SplitAdmin {$true}
    function script:Assert-NoOtherVpn {param($Root)}
    function script:Get-OwnedCore {param($Root) $null}
    function script:Get-SplitSettings {param($Root) @{}}
    function script:Get-CorePath {param($Root) Join-Path $Root 'bin\MockCore.exe'}
    function script:Get-SplitTransport {
        param($Root)
        if($script:CancelAt -eq 'transport'){[IO.File]::WriteAllText((Join-Path $Root 'runtime\stop.request'),'cancel')}
        @{interface_name='Fixture Ethernet'}
    }
    function script:Set-SplitConfig {param($Root,$Settings,$TunEnabled,[switch]$Reload,$Transport);$script:Events.Add('config')}
    function script:Start-Process {
        param($FilePath,$ArgumentList,$WorkingDirectory,$WindowStyle,[switch]$PassThru,$RedirectStandardOutput,$RedirectStandardError)
        $script:Events.Add('core:start')
        $core=[pscustomobject]@{Id=123;StartTime=[datetime]::Now;HasExited=$false}
        $core | Add-Member NoteProperty Events $script:Events
        $core | Add-Member NoteProperty KillFails $script:KillFails
        $core | Add-Member ScriptMethod Kill {
            $this.Events.Add('core:kill')
            if ($this.KillFails) { throw 'Fixture direct process cleanup detail must not replace the original error' }
            $this.HasExited=$true
        }
        $core | Add-Member ScriptMethod WaitForExit { param($milliseconds) $true }
        $script:LastCore=$core
        $core
    }
    function script:Invoke-CoreApi {
        param($Root,$Route)
        if($script:CancelAt -eq 'controller'){
            [IO.File]::WriteAllText((Join-Path $Root 'runtime\stop.request'),'cancel')
            throw 'Controller not ready yet'
        }
        @{}
    }
    function script:Assert-SplitAwgHealth {
        param($Root,$Phase)
        Assert-SplitNotCancelled $Root
        if($script:CancelAt -eq 'post-route' -and $Phase -eq 'after TUN routing'){
            [IO.File]::WriteAllText((Join-Path $Root 'runtime\stop.request'),'cancel')
        }
    }
    function script:Get-NetAdapter {param($Name,$ErrorAction);[pscustomobject]@{Status='Up'}}
    function script:Set-SplitTunFirewall {param($Root);$script:Events.Add('firewall')}
    function script:Set-SessionDns {
        param($Root)
        $script:Events.Add('dns:set');Write-AtomicJson (Join-Path $Root 'runtime\dns-backup.json') @{fixture=$true}
        if($script:CancelAt -eq 'dns'){[IO.File]::WriteAllText((Join-Path $Root 'runtime\stop.request'),'cancel')}
    }
    function script:Assert-SplitTunDns {
        if($script:CancelAt -eq 'commit'){[IO.File]::WriteAllText((Join-Path $script:FixtureRoot 'runtime\stop.request'),'cancel')}
        if($script:StartupError){throw $script:StartupError}
    }
    function script:Restore-SessionDns {
        param($Root)
        $script:Events.Add('dns:restore')
        if($script:RestoreFails){throw 'Fixture unavailable adapter'}
        [IO.File]::Delete((Join-Path $Root 'runtime\dns-backup.json'))
    }
    function script:Stop-OwnedCore {
        param($Root)
        $script:Events.Add('core:stop')
        if($script:StopFails){throw 'Fixture owned process cleanup detail must not replace the original error'}
        [IO.File]::Delete((Join-Path $Root 'runtime\session.json'))
    }
    function script:Write-SplitLog {param($Root,$Message)}
}
try {
    & $module {
        param($fixture)
        $script:FixtureRoot=$fixture;$script:RestoreFails=$false;$script:StopFails=$false;$script:KillFails=$false;$script:StartupError=$null
        foreach($phase in @('pre-start','transport','controller','post-route','dns','commit')){
            $script:CancelAt=$phase;$script:Events=New-Object 'Collections.Generic.List[string]'
            $marker=Join-Path $fixture 'runtime\stop.request';[IO.File]::Delete($marker)
            if($phase -eq 'pre-start'){[IO.File]::WriteAllText($marker,'cancel')}
            $failed=$false
            try {Start-SplitSession $fixture $true} catch {$failed=$true}
            if(!$failed){throw ('Cancelled startup succeeded at '+$phase)}
            if($phase -in @('pre-start','transport')){
                if($script:Events -contains 'core:start' -or $script:Events -contains 'dns:set'){throw 'Pre-start cancellation changed the connection'}
            } else {
                if($script:Events -notcontains 'core:stop' -or $script:Events -notcontains 'dns:restore'){throw 'Cancellation skipped cleanup'}
                if(Test-Path -LiteralPath (Join-Path $fixture 'runtime\session.json')){throw 'Cancellation committed a running session'}
                if(Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json')){throw 'Cancellation retained a restorable DNS snapshot'}
                if($phase -notin @('dns','commit') -and $script:Events -contains 'dns:set'){throw 'Cancelled routing check still changed DNS'}
            }
            Write-Host ('PASS: cancellation at '+$phase+' leaves no running session')
        }
        $script:RestoreFails=$true;$script:Events=New-Object 'Collections.Generic.List[string]'
        $failed=$false;try {Stop-SplitSession $fixture} catch {$failed=$true}
        if(!$failed -or $script:Events -notcontains 'core:stop'){throw 'DNS restore failure prevented core shutdown'}
        Write-Host 'PASS: unavailable DNS adapter never prevents the owned core from stopping'
        foreach($cleanupFails in @($false,$true)){
            [IO.File]::Delete((Join-Path $fixture 'runtime\stop.request'))
            [IO.File]::Delete((Join-Path $fixture 'runtime\dns-backup.json'))
            [IO.File]::Delete((Join-Path $fixture 'runtime\session.json'))
            $script:CancelAt='';$script:RestoreFails=$true;$script:StopFails=$cleanupFails;$script:KillFails=$cleanupFails
            $script:Events=New-Object 'Collections.Generic.List[string]'
            $original=New-Object Management.Automation.ErrorRecord (
                (New-Object InvalidOperationException 'Original startup DNS probe failure'),
                'FixtureStartupProbe',
                [Management.Automation.ErrorCategory]::InvalidOperation,
                'Fixture TUN probe'
            )
            $script:StartupError=$original
            $failure=$null
            try {Start-SplitSession $fixture $true} catch {$failure=$_}
            if(!$failure -or $failure.Exception.Message -ne $original.Exception.Message -or $failure.FullyQualifiedErrorId -notmatch '^FixtureStartupProbe'){throw 'Startup rollback replaced the original ErrorRecord'}
            if(![object]::ReferenceEquals($failure.Exception,$original.Exception)){throw 'Startup rollback replaced the original exception instance'}
            foreach($required in @('dns:set','dns:restore','core:stop','core:kill')){if($script:Events -notcontains $required){throw ('Startup rollback skipped '+$required)}}
            if($script:Events.IndexOf('dns:restore') -gt $script:Events.IndexOf('core:stop') -or $script:Events.IndexOf('core:stop') -gt $script:Events.IndexOf('core:kill')){throw 'Startup rollback cleanup stages ran out of order'}
            if(!(Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json'))){throw 'Failed DNS rollback lost its recovery snapshot'}
            $annotations=@($failure.Exception.Data['MukhomorRollbackFailures'])
            $expected=if($cleanupFails){@('dns','core','core-direct')}else{@('dns')}
            if((@($annotations | Sort-Object) -join ',') -cne (@($expected | Sort-Object) -join ',')){throw 'Startup rollback annotations must contain only the failed safe stage labels'}
            if(!$cleanupFails -and !$script:LastCore.HasExited){throw 'DNS rollback failure prevented direct owned-process termination'}
            Write-Host ('PASS: original startup ErrorRecord survives DNS rollback'+$(if($cleanupFails){' and both core cleanup failures'}else{'; owned core still terminates'}))
        }
        $script:RestoreFails=$false;$script:StopFails=$false;$script:KillFails=$false;$script:StartupError=$null
    } $fixture
    Write-Host 'PASS: 9 startup cancellation / disconnect boundaries; all network operations mocked'
} finally {
    $resolved=[IO.Path]::GetFullPath($fixture);$expected=([IO.Path]::GetFullPath($PSScriptRoot)+'\cancellation-')
    if($resolved.StartsWith($expected,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^cancellation-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
