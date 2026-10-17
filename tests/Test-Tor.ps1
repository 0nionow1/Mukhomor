Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$fixture=Join-Path $PSScriptRoot ('tor-routing-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
Set-OwnedTestRoot $fixture $fixture
Initialize-SplitRoot $fixture
$settings=Get-SplitSettings $fixture
$settings.direct.domains=@('example.com');$settings.routing.mode='split';$settings.routing.allow_lan=$true
$before=$settings|ConvertTo-Json -Depth 40 -Compress
$transport=@{network='tor';server='127.0.0.1';port=12345;addresses=@('127.0.0.1');selected_address='127.0.0.1';interface_name='Fixture';tor_proxy=@{name='AWG';type='socks5';server='127.0.0.1';port=12345;username=('a'*64);password=('b'*64);udp=$false}}
$config=New-CoreConfig $fixture $settings $false $transport
if(($config.rules -join '|') -cne 'NETWORK,udp,REJECT|MATCH,AWG'){throw 'Tor bypass or UDP leak in generated rules'}
if($config.proxies[0].udp -or $config['mixed-port'] -ne 0 -or $config['rule-providers'].Count){throw 'Tor exposes an unintended local proxy or split provider'}
if($config['log-level'] -ne 'silent' -or $config.profile['store-fake-ip']){throw 'Tor persists destination metadata'}
foreach($key in @('nameserver','default-nameserver','proxy-server-nameserver','direct-nameserver')){
    foreach($resolver in $config.dns[$key]){if($resolver -cnotmatch '^https://[0-9.]+/dns-query#AWG$'){throw ('Tor DNS bypass: '+$key)}}
}
if(($settings|ConvertTo-Json -Depth 40 -Compress) -cne $before){throw 'Tor modified saved routing preferences'}
$i2pTransport=$transport.Clone();$i2pTransport.network='i2p'
$i2pConfig=New-CoreConfig $fixture $settings $false $i2pTransport
if(($i2pConfig.rules -join '|') -cne 'NETWORK,udp,REJECT|DOMAIN-SUFFIX,i2p,AWG|MATCH,REJECT'){throw 'I2P permits traffic outside the internal network'}
if($i2pConfig.sniffer.enable -or $i2pConfig['mixed-port'] -ne 0){throw 'I2P exposes an unintended sniffer or local proxy'}
foreach($key in @('nameserver','proxy-server-nameserver','direct-nameserver')){if(($i2pConfig.dns[$key] -join '|') -cne 'rcode://success'){throw 'I2P uses external DNS'}}
if(($i2pConfig.dns['default-nameserver'] -join '|') -cne '127.0.0.1:1'){throw 'I2P DNS bootstrap escapes loopback'}
$old=$env:MUKHOMOR_CORE_PATH
try{
    $env:MUKHOMOR_CORE_PATH=Get-CorePath $package
    $path=Join-Path $fixture 'private\tor-validation.json'
    Write-AtomicJson $path $config
    Test-CoreConfig $fixture $path
    $i2pPath=Join-Path $fixture 'private\i2p-validation.json'
    Write-AtomicJson $i2pPath $i2pConfig
    Test-CoreConfig $fixture $i2pPath
    $saved=[IO.File]::ReadAllText((Join-Path $fixture 'settings.json'))
    Set-SplitConfig $fixture $settings $false -Transport $transport -SessionOnly
    if([IO.File]::ReadAllText((Join-Path $fixture 'settings.json')) -cne $saved){throw 'Tor session overwrote saved settings'}
}finally{$env:MUKHOMOR_CORE_PATH=$old}
'PASS: Tor config accepted by bundled Mihomo; UDP, direct exceptions and physical DNS excluded. No system networking changed.'

Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
# Real loopback HTTP failures exercise bounded I2P warmup retries and cancellation.
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
public sealed class I2PHealthFixture : IDisposable {
    readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 0);
    readonly Thread worker;
    public volatile int Count;
    public volatile bool FailForever;
    public string CancelPath;
    public int Port { get { return ((IPEndPoint)listener.LocalEndpoint).Port; } }
    public I2PHealthFixture() {
        listener.Start(); worker = new Thread(Run); worker.IsBackground = true; worker.Start();
    }
    void Run() {
        try { while (true) using (var client = listener.AcceptTcpClient()) {
            client.ReceiveTimeout = 1000;
            using (var stream = client.GetStream()) using (var reader = new StreamReader(stream, Encoding.ASCII)) {
                string line; while (!String.IsNullOrEmpty(line = reader.ReadLine())) {}
                int count = ++Count;
                if (CancelPath != null) File.WriteAllText(CancelPath, "cancel");
                bool ok = count > 1 && !FailForever;
                string body = ok ? "{\"delay\":12}" : "{}";
                byte[] response = Encoding.ASCII.GetBytes("HTTP/1.1 " + (ok ? "200 OK" : "503 Unavailable") + "\r\nConnection: close\r\nContent-Length: " + body.Length + "\r\n\r\n" + body);
                stream.Write(response, 0, response.Length);
            }
        }} catch (SocketException) {} catch (IOException) {} catch (ObjectDisposedException) {}
    }
    public void Dispose() { listener.Stop(); worker.Join(2000); }
}
'@
& (Get-Module SplitWindows) {
    param($fixture)
    function script:Get-CoreApiContext {param($Root);@{url=$script:HealthUrl;token='fixture'}}
    foreach($scenario in @('warmup','deadline','cancel')){
        $server=[I2PHealthFixture]::new()
        try {
            $script:HealthUrl='http://127.0.0.1:'+$server.Port
            $server.FailForever=($scenario -ne 'warmup')
            if($scenario -eq 'cancel'){$server.CancelPath=Join-Path $fixture 'runtime\stop.request'}
            $clock=[Diagnostics.Stopwatch]::StartNew();$failed=$false
            try {Assert-SplitAwgHealth $fixture 'fixture' -Network i2p -Deadline $(if($scenario -eq 'deadline'){1500}else{6000})}
            catch {$failed=$true;if($scenario -eq 'cancel' -and $_.Exception.Message -ne 'Startup cancelled'){throw}}
            if($scenario -eq 'warmup' -and ($failed -or $server.Count -lt 2)){throw 'I2P did not retry a transient destination failure'}
            if($scenario -ne 'warmup' -and (!$failed -or $clock.ElapsedMilliseconds -gt 3000)){throw 'I2P health retry ignored deadline or cancellation'}
        } finally {
            $server.Dispose()
            [IO.File]::Delete((Join-Path $fixture 'runtime\stop.request'))
        }
    }
} $fixture
'PASS: I2P destination warmup retries stay within one deadline and respect cancellation; loopback HTTP only.'
& (Get-Module SplitWindows) {
    param($fixture,$transport)
    function script:Import-Module {param($Name,[switch]$DisableNameChecking)}
    function script:Test-SplitAdmin {$true}
    function script:Assert-NoOtherVpn {param($Root)}
    function script:Get-OwnedCore {param($Root) $null}
    function script:Get-CorePath {param($Root) Join-Path $Root 'bin\fixture.exe'}
    function script:Get-SplitSettings {param($Root) @{routing=@{mode='split';allow_lan=$true}}}
    function script:Enable-SplitTrafficGuard {param($Root,$Settings,$Transport);$script:Armed=$true}
    function script:Disable-SplitTrafficGuard {param($Root);throw 'Rollback must retain traffic protection'}
    function script:Start-SplitTor {param($Root,$Network);$script:Helper=$true;return $script:Transport}
    function script:Stop-SplitTor {$script:Helper=$false}
    function script:Set-SplitConfig {param($Root,$Settings,$TunEnabled,$Transport,[switch]$SessionOnly);throw 'Synthetic config validation failure'}
    function script:Restore-SessionDns {param($Root)}
    function script:Stop-OwnedCore {param($Root)}
    $script:Transport=$transport;$script:Armed=$false;$script:Helper=$false
    [IO.Directory]::CreateDirectory((Join-Path $fixture 'bin'))|Out-Null
    [IO.File]::WriteAllText((Join-Path $fixture 'bin\wintun.dll'),'Unused test fixture')
    $failed=$false
    try{Start-SplitSession $fixture $true 'tor'}catch{$failed=$true}
    if(!$failed -or $script:Helper -or !$script:Armed){throw 'Tor initialization failure leaked a helper or removed protection'}
    if(!(Test-Path -LiteralPath (Join-Path $fixture 'private\tor-mode'))){throw 'Tor recovery intent was not persisted'}
    function script:Get-OwnedCore {param($Root);[pscustomobject]@{Id=12345}}
    function script:Read-JsonFile {param($Path);@{phase='connected';transport=@{network='i2p'}}}
    function script:Test-SplitTor {$false}
    function script:Get-SplitTrafficGuardStatus {param($Root);@{configured=$true;verified=$true;protected=$true}}
    $status=Get-SplitStatus $fixture
    if($status.running -or $status.traffic_guard.protected -or !$status.traffic_guard.recovery_pending){throw 'Dead I2P helper was reported as a working protected connection'}
} $fixture $transport
'PASS: Tor configuration failure stops its helper, keeps protection and retains explicit recovery. All host networking mocked.'

