Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('lifecycle-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $fixture 'runtime')) | Out-Null
$passed=0
function Assert($Condition,[string]$Message) { if (!$Condition) { throw "FAIL: $Message" }; $script:passed++ }
try {
    $shell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $result=Invoke-SplitBoundedProcess $shell @('-NoProfile','-NonInteractive','-Command','[Console]::Write("a b"); exit 7') 5000 'Fixture command'
    Assert ($result.exit_code -eq 7 -and $result.stdout -eq 'a b') 'bounded child preserves quoted arguments, output and exit code'
    $clock=[Diagnostics.Stopwatch]::StartNew(); $failed=$false
    try { Invoke-SplitBoundedProcess $shell @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') 300 'Fixture command' | Out-Null } catch { $failed=$_.Exception.Message -match 'timed out' }
    Assert ($failed -and $clock.ElapsedMilliseconds -lt 2500) 'stalled native child is terminated within its deadline'
    $script:DnsRequests=0
    $slowDns={
        param($provider,$hostName,$type,$timeoutSeconds)
        $script:DnsRequests++
        Start-Sleep -Milliseconds 150
        return @{Status=0;Answer=@(@{name=$hostName;type=5;TTL=60;data='later.example.test'})}
    }
    $clock=[Diagnostics.Stopwatch]::StartNew(); $failed=$false
    try { Resolve-DnsApiHost 'fixture.example.test' 64 $slowDns 100 | Out-Null } catch { $failed=$_.Exception.Message -match 'deadline' }
    Assert ($failed -and $script:DnsRequests -eq 1 -and $clock.ElapsedMilliseconds -lt 1000) 'CNAME and provider fallback share one DNS deadline'
    $cancelDns={
        param($provider,$hostName,$type,$timeoutSeconds)
        [IO.File]::WriteAllText((Join-Path $fixture 'runtime\stop.request'),'disconnect')
        return @{Status=0;Answer=@(@{name=$hostName;type=1;TTL=60;data='8.8.8.8'})}
    }
    [IO.File]::WriteAllText((Join-Path $fixture 'runtime\session.json'),'{}')
    $failed=$false
    try { Resolve-DnsApiHost 'fixture.example.test' 64 $cancelDns 8000 $fixture | Out-Null } catch { $failed=$_.Exception.Message -eq 'DNS update cancelled' }
    Assert $failed 'DNS resolution rechecks cancellation before accepting or following an answer'
    [IO.File]::Delete((Join-Path $fixture 'runtime\stop.request')); [IO.File]::Delete((Join-Path $fixture 'runtime\session.json'))
    $passed+= & $module {
        param($fixture)
        $script:Passed=0
        $script:AtomicWriter=(Get-Command Write-AtomicJson).ScriptBlock
        function Test-Assert($Condition,[string]$Message) { if (!$Condition) { throw "FAIL: $Message" }; $script:Passed++ }
        # Validate the netsh argv/deadline contract, never launch netsh.
        $script:NativeCalls=New-Object 'Collections.Generic.List[object]'
        function script:Invoke-SplitBoundedProcess {
            param($FilePath,$Arguments,$TimeoutMilliseconds,$Purpose)
            $script:NativeCalls.Add(@{path=$FilePath;args=@($Arguments);timeout=$TimeoutMilliseconds})
            return @{exit_code=0;stdout='';stderr=''}
        }
        Invoke-NetshDns 42 'ipv4' 'static' @('8.8.8.8','8.8.4.4')
        Test-Assert ($script:NativeCalls.Count -eq 2 -and $script:NativeCalls[0].args.Count -eq 8 -and $script:NativeCalls[0].args[0] -eq 'interface') 'DNS commands preserve the complete argument list'
        Test-Assert ($script:NativeCalls[0].path -eq (Join-Path $env:SystemRoot 'System32\netsh.exe') -and $script:NativeCalls[0].timeout -le 5000) 'DNS command uses the system executable with a bounded timeout'
        # All remaining process, adapter, DNS and controller calls are mocks.
        $script:Adapter=[pscustomobject]@{InterfaceGuid=[guid]'11111111-1111-1111-1111-111111111111';ifIndex=42;Status='Up';Name='Fixture'}
        function script:Get-NetAdapter { param($Name,[switch]$IncludeHidden,$ErrorAction) $script:Adapter }
        function script:Invoke-NetshDns { param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds) if ($Family -eq 'ipv6') { throw 'Fixture DNS command timeout' } }
        $rows=@(@{guid=$script:Adapter.InterfaceGuid.ToString('B');name='Fixture';family='ipv4';source='dhcp';servers=@()},@{guid=$script:Adapter.InterfaceGuid.ToString('B');name='Fixture';family='ipv6';source='dhcp';servers=@()})
        $backup=Join-Path $fixture 'runtime\dns-backup.json'
        Write-AtomicJson $backup @{schema=1;adapters=$rows}
        $failed=$false; try { Restore-SessionDns $fixture } catch { $failed=$true }
        $left=Read-JsonFile $backup
        Test-Assert ($failed -and $left.adapters.Count -eq 1 -and $left.adapters[0].family -eq 'ipv6') 'failed DNS command retains only the unrestored row for retry'
        function script:Invoke-NetshDns { param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds) }
        Restore-SessionDns $fixture
        Test-Assert (!(Test-Path -LiteralPath $backup)) 'retry clears the snapshot after successful restoration'
        $script:DnsAttempts=0
        function script:Invoke-NetshDns {
            param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds)
            $script:DnsAttempts++
            Start-Sleep -Milliseconds ([Math]::Min(100,$TimeoutMilliseconds))
            if ($TimeoutMilliseconds -le 100) { throw 'Fixture DNS command deadline' }
        }
        Write-AtomicJson $backup @{schema=1;adapters=@($rows[0],$rows[0],$rows[0],$rows[0])}
        $timer=[Diagnostics.Stopwatch]::StartNew(); $failed=$false
        try { Restore-SessionDns $fixture 150 } catch { $failed=$true }
        $left=Read-JsonFile $backup
        Test-Assert ($failed -and $script:DnsAttempts -eq 2 -and $left.adapters.Count -eq 3 -and $timer.ElapsedMilliseconds -lt 500) 'DNS recovery shares one total budget and retains all unattempted rows'
        function script:Invoke-NetshDns { param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds) }
        Restore-SessionDns $fixture
        function New-FixtureCore {
            $core=[pscustomobject]@{Id=123;StartTime=[datetime]::Now;HasExited=$false;Killed=$false}
            $core | Add-Member ScriptMethod Kill { $this.Killed=$true; $this.HasExited=$true }
            $core | Add-Member ScriptMethod WaitForExit { param($milliseconds) $true }
            return $core
        }
        $script:Core=New-FixtureCore
        function script:Get-OwnedCore { param($Root) $script:Core }
        function script:Invoke-CoreApi { param($Root,$Route,$Method,$Body,$TimeoutSeconds) if ($TimeoutSeconds -ne 1) { throw 'Graceful stop must use a one-second timeout' }; throw 'Fixture controller timeout' }
        Stop-OwnedCore $fixture
        Test-Assert $script:Core.Killed 'controller failure still terminates the verified owned process'
        $script:Stopped=$false
        function script:Restore-SessionDns { param($Root) throw 'Fixture DNS failure' }
        function script:Stop-OwnedCore { param($Root) $script:Stopped=$true }
        $failed=$false; try { Stop-SplitSession $fixture } catch { $failed=$_.Exception.Message -eq 'Fixture DNS failure' }
        Test-Assert ($failed -and $script:Stopped) 'DNS recovery error cannot bypass core shutdown'
        function script:Restore-SessionDns { param($Root) }
        function script:Assert-NoOtherVpn { param($Root) }
        function script:Get-OwnedCore { param($Root) $null }
        function script:Get-SplitSettings { param($Root) @{} }
        function script:Get-SplitTransport { param($Root) @{interface_name='Fixture Ethernet'} }
        function script:Set-SplitConfig { param($Root,$Settings,$TunEnabled,[switch]$Reload,$Transport) }
        function script:Get-CorePath { param($Root) Join-Path $Root 'fixture.exe' }
        function script:Start-Process { param($FilePath,$ArgumentList,$WorkingDirectory,$WindowStyle,[switch]$PassThru,$RedirectStandardOutput,$RedirectStandardError) $script:Core }
        function script:Write-AtomicJson { param($Path,$Value) throw 'Fixture session registration failure' }
        $script:Core=New-FixtureCore; $script:Stopped=$false
        $failed=$false; try { Start-SplitSession $fixture $false } catch { $failed=$_.Exception.Message -eq 'Fixture session registration failure' }
        Test-Assert ($failed -and $script:Core.Killed -and $script:Stopped) 'failed session registration rolls back the newly launched process without a session record'
        function script:Write-AtomicJson { param($Path,$Value) & $script:AtomicWriter $Path $Value }
        $script:Core=New-FixtureCore; $script:Stopped=$false; $script:ControllerCalls=0; $script:ShortTimeout=$true
        function script:Invoke-CoreApi {
            param($Root,$Route,$Method,$Body,$TimeoutSeconds)
            $script:ControllerCalls++
            if ($TimeoutSeconds -ne 1) { $script:ShortTimeout=$false }
            Start-Sleep -Milliseconds 300
            throw 'Fixture controller never becomes ready'
        }
        $timer=[Diagnostics.Stopwatch]::StartNew(); $failed=$false
        try { Start-SplitSession $fixture $false } catch { $failed=$_.Exception.Message -eq 'Mihomo controller did not start' }
        Test-Assert ($failed -and $timer.ElapsedMilliseconds -ge 12000 -and $timer.ElapsedMilliseconds -lt 14000 -and $script:Core.Killed) 'readiness retries share one cumulative 12-second deadline and roll back'
        Test-Assert ($script:ShortTimeout -and $script:ControllerCalls -gt 1 -and $script:ControllerCalls -lt 40) 'readiness requests use one-second timeouts rather than multiplying 40 long timeouts'
        return $script:Passed
    } $fixture
    Write-Host ('PASS: '+$passed+' lifecycle assertions; no TUN or system DNS touched')
} finally {
    $resolved=[IO.Path]::GetFullPath($fixture)
    $allowed=[IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')+'\'
    if (!$resolved.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture cleanup left the test directory' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
