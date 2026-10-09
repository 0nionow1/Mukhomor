Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('startup-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $fixture 'runtime')) | Out-Null
[IO.Directory]::CreateDirectory((Join-Path $fixture 'bin')) | Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'bin\wintun.dll'),'Mock; not loaded')
# All core/process/adapter/DNS operations are mocked. Only fixture JSON is real.
& $module {
    function script:Test-SplitAdmin { $true }
    function script:Assert-NoOtherVpn { param($Root) }
    function script:Get-OwnedCore { param($Root) $null }
    function script:Get-SplitSettings { param($Root) @{} }
    function script:Get-SplitTransport { param($Root) @{server='192.0.2.1';port=443;addresses=@('192.0.2.1');selected_address='192.0.2.1';interface_name='Fixture Ethernet'} }
    function script:Set-SplitConfig {
        param($Root,$Settings,$TunEnabled,[switch]$Reload,$Transport)
        if (!$Transport -or $Transport.interface_name -ne 'Fixture Ethernet') { throw 'Transport context was not passed through startup' }
        $script:Events.Add('config:'+ $TunEnabled)
    }
    function script:Get-CorePath { param($Root) Join-Path $Root 'bin\MockCore.exe' }
    function script:Start-Process {
        param($FilePath,$ArgumentList,$WorkingDirectory,$WindowStyle,[switch]$PassThru,$RedirectStandardOutput,$RedirectStandardError)
        $core=[pscustomobject]@{Id=123;StartTime=[datetime]::Now;HasExited=$false}
        $core | Add-Member ScriptMethod Kill { $this.HasExited=$true }
        $core | Add-Member ScriptMethod WaitForExit { param($milliseconds) $true }
        $core
    }
    function script:Invoke-CoreApi { param($Root,$Route) @{} }
    function script:Assert-SplitAwgHealth {
        param($Root,$Phase)
        $script:Events.Add('health:'+ $Phase)
        if ($Phase -eq $script:FailPhase) { throw 'Simulated AWG failure' }
    }
    function script:Get-NetAdapter { param($Name,$ErrorAction) [pscustomobject]@{Status='Up'} }
    function script:Set-SplitTunFirewall {
        param($Root)
        $script:Events.Add('firewall:tun')
        if ($script:FailPhase -eq 'firewall') {throw 'Simulated firewall failure'}
    }
    function script:Set-SessionDns {
        param($Root)
        $script:Events.Add('dns:set')
        Write-AtomicJson (Join-Path $Root 'runtime\dns-backup.json') @{mock=$true}
        if ($script:FailPhase -eq 'dns') { throw 'Simulated DNS failure' }
    }
    function script:Assert-SplitTunDns {
        $script:Events.Add('dns:probe')
        if($script:FailPhase -eq 'tun DNS'){throw 'Simulated TUN DNS failure'}
    }
    function script:Restore-SessionDns {
        param($Root)
        $script:Events.Add('dns:restore')
        [IO.File]::Delete((Join-Path $Root 'runtime\dns-backup.json'))
    }
    function script:Stop-OwnedCore {
        param($Root)
        $script:Events.Add('core:stop')
        [IO.File]::Delete((Join-Path $Root 'runtime\session.json'))
    }
    function script:Write-SplitLog { param($Root,$Message) }
}
& $module {
    param($fixture)
    foreach ($phase in @('before TUN','firewall','after TUN routing','dns','tun DNS','')) {
        $script:Events=New-Object 'Collections.Generic.List[string]'
        $script:FailPhase=$phase; $failed=$false
        try {Start-SplitSession $fixture $true} catch {$failed=$true}
        if ($phase) {
            if (!$failed -or $script:Events -notcontains 'core:stop' -or $script:Events -notcontains 'dns:restore') {throw "Missing failure rollback: $phase"}
            if (Test-Path -LiteralPath (Join-Path $fixture 'runtime\session.json')) {throw 'Failed startup left a running session'}
            if (Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json')) {throw 'DNS rollback was skipped'}
            if ($phase -notin @('dns','tun DNS') -and $script:Events -contains 'dns:set') {throw 'DNS changed before the health checks passed'}
            "PASS: $phase failure stops core and restores DNS; no false running state"
        } else {
            if ($failed) {throw 'Healthy startup failed'}
            $session=Read-JsonFile (Join-Path $fixture 'runtime\session.json')
            if ($session.phase -ne 'running' -or $session.transport.interface_name -ne 'Fixture Ethernet') {throw 'Healthy startup lost state/transport'}
            $post=$script:Events.IndexOf('health:after TUN routing'); $dns=$script:Events.IndexOf('dns:set')
            if ($post -lt 0 -or $post -ge $dns) {throw 'Post-TUN check must precede DNS mutation'}
            if($script:Events.IndexOf('dns:probe') -le $dns){throw 'Real DNS probe must follow DNS mutation before reporting running'}
            'PASS: healthy startup binds transport and checks AWG after TUN, before changing DNS'
            Restore-SessionDns $fixture; Stop-OwnedCore $fixture
        }
    }
} $fixture
'All core and network changes in startup recovery tests were mocked.'