Import-Module (Join-Path $package 'TrafficGuard.psm1') -Force -DisableNameChecking
& (Get-Module TrafficGuard) {
    function script:Import-Module {param($Name,[switch]$DisableNameChecking)}
    function script:Get-SplitTorOptions {param($Root);@{mode='snowflake';bridges=''}}
    function script:Get-SplitTorPath {param($Root);Join-Path $Root 'bin\mukhomor-tor.exe'}
    function script:Get-SplitTorOptions {param($Root);@{mode='snowflake-amp';bridges=''}}
    $torPlan=New-SplitTrafficGuardPlan $args[0] @{routing=@{allow_lan=$true}} @{network='tor'}
    if($torPlan.transport -ne 'tor-snowflake' -or $torPlan.allow_private_lan){throw 'Snowflake AMP did not receive the scoped bridge policy'}
    $plan=New-SplitTrafficGuardPlan $args[0] @{routing=@{allow_lan=$true}} @{network='i2p'}
    if($plan.transport -ne 'tor-bridges' -or $plan.allow_private_lan -or $plan.endpoints.Count -or [IO.Path]::GetFileName($plan.core_executable) -ne 'mukhomor-pt.exe'){throw 'I2P inherited a Snowflake UDP or LAN exception'}
} $fixture
'PASS: I2P helper failure requests recovery; its Windows guard permits only the TCP adapter even when Tor uses Snowflake.'

