# Real, loopback-only protocol handshakes. No TUN, system DNS or service changes.
param([string]$Base)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=if($Base){[IO.Path]::GetFullPath($Base)}else{Split-Path -Parent $PSScriptRoot}
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$fixture=Join-Path $PSScriptRoot ('protocol-'+[guid]::NewGuid().ToString('N'))
$passed=0; $server=$null; $client=$null; $origin=$null
$previousCore=$env:MUKHOMOR_CORE_PATH
$corePath=Get-CorePath $package
$allocated=New-Object 'Collections.Generic.HashSet[int]'
function Assert($Condition,[string]$Message) {
    if(!$Condition){throw ('FAIL: '+$Message)}
    $script:passed++; Write-Host ('PASS: '+$Message)
}
function Free-Port {
    do {
        $listener=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
        try {$listener.Start();$port=$listener.LocalEndpoint.Port} finally {$listener.Stop()}
    } while(!$allocated.Add($port))
    return $port
}
function New-FixtureRoot([string]$Name) {
    $path=Join-Path $fixture $Name
    [IO.Directory]::CreateDirectory($path)|Out-Null
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $path 'settings.json')
    Initialize-SplitRoot $path
    $settings=Get-SplitSettings $path
    $settings.ports.proxy=Free-Port; $settings.ports.controller=Free-Port; $settings.ports.dns=Free-Port
    $settings.dns_update.enabled=$false; $settings.direct.domain_rule_sets=@(); $settings.direct.rule_set_exclusions=@()
    Write-AtomicJson (Join-Path $path 'settings.json') $settings
    return $path
}
function Start-FixtureCore([string]$Root,[string]$ConfigPath,[string]$Name) {
    Test-CoreConfig $package $ConfigPath
    $process=Start-Process (Get-CorePath $package) -ArgumentList @('-d',('"'+$Root+'"'),'-f',('"'+$ConfigPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture ($Name+'.log')) -RedirectStandardError (Join-Path $fixture ($Name+'-error.log'))
    try {
        for($n=0;$n -lt 40;$n++){
            if($process.HasExited){throw ('Fixture '+$Name+' core exited before readiness')}
            try {Invoke-CoreApi $Root '/version' -TimeoutSeconds 1|Out-Null;return $process} catch {Start-Sleep -Milliseconds 100}
        }
        throw ('Fixture '+$Name+' core did not become ready')
    } catch {
        if(!$process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}
        $process.Dispose();throw
    }
}
function Proxy-Get([int]$ProxyPort,[int]$OriginPort) {
    $socket=New-Object Net.Sockets.TcpClient
    try {
        $socket.Connect('127.0.0.1',$ProxyPort);$socket.ReceiveTimeout=4000;$socket.SendTimeout=4000
        $stream=$socket.GetStream()
        $request=[Text.Encoding]::ASCII.GetBytes("GET http://127.0.0.1:$OriginPort/ HTTP/1.1`r`nHost: 127.0.0.1:$OriginPort`r`nConnection: close`r`n`r`n")
        $stream.Write($request,0,$request.Length)
        $reader=New-Object IO.StreamReader($stream)
        try {return $reader.ReadToEnd()} finally {$reader.Dispose()}
    } finally {$socket.Dispose()}
}
function Client-Config($Proxy) {
    Write-AtomicJson (Join-Path $clientRoot 'private\selected-profile.json') @{schema=1;proxy=$Proxy}
    $config=New-CoreConfig $clientRoot (Get-SplitSettings $clientRoot) $false
    # Production deliberately bypasses LAN traffic. Force this fixture's origin
    # through the protocol so an accidental DIRECT path cannot pass the test.
    $config.rules=@('MATCH,AWG');$config.dns=@{enable=$false};$config.sniffer=@{enable=$false}
    $config['rule-providers']=@{};$config['find-process-mode']='off';$config.ntp=@{enable=$false}
    return $config
}
try {
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    $env:MUKHOMOR_CORE_PATH=$corePath
    $originPortPath=Join-Path $fixture 'origin-port.txt'
    $origin=Start-Process (Get-Command node.exe).Source -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'local-origin.cjs')+'"'),('"'+$originPortPath+'"')) -WindowStyle Hidden -PassThru
    for($n=0;$n -lt 40 -and !(Test-Path -LiteralPath $originPortPath);$n++){
        if($origin.HasExited){throw 'Local origin exited before readiness'}
        Start-Sleep -Milliseconds 100
    }
    $originPort=[int][IO.File]::ReadAllText($originPortPath)
    $serverRoot=New-FixtureRoot 'server';$clientRoot=New-FixtureRoot 'client'
    $serverSettings=Get-SplitSettings $serverRoot;$clientSettings=Get-SplitSettings $clientRoot
    $ssPort=Free-Port;$vlessPort=Free-Port;$vmessPort=Free-Port;$mixedPort=Free-Port
    $uuid='11111111-1111-4111-8111-111111111111'
    $password='PUBLIC-LOOPBACK-FIXTURE-PASSWORD';$username='fixture'
    $serverConfig=@{
        'mixed-port'=0;'allow-lan'=$false;'bind-address'='127.0.0.1';mode='rule';'log-level'='warning'
        'external-controller'="127.0.0.1:$($serverSettings.ports.controller)"
        secret=([IO.File]::ReadAllText((Join-Path $serverRoot 'private\api-token.txt')).Trim())
        'geo-auto-update'=$false;dns=@{enable=$false};tun=@{enable=$false};ntp=@{enable=$false};rules=@('MATCH,DIRECT');'rule-providers'=@{}
        listeners=@(
            @{name='Fixture SS';type='shadowsocks';listen='127.0.0.1';port=[string]$ssPort;cipher='aes-128-gcm';password=$password;udp=$false},
            @{name='Fixture VLESS';type='vless';listen='127.0.0.1';port=[string]$vlessPort;'allow-insecure'=$true;users=@(@{uuid=$uuid})},
            @{name='Fixture VMess';type='vmess';listen='127.0.0.1';port=[string]$vmessPort;users=@(@{uuid=$uuid;alterId=0})},
            @{name='Fixture HTTP SOCKS5';type='mixed';listen='127.0.0.1';port=[string]$mixedPort;users=@(@{username=$username;password=$password});udp=$false}
        )
    }
    $serverPath=Join-Path $serverRoot 'private\server.json';Write-AtomicJson $serverPath $serverConfig
    $profiles=@(
        @{name='Fixture SS';type='ss';server='127.0.0.1';port=$ssPort;cipher='aes-128-gcm';password=$password;udp=$false},
        @{name='Fixture VLESS';type='vless';server='127.0.0.1';port=$vlessPort;uuid=$uuid;tls=$false;udp=$false},
        @{name='Fixture VMess';type='vmess';server='127.0.0.1';port=$vmessPort;uuid=$uuid;alterId=0;cipher='auto';tls=$false;udp=$false},
        @{name='Fixture HTTP';type='http';server='127.0.0.1';port=$mixedPort;username=$username;password=$password;tls=$false},
        @{name='Fixture SOCKS5';type='socks5';server='127.0.0.1';port=$mixedPort;username=$username;password=$password;tls=$false;udp=$false}
    )
    $nodes=@(Convert-SplitProfileText ($profiles|ConvertTo-Json -Depth 12 -Compress) 'json')
    Assert ($nodes.Count -eq 5) 'all five loopback profiles pass the production importer'
    Test-SplitProfileProxies $clientRoot @($nodes|ForEach-Object {$_.proxy})
    $server=Start-FixtureCore $serverRoot $serverPath 'server'
    $clientPath=Join-Path $clientRoot 'private\client.json'
    $delayRoute='/proxies/AWG/delay?timeout=2500&url='+[Uri]::EscapeDataString("http://127.0.0.1:$originPort/")
    foreach($node in $nodes){
        $config=Client-Config $node.proxy;Write-AtomicJson $clientPath $config
        Assert (!$config.tun.enable -and !$config.dns.enable -and !$config.ntp.enable -and !$config['geo-auto-update'] -and !$config['rule-providers'].Count -and ($config|ConvertTo-Json -Depth 40 -Compress) -notmatch 'https?://' -and $config.rules.Count -eq 1 -and $config.rules[0] -eq 'MATCH,AWG') ($node.protocol+': isolated config forces protocol transport without external update URLs')
        if(!$client){$client=Start-FixtureCore $clientRoot $clientPath 'client'}else{
            Test-CoreConfig $package $clientPath
            Invoke-CoreApi $clientRoot '/configs?force=true' 'PUT' @{path=$clientPath}|Out-Null
        }
        $delay=Invoke-CoreApi $clientRoot $delayRoute -TimeoutSeconds 4
        Assert ($null -ne $delay.delay -and $delay.delay -ge 0) ($node.protocol+': actual handshake passes the Mihomo HTTP health probe')
        $response=Proxy-Get $clientSettings.ports.proxy $originPort
        Assert ($response -match '^HTTP/1\.[01] 200 ' -and $response -match 'DIRECT_OK') ($node.protocol+': tunneled request reaches the HTTP origin')
    }
    # A valid config with a wrong SS password must fail both real paths. This
    # proves the positive results were not merely local DIRECT requests.
    $invalid=$nodes[0].proxy.Clone();$invalid.password='WRONG-LOOPBACK-FIXTURE-PASSWORD'
    Write-AtomicJson $clientPath (Client-Config $invalid)
    Test-CoreConfig $package $clientPath
    Invoke-CoreApi $clientRoot '/configs?force=true' 'PUT' @{path=$clientPath}|Out-Null
    Invoke-CoreApi $serverRoot '/version' -TimeoutSeconds 1|Out-Null
    Invoke-CoreApi $clientRoot '/version' -TimeoutSeconds 1|Out-Null
    Assert (!$origin.HasExited -and !$server.HasExited -and !$client.HasExited -and (Proxy-Get $originPort $originPort) -match 'DIRECT_OK') 'origin and both engines remain healthy before the credential rejection control'
    $healthRejected=$false
    try {Invoke-CoreApi $clientRoot $delayRoute -TimeoutSeconds 4|Out-Null} catch {$healthRejected=$true}
    Assert $healthRejected 'wrong SS credentials fail the actual HTTP health probe'
    $response='';try {$response=Proxy-Get $clientSettings.ports.proxy $originPort} catch {}
    Assert ($response -notmatch 'DIRECT_OK') 'wrong SS credentials cannot bypass the protocol and reach the origin'
    Write-Host ('PASS: '+$passed+' real loopback protocol assertions; TUN, system DNS and services unchanged')
} catch {
    # These logs contain only this script's public loopback fixture credentials.
    foreach($name in @('server.log','server-error.log','client.log','client-error.log')){
        $path=Join-Path $fixture $name
        if(Test-Path -LiteralPath $path){Write-Host $name;Get-Content -LiteralPath $path -Tail 12|Write-Host}
    }
    throw
} finally {
    $env:MUKHOMOR_CORE_PATH=$previousCore
    foreach($process in @($client,$server,$origin)){
        if($process){try {if(!$process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}} catch {} finally {$process.Dispose()}}
    }
    $resolved=[IO.Path]::GetFullPath($fixture)
    if($resolved.StartsWith(([IO.Path]::GetFullPath($PSScriptRoot)+'\protocol-'),[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^protocol-[a-f0-9]{32}$'){
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
    }
}
