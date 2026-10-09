Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $root 'SplitVpn.psm1') -Force -DisableNameChecking
Import-Module (Join-Path $root 'SplitWindows.psm1') -Force -DisableNameChecking
function Free-Port {
    $listener=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
    $listener.Start(); $port=$listener.LocalEndpoint.Port; $listener.Stop(); return $port
}
function Proxy-Get([int]$ProxyPort,[int]$OriginPort,[string]$HostName) {
    $client=New-Object Net.Sockets.TcpClient
    $client.Connect('127.0.0.1',$ProxyPort); $client.ReceiveTimeout=5000
    try {
        $stream=$client.GetStream()
        $request=[Text.Encoding]::ASCII.GetBytes("GET http://${HostName}:$OriginPort/ HTTP/1.1`r`nHost: ${HostName}:$OriginPort`r`nConnection: close`r`n`r`n")
        $stream.Write($request,0,$request.Length)
        $reader=New-Object IO.StreamReader($stream)
        return $reader.ReadToEnd()
    } finally { $client.Dispose() }
}
$fixture=Join-Path $PSScriptRoot ('routing-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture) | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
Initialize-SplitRoot $fixture
$s=Get-SplitSettings $fixture
$s.dns_update.enabled=$true; $s.direct.domain_rule_sets=@(); $s.direct.rule_set_exclusions=@()
$s.ports.proxy=Free-Port; $s.ports.controller=Free-Port; $s.ports.dns=Free-Port
$s.direct.process_names=@('node.exe'); $s.direct.process_paths=@(); $s.direct.domains=@(); $s.direct.domain_suffixes=@('ru'); $s.direct.ip_cidrs=@()
Write-AtomicJson (Join-Path $fixture 'settings.json') $s
# Synthetic fixture only. No real VPN credentials or remote VPN connection.
Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = eCtXsJZ27+4PbhDkHnB923tkUn2Gj59wZw5wFA75MnU=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
$c=New-CoreConfig $fixture $s $false
$c.hosts=@{'allowed.ru'='127.0.0.1';'sub.allowed.ru'='127.0.0.1';'unrelated.example'='127.0.0.1'}
# Model a shared address in the fallback list. Remove local-network rules in this
# fixture so they cannot mask the domain guard. Production keeps LAN access.
$c.rules=@($c.rules | Where-Object { $_ -notmatch '^IP-CIDR' } | ForEach-Object { $_ -replace ',AWG$',',REJECT' })
Write-AtomicText (Join-Path $fixture 'runtime\rules\dns-ips.yaml') "payload:`n  - '127.0.0.1/32'`n"
$configPath=Join-Path $fixture 'private\config.json'
Write-AtomicJson $configPath $c
Test-CoreConfig $root $configPath
$origin=$null; $core=$null
try {
    $originPortPath=Join-Path $fixture 'origin-port.txt'
    $origin=Start-Process (Get-Command node.exe).Source -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'local-origin.cjs')+'"'),('"'+$originPortPath+'"')) -WindowStyle Hidden -PassThru
    for ($i=0;$i -lt 40 -and !(Test-Path -LiteralPath $originPortPath);$i++) { Start-Sleep -Milliseconds 100 }
    $originPort=[int](Get-Content -LiteralPath $originPortPath -Raw)
    $core=Start-Process (Get-CorePath $root) -ArgumentList @('-d',('"'+$fixture+'"'),'-f',('"'+$configPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core-error.log')
    $ready=$false
    for ($i=0;$i -lt 40;$i++) {
        try { Invoke-CoreApi $fixture '/version' | Out-Null; $ready=$true; break } catch { Start-Sleep -Milliseconds 100 }
    }
    if (!$ready) { throw 'Fixture core did not start' }
    Assert-SplitTunDns -Server '127.0.0.1' -Port $s.ports.dns
    'PASS: startup DNS probe accepts a real Mihomo UDP DNS response'
    foreach ($hostName in @('allowed.ru','sub.allowed.ru')) {
        if ((Proxy-Get $s.ports.proxy $originPort $hostName) -notmatch 'DIRECT_OK') { throw "Domain bypass failed: $hostName" }
    }
    'PASS: .ru and subdomain routing through real Mihomo'
    if ((Proxy-Get $s.ports.proxy $originPort 'unrelated.example') -match 'DIRECT_OK') { throw 'Shared-IP fallback incorrectly bypassed unrelated domain' }
    'PASS: unrelated domain on shared IP remains under the default VPN policy'
    if ((Proxy-Get $s.ports.proxy $originPort '127.0.0.1') -notmatch 'DIRECT_OK') { throw 'Bare IP fallback failed' }
    'PASS: bare IP uses refreshed-IP fallback'
    $nodeResult=& node.exe (Join-Path $PSScriptRoot 'proxy-client.cjs') $s.ports.proxy $originPort 'unrelated.example' 2>&1
    if ($LASTEXITCODE -ne 0 -or ($nodeResult -join '') -notmatch 'DIRECT_OK') { throw 'Executable bypass failed in real core' }
    'PASS: Windows .exe rule overrides domain default in real Mihomo'
    $explicit=$c.Clone(); $explicit.rules=@('IP-CIDR,127.0.0.1/32,DIRECT')+@($c.rules)
    $explicitPath=Join-Path $fixture 'private\explicit.json'; Write-AtomicJson $explicitPath $explicit
    Test-CoreConfig $root $explicitPath
    Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$explicitPath} | Out-Null
    if ((Proxy-Get $s.ports.proxy $originPort 'unrelated.example') -notmatch 'DIRECT_OK') { throw 'Explicit IP exception did not apply to known domain' }
    'PASS: explicit IP exception takes priority for known domains'
    $changed=$c; $changed.rules=@($c.rules | Where-Object { $_ -ne 'DOMAIN-SUFFIX,ru,DIRECT' })
    $reloadPath=Join-Path $fixture 'private\reload.json'; Write-AtomicJson $reloadPath $changed
    Test-CoreConfig $root $reloadPath
    Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$reloadPath} | Out-Null
    if ((Proxy-Get $s.ports.proxy $originPort 'allowed.ru') -match 'DIRECT_OK') { throw 'Live rule update failed' }
    'PASS: validated rule reload applies to new connections'
    'All integration traffic used loopback; TUN and system DNS were unchanged.'
} finally {
    if ($core -and !$core.HasExited) { $core.Kill(); $core.WaitForExit(10000) | Out-Null }
    if ($origin -and !$origin.HasExited) { $origin.Kill(); $origin.WaitForExit(10000) | Out-Null }
}
