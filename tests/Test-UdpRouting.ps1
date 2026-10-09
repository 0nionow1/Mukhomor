Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $root 'SplitVpn.psm1') -Force -DisableNameChecking
Import-Module (Join-Path $root 'SplitWindows.psm1') -Force -DisableNameChecking
$windowsModule=Get-Module SplitWindows
& $windowsModule {
    function script:Get-SplitStatus {param($Root) @{running=$true;starting=$false}}
    function script:Get-NetAdapter { @() }
}
function Free-Port {
    $l=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
    $l.Start(); $p=$l.LocalEndpoint.Port; $l.Stop(); return $p
}
$fixture=Join-Path $PSScriptRoot ('udp-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture) | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
Initialize-SplitRoot $fixture
$s=Get-SplitSettings $fixture
$s.dns_update.enabled=$false; $s.direct.domain_rule_sets=@(); $s.direct.rule_set_exclusions=@()
$s.direct.process_names=@('node.exe'); $s.direct.process_paths=@(); $s.direct.domains=@(); $s.direct.domain_suffixes=@('ru'); $s.direct.ip_cidrs=@()
$s.ports.proxy=Free-Port; $s.ports.controller=Free-Port; $s.ports.dns=Free-Port
Write-AtomicJson (Join-Path $fixture 'settings.json') $s
Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = eCtXsJZ27+4PbhDkHnB923tkUn2Gj59wZw5wFA75MnU=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
$c=New-CoreConfig $fixture $s $false
$c.hosts=@{'allowed.ru'='127.0.0.1';'sub.allowed.ru'='127.0.0.1';'unrelated.example'='127.0.0.1'}
# Remove LAN/endpoint rules so they cannot mask executable or domain matches.
# No real AWG dial is allowed: the fixture default is REJECT.
$c.rules=@($c.rules | Where-Object { $_ -notmatch '^IP-CIDR' } | ForEach-Object {$_ -replace ',AWG$',',REJECT'})
$path=Join-Path $fixture 'private\config.json'; Write-AtomicJson $path $c
Test-CoreConfig $root $path
$core=$null
try {
    $core=Start-Process (Get-CorePath $root) -ArgumentList @('-d',('"'+$fixture+'"'),'-f',('"'+$path+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core-error.log')
    $ready=$false
    for ($i=0;$i -lt 40;$i++) {
        try { Invoke-CoreApi $fixture '/version' | Out-Null; $ready=$true; break } catch { Start-Sleep -Milliseconds 100 }
    }
    if (!$ready) { throw 'UDP fixture core did not start' }
    foreach ($mode in @('process','domain','blocked')) {
        if ($mode -eq 'domain') {
            $c.rules=@($c.rules | Where-Object {$_ -notmatch '^PROCESS-'})
            Write-AtomicJson $path $c; Test-CoreConfig $root $path
            Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$path} | Out-Null
            Invoke-CoreApi $fixture '/connections' 'DELETE' | Out-Null
        }
        $output=& node.exe (Join-Path $PSScriptRoot 'udp-client.cjs') $fixture $mode 2>&1
        if ($LASTEXITCODE -ne 0) { throw ($output -join "`n") }
        $output
        if ($mode -eq 'process') {
            $diagnostic=Read-JsonFile (Write-SplitDiagnostics $fixture)
            if (!$diagnostic.udp_connections.Count -or $diagnostic.notes.Count -or $diagnostic.active_tun_stack -ne 'system') {throw 'Real UDP diagnostics failed'}
            if (@($diagnostic.udp_connections | Where-Object {$_.chains -notcontains 'DIRECT'}).Count) {throw 'Diagnostics did not report real UDP DIRECT chains'}
            $text=[IO.File]::ReadAllText((Join-Path $fixture 'runtime\udp-diagnostics.json'))
            if ($text.Contains($c.secret) -or $text.Contains($c.proxies[0]['private-key'])) {throw 'Diagnostic report exposed secrets'}
            'PASS: diagnostics captured actual UDP rules/paths/stack without the API token or VPN key'
        }
    }
    'UDP tests used loopback and the real Mihomo core; they do not exercise Wintun or external game servers.'
} finally {
    if ($core -and !$core.HasExited) { $core.Kill(); $core.WaitForExit(10000) | Out-Null }
}
