# Exercise shipped defaults and actual seeded provider matching without internet
# access, TUN, adapter changes, or a remote VPN. All origins resolve to loopback.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$fixture=Join-Path $PSScriptRoot ('bundled-routing-'+[guid]::NewGuid().ToString('N'))
$passed=0;$core=$null;$origin=$null;$oldAssets=$env:MUKHOMOR_ASSETS
$allocated=New-Object 'Collections.Generic.HashSet[int]'
function Assert($Condition,[string]$Message) {if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++;Write-Host ('PASS: '+$Message)}
function Free-Port {
    do {$listener=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0);try {$listener.Start();$port=$listener.LocalEndpoint.Port} finally {$listener.Stop()}} while(!$allocated.Add($port))
    return $port
}
function Proxy-Get([string]$HostName) {
    $client=New-Object Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1',$settings.ports.proxy);$client.ReceiveTimeout=3000
        $stream=$client.GetStream()
        $bytes=[Text.Encoding]::ASCII.GetBytes("GET http://${HostName}:$originPort/ HTTP/1.1`r`nHost: ${HostName}:$originPort`r`nConnection: close`r`n`r`n")
        $stream.Write($bytes,0,$bytes.Length)
        $reader=New-Object IO.StreamReader($stream)
        try {return $reader.ReadToEnd()} finally {$reader.Dispose()}
    } finally {$client.Dispose()}
}
function Fixture-Config {
    $config=New-CoreConfig $fixture $settings $false
    $config.hosts=@{};foreach($name in $hostsToTest) {$config.hosts[$name]='127.0.0.1'}
    # REJECT models the VPN result; remove automatic LAN rules so loopback cannot
    # hide a wrong domain decision. Keep the production domain priority unchanged.
    $config.rules=@($config.rules|Where-Object {$_ -notmatch '^IP-CIDR'}|ForEach-Object {$_ -replace ',AWG$',',REJECT'})
    foreach($name in (Get-EffectiveSplitSettings $settings).direct.domain_rule_sets) {
        $config['rule-providers']["service-$name"]=@{type='file';behavior='domain';format='text';path="./runtime/rules/services/$name.list"}
    }
    $config.ntp=@{enable=$false}
    return $config
}
function Apply-Fixture {
    Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
    $config=Fixture-Config;Write-AtomicJson $configPath $config;Test-CoreConfig $package $configPath
    Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$configPath}|Out-Null
    Invoke-CoreApi $fixture '/connections' 'DELETE'|Out-Null
}
try {
    $metadata=Read-JsonFile (Join-Path $package 'lists\sources.json');$catalog=Get-DomainRuleSetCatalog
    foreach($name in $catalog.Keys) {
        $path=Join-Path $package "lists\seed-$name.list"
        $entries=@([IO.File]::ReadAllLines($path)|Where-Object {$_.Trim()})
        $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
        foreach($entry in $entries) {
            $domain=if($entry.StartsWith('+.')) {Normalize-Domain $entry -Suffix} else {Normalize-Domain $entry}
            if(!$domain -or !$seen.Add($entry)) {throw ('Invalid or duplicated bundled '+$name+' rule')}
        }
        Assert ($entries.Count -eq $metadata.sources[$name].entries -and (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ceq $metadata.sources[$name].seed_sha256) ($name+' seed is valid and matches recorded count/hash')
        Assert ($catalog[$name].url -ceq $metadata.sources[$name].download_url) ($name+' refresh uses the documented source')
    }
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
    Set-OwnedTestRoot $fixture $fixture;$env:MUKHOMOR_ASSETS=$package;Initialize-SplitRoot $fixture
    foreach($name in $catalog.Keys) {Assert ((Get-FileHash -LiteralPath (Join-Path $fixture "runtime\rules\services\$name.list")).Hash.ToLowerInvariant() -ceq $metadata.sources[$name].seed_sha256) ($name+' is seeded for first offline startup')}
    $settings=Get-SplitSettings $fixture
    Assert ((Get-RuleSettingsIssues $settings).Count -eq 0) 'expanded defaults have no invalid, duplicate or shadowed rules'
    $effective=Get-EffectiveSplitSettings $settings
    foreach($name in @('cs2.exe','Warframe.x64.exe','steam.exe','transmission-qt.exe')) {Assert ($effective.direct.process_names -contains $name) ('default process bypass includes '+$name)}
    foreach($name in @('java.exe','javaw.exe','python.exe','pythonw.exe','node.exe','launcher.exe','game.exe','svchost.exe')) {Assert ($effective.direct.process_names -notcontains $name) ('shared runtime '+$name+' keeps the VPN route')}
    $settings.ports.proxy=Free-Port;$settings.ports.controller=Free-Port;$settings.ports.dns=Free-Port
    Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
    Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
    $production=New-CoreConfig $fixture $settings $false
    Assert ($production['find-process-mode'] -eq 'strict' -and $production.rules -contains 'PROCESS-NAME,Warframe.x64.exe,DIRECT') 'expanded processes compile into native attribution rules'
    Assert ($production['rule-providers']['service-games'].type -eq 'http' -and $production['rule-providers']['service-games'].interval -eq 21600) 'gaming provider retains the six-hour automatic refresh'
    $directHosts=@('battle.net','eu.battle.net','epicgames.com','riotgames.com','gaijin.net','minecraft.net','faceit.com','xboxlive.com','roblox.com','hoyoverse.com','store.steampowered.com','gosuslugi.ru','online.sberbank.ru','service.su','service.moscow','service.xn--80adxhks','service.xn--p1acf','service.xn--d1acj3b','userapi.com','s3.yandexcloud.net','yandex.cloud','img.wbstatic.net','okko.tv','kaspersky.com','cdn.ozonusercontent.com')
    $vpnHosts=@('unrelated.example','discord.com','google.com','unrelated.cloudfront.net','battle.net.evil.test','yandexcloud.net.evil.test','in.th','game.in.th','ubs.com')
    $hostsToTest=$directHosts+$vpnHosts+@('www.ozonbank.ru')
    $configPath=Join-Path $fixture 'private\config.json';Write-AtomicJson $configPath (Fixture-Config);Test-CoreConfig $package $configPath
    $originPortPath=Join-Path $fixture 'origin-port.txt'
    $origin=Start-Process (Get-Command node.exe).Source -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'local-origin.cjs')+'"'),('"'+$originPortPath+'"')) -WindowStyle Hidden -PassThru
    for($n=0;$n -lt 300 -and !(Test-Path -LiteralPath $originPortPath);$n++){if($origin.HasExited){throw 'Origin exited'};Start-Sleep -Milliseconds 100}
    if(!(Test-Path -LiteralPath $originPortPath)){throw 'Local origin did not become ready'}
    $originPort=[int][IO.File]::ReadAllText($originPortPath)
    $core=Start-Process (Get-CorePath $package) -ArgumentList @('-d',('"'+$fixture+'"'),'-f',('"'+$configPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core-error.log')
    $ready=$false;for($n=0;$n -lt 40;$n++){if($core.HasExited){throw 'Core exited'};try {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null;$ready=$true;break} catch {Start-Sleep -Milliseconds 100}}
    Assert $ready 'pinned core starts with all expanded bundled providers'
    foreach($name in $directHosts) {Assert ((Proxy-Get $name) -match 'DIRECT_OK') ('bundled domain bypass routes '+$name)}
    foreach($name in $vpnHosts) {Assert ((Proxy-Get $name) -notmatch 'DIRECT_OK') ('default VPN route is retained for '+$name)}
    $gameNode=Join-Path $fixture 'Warframe.x64.exe';Copy-Item -LiteralPath (Get-Command node.exe).Source -Destination $gameNode
    $processOutput=& $gameNode (Join-Path $PSScriptRoot 'proxy-client.cjs') $settings.ports.proxy $originPort 'unrelated.example' 2>&1
    Assert ($LASTEXITCODE -eq 0 -and ($processOutput -join "`n") -match 'DIRECT_OK') 'actual game-named TCP process bypasses the VPN independent of domains'
    @($settings.presets|Where-Object builtin_id -eq games)[0].enabled=$false;Apply-Fixture
    Assert ((Proxy-Get 'epicgames.com') -notmatch 'DIRECT_OK') 'disabling Games disables its gaming domains'
    Assert ((Proxy-Get 'store.steampowered.com') -match 'DIRECT_OK') 'Steam remains independently enabled when Games is disabled'
    @($settings.presets|Where-Object builtin_id -eq russia)[0].enabled=$false;Apply-Fixture
    Assert ((Proxy-Get 's3.yandexcloud.net') -notmatch 'DIRECT_OK') 'disabling Russia disables its explicit and provider domains'
    Assert ((Proxy-Get 'cdn.ozonusercontent.com') -notmatch 'DIRECT_OK') 'Ozon is disabled when both Russian and Ozon presets are off'
    @($settings.presets|Where-Object builtin_id -eq ozon)[0].enabled=$true;Apply-Fixture
    foreach($name in @('cdn.ozonusercontent.com','www.ozonbank.ru')) {Assert ((Proxy-Get $name) -match 'DIRECT_OK') ('standalone Ozon preset covers '+$name)}
    $settings.routing.mode='full';$full=New-CoreConfig $fixture $settings $false
    Assert (!@($full.rules|Where-Object {$_ -match '^PROCESS-|^DOMAIN|^RULE-SET'}).Count) 'full routing still ignores all bundled bypasses'
    Write-Host ('PASS: '+$passed+' bundled-routing assertions; offline loopback only')
} finally {
    $env:MUKHOMOR_ASSETS=$oldAssets
    foreach($process in @($core,$origin)){if($process){try {if(!$process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}} finally {$process.Dispose()}}}
}
