# Isolated regression tests for caches, persistent IPC and concurrent probes.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$fixture=Join-Path $PSScriptRoot ('perf-regression-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
Set-OwnedTestRoot $fixture $fixture
$savedState=$env:MUKHOMOR_STATE_ROOT;$savedAssets=$env:MUKHOMOR_ASSETS;$savedCore=$env:MUKHOMOR_CORE_PATH
$child=$null;$passed=0
function Assert($Condition,[string]$Message){if(!$Condition){throw $Message};$script:passed++}
try {
    $env:MUKHOMOR_STATE_ROOT=$fixture;$env:MUKHOMOR_ASSETS=$package
    Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
    Initialize-SplitRoot $fixture
    Add-Type -Path (Join-Path $package 'RuleNormalizer.cs')
    $examples=@('','  ','example.com','EXAMPLE.COM.','*.example.com','+.example.com','.example.com','localhost','127.1','127.0.0.1','::1','2001:db8::abcd/64','1.2.3.4/0','1.2.3.4/24','1.2.3.4/33','1.2.3.4/99999999999','1.2.3.4/','a..b','-a.example','a-.example','a_b.example','a/b.example',"bad`nvalue",'game.exe','GAME.EXE','game.exe:bad','C:\Games\game.exe','C:\Game Folder\GAME.EXE','*.exe',('a'*64+'.com'))
    $examples+=([string][char]0x043F+[char]0x0440+[char]0x0438+[char]0x043C+[char]0x0435+[char]0x0440+'.com')
    foreach($section in @('process_names','process_paths','domains','domain_suffixes','rule_set_exclusions','ip_cidrs')) {
        foreach($value in $examples) {
            $old=$null;$fast=$null;$oldFailed=$false;$fastFailed=$false
            try {$old=Convert-PresetRule $section $value}catch{$oldFailed=$true}
            try {$fast=[MukhomorRuleNormalizer]::Convert($section,$value)}catch{$fastFailed=$true}
            Assert ($oldFailed -eq $fastFailed -and ($oldFailed -or $old -ceq $fast)) ('Normalizer parity failed: '+$section)
        }
    }
    $settings=Get-SplitSettings $fixture
    $cidrs=@('::/1','2001:db8::/32','127.0.0.1/32','10.0.0.0/8','10.0.0.0/16','203.0.113.255/32')
    Assert (([MukhomorRuleNormalizer]::SortCidrs($cidrs) -join '|') -ceq (@($cidrs|Sort-Object {Get-CidrSortKey $_}) -join '|')) 'Fast CIDR sort changed IPv4/IPv6 ordering'
    $mutable=Get-SplitSettings $fixture;$mutable.direct.domains=@('caller-change.invalid')
    Assert ('caller-change.invalid' -notin (Get-SplitSettings $fixture).direct.domains) 'Caller mutation poisoned validated settings cache'
    $savedText=[IO.File]::ReadAllText((Join-Path $fixture 'settings.json'))
    $invalid=Convert-ToMap $settings;$invalid.ports.controller=-1
    Write-AtomicJson (Join-Path $fixture 'settings.json') $invalid
    $failed=$false;try {Get-SplitSettings $fixture|Out-Null}catch{$failed=$true}
    Assert $failed 'Changed invalid settings bypassed validation through the cache'
    Write-AtomicText (Join-Path $fixture 'settings.json') $savedText
    $first=Get-RuleSettingsProjection $settings
    Assert ($first.settings.direct.ip_cidrs -is [array]) 'Empty CIDR projection lost its array shape'
    $one=Convert-ToMap $settings;$one.presets=@();$one.direct.ip_cidrs=@('127.0.0.1/32')
    Assert ((Get-RuleSettingsProjection $one).settings.direct.ip_cidrs -is [array]) 'Single CIDR projection lost its array shape'
    $first=Get-RuleSettingsProjection $settings
    $cachePath=Join-Path $fixture 'runtime\rule-projection-cache.json'
    Assert (Test-Path -LiteralPath $cachePath) 'Projection cache was not persisted'
    $first.settings.direct.domains=@('mutated.invalid')
    $again=Get-RuleSettingsProjection $settings
    Assert ('mutated.invalid' -notin $again.settings.direct.domains) 'Caller mutation poisoned projection cache'
    Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
    & (Get-Module SplitVpn) {function script:New-RuleSettingsProjection {throw 'Cache miss in persistent cache test'}}
    $disk=Get-RuleSettingsProjection $settings
    Assert ($disk.settings.schema -eq 2) 'Persistent cache did not survive module reload'
    $changed=Convert-ToMap $settings;$changed.direct.domains=@('new.invalid')
    $miss=$false;try {Get-RuleSettingsProjection $changed|Out-Null}catch{$miss=$true}
    Assert $miss 'Changed rules reused stale cache'
    Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
    Write-AtomicText $cachePath '{corrupt'
    $rebuilt=Get-RuleSettingsProjection $changed
    Assert ('new.invalid' -in $rebuilt.settings.direct.domains) 'Corrupt cache did not rebuild'
    $changed.presets[0].enabled=!$changed.presets[0].enabled
    $expected=New-RuleSettingsProjection $changed
    $actual=Get-RuleSettingsProjection $changed
    Assert ((Convert-SplitCanonicalValue $expected|ConvertTo-Json -Depth 40 -Compress) -ceq (Convert-SplitCanonicalValue $actual|ConvertTo-Json -Depth 40 -Compress)) 'Preset toggle reused stale projection'

    [IO.Directory]::CreateDirectory((Join-Path $fixture 'bin'))|Out-Null
    $fakeCore=Join-Path $fixture 'bin\mihomo-mycelium.exe'
    Write-AtomicText $fakeCore 'Integrity fixture only; this file is never executed'
    Write-AtomicJson (Join-Path $fixture 'bin\mycelium-build.json') @{schema=1;protocol_version=1;core_sha256=(Get-FileHash -LiteralPath $fakeCore).Hash.ToLowerInvariant()}
    $env:MUKHOMOR_CORE_PATH=$fakeCore
    Assert ((Get-CorePath $fixture) -ceq $fakeCore -and (Get-CorePath $fixture) -ceq $fakeCore) 'Repeated pinned core lookup failed'
    $blocked=$false;try {[IO.File]::AppendAllText($fakeCore,'changed')}catch{$blocked=$true}
    Assert $blocked 'Pinned executable was writable during the operation'
    Clear-SplitOperationCache
    [IO.File]::AppendAllText($fakeCore,'changed')
    $rejected=$false;try {Get-CorePath $fixture|Out-Null}catch{$rejected=$true}
    Assert $rejected 'Changed executable bypassed verification in the next operation'
    $env:MUKHOMOR_CORE_PATH=$savedCore

    # Exercise the actual asynchronous HTTP probe against loopback only.
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
public sealed class MukhomorHealthFixture : IDisposable {
    readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 0);
    readonly ManualResetEventSlim stopped = new ManualResetEventSlim(false);
    readonly Thread worker;
    public int Requests;
    public volatile bool Fail;
    public int Port { get { return ((IPEndPoint)listener.LocalEndpoint).Port; } }
    public MukhomorHealthFixture() {
        listener.Start(); worker = new Thread(Accept); worker.IsBackground = true; worker.Start();
    }
    void Accept() {
        while (!stopped.IsSet) {
            TcpClient client;
            try { client = listener.AcceptTcpClient(); } catch { break; }
            ThreadPool.QueueUserWorkItem(_ => Reply(client));
        }
    }
    void Reply(TcpClient client) {
        using (client) try {
            client.ReceiveTimeout = 3000;
            Stream stream = client.GetStream();
            var reader = new StreamReader(stream, Encoding.ASCII);
            string first = reader.ReadLine(), line;
            while (!String.IsNullOrEmpty(line = reader.ReadLine())) {}
            Interlocked.Increment(ref Requests);
            bool fail = Fail;
            if (stopped.Wait(!fail && first.Contains("gstatic") ? 2200 : 40)) return;
            string body = fail ? "{}" : "{\"delay\":1}";
            byte[] bytes = Encoding.ASCII.GetBytes("HTTP/1.1 " + (fail ? "500 Error" : "200 OK") + "\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: " + body.Length + "\r\n\r\n" + body);
            stream.Write(bytes, 0, bytes.Length); stream.Flush();
        } catch {}
    }
    public void Dispose() { stopped.Set(); listener.Stop(); worker.Join(1000); }
}
'@
    Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
    $server=New-Object MukhomorHealthFixture
    try {
        $probeSettings=Convert-ToMap $settings;$probeSettings.ports.controller=$server.Port
        Write-AtomicJson (Join-Path $fixture 'settings.json') $probeSettings
        $clock=[Diagnostics.Stopwatch]::StartNew()
        Assert-SplitAwgHealth $fixture 'parallel fixture'
        Assert ($server.Requests -eq 2 -and $clock.ElapsedMilliseconds -lt 1800) 'Healthy second endpoint waited for the stalled first endpoint'
        $server.Fail=$true
        $failed=$false;try {Assert-SplitAwgHealth $fixture 'failed fixture'}catch{$failed=$true}
        Assert $failed 'Two failed health probes were reported as connected'
        Write-AtomicText (Join-Path $fixture 'runtime\stop.request') 'fixture cancellation'
        $failed=$false;try {Assert-SplitAwgHealth $fixture 'cancelled fixture'}catch{$failed=$_.Exception.Message -eq 'Startup cancelled'}
        Assert $failed 'Cancelled health check was not interrupted'
    } finally {
        [IO.File]::Delete((Join-Path $fixture 'runtime\stop.request'))
        Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
        $server.Dispose()
    }

    # A failed command must not poison the interpreter or the next frame.
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $info.Arguments='-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'+(Join-Path $package 'NativeBridge.ps1')+'" -Root "'+$fixture+'" -Persistent'
    $info.UseShellExecute=$false;$info.CreateNoWindow=$true
    $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    $info.StandardOutputEncoding=New-Object Text.UTF8Encoding($false)
    $child=[Diagnostics.Process]::Start($info)
    $stderr=$child.StandardError.ReadToEndAsync()
    foreach($action in @('Init','UnsupportedFixtureAction','Snapshot','ValidateSettings','Snapshot')) {
        $request=@{action=$action};if($action -eq 'ValidateSettings'){$request.settings=$settings}
        $bytes=[Text.Encoding]::UTF8.GetBytes(($request|ConvertTo-Json -Depth 40 -Compress)+"`n")
        $child.StandardInput.BaseStream.Write($bytes,0,$bytes.Length);$child.StandardInput.BaseStream.Flush()
        $reply=$child.StandardOutput.ReadLineAsync()
        Assert ($reply.Wait(30000)) 'Persistent bridge deadline exceeded'
        Assert (!$child.HasExited -and $null -ne $reply.Result) 'Persistent interpreter exited between requests'
        $value=$reply.Result|ConvertFrom-Json
        Assert ([bool]$value.ok -eq ($action -ne 'UnsupportedFixtureAction')) ('Persistent bridge reply did not match '+$action)
    }
    $child.StandardInput.Close()
    Assert ($child.WaitForExit(3000)) 'Persistent bridge did not exit on EOF'
    Assert ($child.ExitCode -eq 0) 'Persistent bridge failed at EOF'
    Write-Host ('PASS: '+$passed+' performance cache, normalizer and persistent bridge assertions')
} finally {
    Clear-SplitOperationCache
    if($child){if(!$child.HasExited){$child.Kill();$child.WaitForExit()};$child.Dispose()}
    $env:MUKHOMOR_STATE_ROOT=$savedState;$env:MUKHOMOR_ASSETS=$savedAssets;$env:MUKHOMOR_CORE_PATH=$savedCore
}
