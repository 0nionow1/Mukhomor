Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('guard-lifecycle-'+[guid]::NewGuid().ToString('N'))
foreach($folder in @('runtime','private','bin')){[IO.Directory]::CreateDirectory((Join-Path $fixture $folder))|Out-Null}
[IO.File]::WriteAllText((Join-Path $fixture 'bin\wintun.dll'),'Mock file, never loaded')
& $module {
    function script:Test-SplitAdmin {$true}
    function script:Assert-NoOtherVpn {param($Root)}
    function script:Get-OwnedCore {param($Root) $null}
    function script:Get-SplitSettings {param($Root) @{routing=@{mode='full';allow_lan=$false}}}
    function script:Get-CorePath {param($Root) Join-Path $Root 'bin\MockCore.exe'}
    function script:Get-SplitTransport {param($Root) @{server='fixture.invalid';port=443;selected_address='192.0.2.1';interface_name='Fixture Ethernet'}}
    function script:Set-SplitConfig {param($Root,$Settings,$TunEnabled,[switch]$Reload,$Transport);$script:Events.Add('config:'+ $TunEnabled)}
    function script:Enable-SplitTrafficGuard {
        param($Root,$Settings,$Transport)
        $script:Events.Add('guard:bootstrap');$script:Armed=$true
        if($script:Failure -eq 'bootstrap'){throw 'Mock bootstrap verification failed'}
    }
    function script:Complete-SplitTrafficGuard {
        param($Root)
        $script:Events.Add('guard:tun')
        if($script:Failure -eq 'identity'){throw 'Mock owned adapter missing'}
    }
    function script:Disable-SplitTrafficGuard {param($Root);$script:Events.Add('guard:disarm');$script:Armed=$false}
    function script:Start-Process {
        param($FilePath,$ArgumentList,$WorkingDirectory,$WindowStyle,[switch]$PassThru,$RedirectStandardOutput,$RedirectStandardError)
        $script:Events.Add('core:start')
        $core=[pscustomobject]@{Id=123;StartTime=[datetime]::Now;HasExited=$false}
        $core|Add-Member ScriptMethod Kill {$this.HasExited=$true}
        $core|Add-Member ScriptMethod WaitForExit {param($Milliseconds) $true}
        $core
    }
    function script:Invoke-CoreApi {param($Root,$Route) @{}}
    function script:Get-NetAdapter {param($Name,$ErrorAction) [pscustomobject]@{Status='Up'}}
    function script:Set-SplitTunFirewall {param($Root);$script:Events.Add('firewall:tun')}
    function script:Assert-SplitAwgHealth {
        param($Root,$Phase)
        $script:Events.Add('health:'+ $Phase)
        if($script:Failure -eq $Phase){throw 'Mock carrier failed'}
    }
    function script:Set-SessionDns {param($Root);$script:Events.Add('dns:set')}
    function script:Assert-SplitTunDns {if($script:Failure -eq 'dns-probe'){throw 'Mock DNS failed'}}
    function script:Restore-SessionDns {param($Root);$script:Events.Add('dns:restore');if($script:StopFailure -eq 'dns'){throw 'Mock DNS recovery failed'}}
    function script:Stop-OwnedCore {
        param($Root)
        $script:Events.Add('core:stop')
        if($script:StopFailure -eq 'core'){throw 'Mock process shutdown failed'}
        [IO.File]::Delete((Join-Path $Root 'runtime\session.json'))
    }
    function script:Write-SplitLog {param($Root,$Message)}
}
try {
    & $module {
        param($fixture)
        $script:StopFailure=''
        foreach($failure in @('bootstrap','before TUN','identity','after TUN routing','dns-probe','')){
            $script:Failure=$failure;$script:Armed=$false;$script:Events=New-Object 'Collections.Generic.List[string]'
            $failed=$false
            try{Start-SplitSession $fixture $true}catch{$failed=$true}
            if($failed -ne [bool]$failure){throw 'Unexpected protected startup outcome'}
            if(!$script:Armed -or $script:Events.Contains('guard:disarm')){throw 'Startup or rollback released protection'}
            if($script:Events.Contains('core:start') -and $script:Events.IndexOf('guard:bootstrap') -gt $script:Events.IndexOf('core:start')){throw 'Core started before traffic protection'}
            if($failure -eq 'bootstrap' -and $script:Events.Contains('core:start')){throw 'Unverified protection allowed process startup'}
            if($script:Events.Contains('dns:set') -and $script:Events.IndexOf('guard:tun') -gt $script:Events.IndexOf('dns:set')){throw 'DNS changed before TUN identity verification'}
            if($failure -eq 'identity' -and $script:Events.Contains('dns:set')){throw 'Physical or missing adapter identity allowed DNS mutation'}
            if($failure -and (Test-Path -LiteralPath (Join-Path $fixture 'runtime\session.json'))){throw 'Failure retained a false running session'}
            if(!$failure -and (Read-JsonFile (Join-Path $fixture 'runtime\session.json')).phase -ne 'running'){throw 'Verified healthy startup did not finish'}
            'PASS: protected startup/rollback '+$(if($failure){$failure}else{'healthy'})
        }
        foreach($stopFailure in @('','dns','core')){
            $script:StopFailure=$stopFailure;$script:Armed=$true;$script:Events=New-Object 'Collections.Generic.List[string]'
            try{Stop-SplitSession $fixture}catch{if(!$stopFailure){throw}}
            if(!$script:Armed){throw 'Automatic cleanup released protection'}
            $script:Events.Clear();$failed=$false
            try{Stop-SplitSession $fixture -ReleaseGuard}catch{$failed=$true}
            if($failed -ne [bool]$stopFailure){throw 'Explicit disconnect lost recovery errors'}
            if($stopFailure -and (!$script:Armed -or $script:Events.Contains('guard:disarm'))){throw 'Failed DNS/core recovery released protection'}
            if(!$stopFailure -and ($script:Armed -or $script:Events.IndexOf('guard:disarm') -le $script:Events.IndexOf('core:stop'))){throw 'Explicit disconnect released protection before core shutdown'}
            'PASS: explicit disconnect and retained recovery '+$(if($stopFailure){$stopFailure}else{'healthy'})
        }
    } $fixture
    & $module {
        param($fixture)
        function script:Get-OwnedCore {param($Root) if($script:StatusHasCore){[pscustomobject]@{Id=123}}}
        function script:Get-SplitTrafficGuardStatus {param($Root) @{configured=$true;armed=$true;verified=$true;protected=$true}}
        foreach($case in @(@{core=$false;phase='running'},@{core=$true;phase='starting'},@{core=$true;phase='running'})){
            $script:StatusHasCore=$case.core
            Write-AtomicJson (Join-Path $fixture 'runtime\session.json') @{phase=$case.phase}
            $status=Get-SplitStatus $fixture
            $expected=$case.core -and $case.phase -eq 'running'
            if($status.traffic_guard.protected -ne $expected -or !$status.traffic_guard.verified -or !$status.traffic_guard.armed){throw 'Tunnel status confused persistent filters with a working owned core'}
            if(!$expected -and !$status.traffic_guard.recovery_pending){throw 'Stopped/starting core lost retained-guard recovery status'}
        }
        'PASS: stopped/starting core cannot claim a working protected tunnel; filter verification is preserved'
    } $fixture
    $guard=Get-Module -All TrafficGuard
    & $guard {
        param($fixture)
        function script:Invoke-SplitTrafficGuard {param($Root,$Action,$Plan) @{armed=$true;verified=$true;protected=$false;plan=@{core_executable='private-test-path';endpoints=@()}}}
        $status=Get-SplitTrafficGuardStatus $fixture
        if($status.ContainsKey('plan') -or !$status.configured -or !$status.recovery_pending){throw 'Recovery discovery disclosed the private policy or claimed clear'}
        function script:Get-SplitTrafficGuardStatus {param($Root) @{configured=$script:GuardActive}}
        $current=@{mode='full';allow_lan=$false}
        foreach($active in @($true,$false)){
            $script:GuardActive=$active
            foreach($candidate in @(@{mode='full';allow_lan=$false},@{mode='split';allow_lan=$false},@{mode='full';allow_lan=$true})){
                $changed=$candidate.mode -cne $current.mode -or $candidate.allow_lan -ne $current.allow_lan
                $failed=$false;try{Assert-SplitTrafficGuardRoutingChange $fixture $current $candidate}catch{$failed=$true}
                if($failed -ne ($active -and $changed)){throw 'Routing policy change ignored retained protection or blocked a cleared policy'}
            }
        }
        'PASS: private discovery status; retained guard requires explicit disconnect before routing/LAN changes'
    } $fixture
}finally{
    $resolved=[IO.Path]::GetFullPath($fixture)
    $parent=[IO.Path]::GetFullPath($PSScriptRoot)+'\'
    if(!$resolved.StartsWith($parent,[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe fixture cleanup path'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
'All network, WFP and process operations were mocked; host VPN untouched.'