# Exercise the real ProcessStartInfo and pipe APIs on Windows PowerShell 5.1.
# Only adapter discovery and the helper path are mocked; no WFP or sockets run.
$processFixture=Join-Path $PSScriptRoot ('tor-process-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $processFixture 'bin'))|Out-Null
$unicodeRoot=Join-Path $processFixture ('unicode-'+[char]0x0428+[char]0x4ed9)
[IO.Directory]::CreateDirectory((Join-Path $unicodeRoot 'runtime'))|Out-Null
$helper=Join-Path $processFixture 'bin\pipe-fixture.exe'
Add-Type -OutputAssembly $helper -OutputType ConsoleApplication -ReferencedAssemblies @('System.dll','System.Core.dll','System.Web.Extensions.dll') -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Collections.Generic;
using System.Diagnostics;
using System.Text.RegularExpressions;
using System.Web.Script.Serialization;
public static class TorPipeFixture {
    public static int Main() {
        string root = Directory.GetParent(Path.GetDirectoryName(typeof(TorPipeFixture).Assembly.Location)).FullName;
        Stream input = Console.OpenStandardInput();
        MemoryStream bytes = new MemoryStream();
        int b;
        while ((b = input.ReadByte()) != -1 && b != 10) {
            bytes.WriteByte((byte)b);
            if (bytes.Length > 16384) return 2;
        }
        if (b != 10) return 3;
        string json = new UTF8Encoding(false, true).GetString(bytes.ToArray());
        if (json.Length == 0 || json[0] == '\ufeff') return 4;
        var init = new JavaScriptSerializer().Deserialize<Dictionary<string, object>>(json);
        string state = Path.Combine(root, "unicode-\u0428\u4ed9", "private", "tor", "state");
        string cache = Path.Combine(root, "unicode-\u0428\u4ed9", "private", "tor", "cache");
        if (init.Count != 6 || (string)init["state_dir"] != state || (string)init["cache_dir"] != cache || (string)init["interface"] != "7") return 5;
        var options = (Dictionary<string, object>)init["tor_options"];
        if ((string)options["mode"] != "snowflake" || (string)options["bridges"] != "") return 5;
        foreach (string key in new[] {"username", "password"})
            if (!Regex.IsMatch((string)init[key], "^[a-f0-9]{64}$")) return 6;
        if ((string)init["username"] == (string)init["password"]) return 7;
        File.WriteAllText(Path.Combine(root, "child.pid"), Process.GetCurrentProcess().Id.ToString());
        string mode = File.ReadAllText(Path.Combine(root, "mode.txt"));
        if (mode == "exit") return 8;
        if (mode == "bad-json") Console.WriteLine("invalid json");
        else if (mode == "missing-port") Console.WriteLine("{\"ready\":true}");
        else if (mode == "string-ready") Console.WriteLine("{\"ready\":\"true\",\"port\":12345}");
        else if (mode == "zero-port") Console.WriteLine("{\"ready\":true,\"port\":0}");
        else if (mode != "wait") Console.WriteLine("{\"ready\":true,\"port\":12345}");
        Console.Out.Flush();
        while (input.ReadByte() != -1) {}
        File.WriteAllText(Path.Combine(root, "eof.txt"), "closed");
        return 0;
    }
}
'@
Import-Module (Join-Path $package 'Tor.psm1') -Force -DisableNameChecking
& (Get-Module Tor) {
    param($root,$fixture,$helper)
    if((Get-SplitTorOptions $root).mode -cne 'snowflake'){throw 'First Tor connection requires manual bridge configuration'}
    $optionsPath=Join-Path $root 'private\tor-options.json'
    Write-AtomicJson $optionsPath @{mode='direct';bridges=''}
    try{if((Get-SplitTorOptions $root).mode -cne 'direct'){throw 'Default Tor preset overwrote an explicit saved choice'}}
    finally{[IO.File]::Delete($optionsPath)}
    $script:FixtureHelper=$helper
    function script:Get-SplitTorPath {param($Root);$script:FixtureHelper}
    function script:Get-NetRoute {param($DestinationPrefix,$ErrorAction);[pscustomobject]@{InterfaceIndex=7;RouteMetric=1}}
    function script:Get-NetAdapter {param($InterfaceIndex,$ErrorAction);[pscustomobject]@{HardwareInterface=$true;Status='Up';ifIndex=7;Name='Fixture';InterfaceGuid='11111111-1111-1111-1111-111111111111'}}
    try {
        [IO.File]::WriteAllText((Join-Path $fixture 'mode.txt'),'ready')
        $transport=Start-SplitTor $root
        if(!(Test-SplitTor) -or $transport.port -ne 12345 -or $transport.tor_proxy.udp){throw 'Real helper did not complete UTF-8 startup'}
        $childId=$script:TorProcess.Id
        Stop-SplitTor
        if((Test-SplitTor) -or (Get-Process -Id $childId -ErrorAction SilentlyContinue) -or !(Test-Path -LiteralPath (Join-Path $fixture 'eof.txt'))){throw 'Stopping Tor did not close stdin and reap the helper'}
        foreach($mode in @('bad-json','missing-port','string-ready','zero-port','exit','wait')){
            [IO.File]::WriteAllText((Join-Path $fixture 'mode.txt'),$mode)
            if($mode -eq 'wait'){[IO.File]::WriteAllText((Join-Path $root 'runtime\stop.request'),'stop')}
            $failure=''
            try {Start-SplitTor $root|Out-Null} catch {$failure=$_.Exception.Message}
            if($failure -notin @('P2P transport initialization failed','P2P bootstrap failed; the selected network may be unavailable or blocked','Startup cancelled') -or (Test-SplitTor)){throw ('Unexpected helper failure or leaked process: '+$mode+': '+$failure)}
            if($mode -eq 'wait' -and $failure -cne 'Startup cancelled'){throw 'Cancellation did not stop the pending helper'}
            $childFile=Join-Path $fixture 'child.pid'
            if(Test-Path -LiteralPath $childFile){
                $childId=[int][IO.File]::ReadAllText($childFile)
                if(Get-Process -Id $childId -ErrorAction SilentlyContinue){throw ('Failed helper survived: '+$mode)}
            }
        }
        [IO.File]::Delete((Join-Path $root 'runtime\stop.request'))
        $script:FixtureHelper=Join-Path $fixture 'bin\invalid.exe'
        [IO.File]::WriteAllText($script:FixtureHelper,'Not an executable')
        $failure=''
        try {Start-SplitTor $root|Out-Null} catch {$failure=$_.Exception.Message}
        if($failure -cne 'Cannot start bundled P2P transport' -or (Test-SplitTor)){throw 'Process creation failure was not reported in English'}
    } finally {Stop-SplitTor}
} $unicodeRoot $processFixture $helper
'PASS: real Windows helper startup, UTF-8 Unicode paths, EOF shutdown, malformed replies, cancellation and English process errors. No host networking changed.'
