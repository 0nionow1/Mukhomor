# Real rule matching and reloads in the pinned core, with loopback origins only.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$fixture=Join-Path $PSScriptRoot ('large-rules-'+[guid]::NewGuid().ToString('N'))
$passed=0; $core=$null; $origin=$null; $previousCore=$env:MUKHOMOR_CORE_PATH
$allocated=New-Object 'Collections.Generic.HashSet[int]'
function Assert($Condition,[string]$Message) {if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++;Write-Host ('PASS: '+$Message)}
function Free-Port {
    do {$l=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0);try {$l.Start();$p=$l.LocalEndpoint.Port} finally {$l.Stop()}} while(!$allocated.Add($p))
    return $p
}
function Proxy-Get([string]$HostName) {
    $client=New-Object Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1',$settings.ports.proxy);$client.ReceiveTimeout=3000
        $stream=$client.GetStream()
        $data=[Text.Encoding]::ASCII.GetBytes("GET http://${HostName}:$originPort/ HTTP/1.1`r`nHost: ${HostName}:$originPort`r`nConnection: close`r`n`r`n")
        $stream.Write($data,0,$data.Length)
        $reader=New-Object IO.StreamReader($stream)
        try {return $reader.ReadToEnd()} finally {$reader.Dispose()}
    } finally {$client.Dispose()}
}
function Apply($Config) {
    Write-AtomicJson $configPath $Config;Test-CoreConfig $package $configPath
    Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$configPath}|Out-Null
    Invoke-CoreApi $fixture '/connections' 'DELETE'|Out-Null
}
try {
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
    Initialize-SplitRoot $fixture
    $settings=Get-SplitSettings $fixture
    $settings.ports.proxy=Free-Port;$settings.ports.controller=Free-Port;$settings.ports.dns=Free-Port
    $settings.dns_update.enabled=$true;$settings.dns_update.observe_subdomains=$false
    $settings.direct.domain_rule_sets=@('steam');$settings.direct.rule_set_exclusions=@('priority.test')
    $settings.direct.process_names=@('node.exe');$settings.direct.process_paths=@()
    $settings.direct.domains=@('exact.test','priority.test')+@(1..40|ForEach-Object {"exact$_.test"})
    $cyrillicTld=[string][char]0x0440+[char]0x0444
    $settings.direct.domain_suffixes=@('ru',$cyrillicTld,'suffix.test')+@(1..40|ForEach-Object {"suffix$_.test"})
    $settings.direct.ip_cidrs=@('127.0.0.1/32','::1/128')+@(1..40|ForEach-Object {"203.0.113.$_/32"})
    Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
    Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = eCtXsJZ27+4PbhDkHnB923tkUn2Gj59wZw5wFA75MnU=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
    $config=New-CoreConfig $fixture $settings $false
    Assert ($config['rule-providers']['custom-domains'].type -eq 'inline' -and $config['rule-providers']['custom-ips'].type -eq 'inline') 'large lists use native in-memory domain and IP sets'
    Assert ($config['find-process-mode'] -eq 'strict') 'EXE rules retain native process attribution'
    $config.hosts=@{}
    foreach($name in @('exact.test','sub.exact.test','priority.test','suffix.test','sub.suffix.test','evilsuffix.test','allowed.ru','sub.allowed.ru','xn--e1afmkfd.xn--p1ai','unrelated.example')) {$config.hosts[$name]='127.0.0.1'}
    # Remove automatic LAN/endpoint bypass only. Explicit custom IP rules remain
    # until the test removes them, so no other DIRECT rule can mask a result.
    $config.rules=@($config.rules|Where-Object {$_ -notmatch '^IP-CIDR'}|ForEach-Object {$_ -replace ',AWG$',',REJECT'})
    $config['rule-providers']['service-steam']=@{type='inline';behavior='domain';payload=@('+.priority.test')}
    $config.ntp=@{enable=$false}
    Write-AtomicText (Join-Path $fixture 'runtime\rules\dns-ips.yaml') "payload:`n  - '127.0.0.1/32'`n  - '::1/128'`n"
    $configPath=Join-Path $fixture 'private\config.json';Write-AtomicJson $configPath $config
    Test-CoreConfig $package $configPath
    $originPortPath=Join-Path $fixture 'origin-port.txt'
    $origin=Start-Process (Get-Command node.exe).Source -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'local-origin.cjs')+'"'),('"'+$originPortPath+'"')) -WindowStyle Hidden -PassThru
    for($n=0;$n -lt 40 -and !(Test-Path -LiteralPath $originPortPath);$n++) {if($origin.HasExited){throw 'Origin exited'};Start-Sleep -Milliseconds 100}
    $originPort=[int][IO.File]::ReadAllText($originPortPath)
    $core=Start-Process (Get-CorePath $package) -ArgumentList @('-d',('"'+$fixture+'"'),'-f',('"'+$configPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core-error.log')
    $ready=$false
    for($n=0;$n -lt 40;$n++){if($core.HasExited){throw 'Core exited'};try {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null;$ready=$true;break} catch {Start-Sleep -Milliseconds 100}}
    Assert $ready 'core accepts optimized production config'
    Assert ((Proxy-Get 'unrelated.example') -match 'DIRECT_OK') 'explicit IP set applies to known domains'
    Assert ((Proxy-Get 'priority.test') -match 'DIRECT_OK') 'custom domain still wins over a service-list exclusion'
    Assert ((Proxy-Get 'sub.priority.test') -notmatch 'DIRECT_OK') 'service-list exclusion retains priority over custom IP set'
    foreach($mode in @('path','ip')) {
        $variant=Convert-ToMap $config
        if($mode -eq 'path') {
            $nodePath=(Get-Command node.exe).Source.ToUpperInvariant()
            $variant.rules=@($variant.rules|ForEach-Object {if($_ -eq 'PROCESS-NAME,node.exe,DIRECT'){"PROCESS-PATH,$nodePath,DIRECT"}else{$_}})
        } else {$variant.rules=@($variant.rules|Where-Object {$_ -notmatch '^PROCESS-'});$variant['find-process-mode']='off'}
        Apply $variant
        $output=& node.exe (Join-Path $PSScriptRoot 'udp-client.cjs') $fixture $mode 2>&1
        if($LASTEXITCODE -ne 0){throw ($output -join "`n")};$output
    }
    # Remove explicit IPs to expose domain boundary errors and the shared-CDN guard.
    $config.rules=@($config.rules|Where-Object {$_ -ne 'RULE-SET,custom-ips,DIRECT'})
    Apply $config
    foreach($name in @('exact.test','suffix.test','sub.suffix.test','allowed.ru','sub.allowed.ru','xn--e1afmkfd.xn--p1ai')) {Assert ((Proxy-Get $name) -match 'DIRECT_OK') ('native domain set routes '+$name)}
    foreach($name in @('sub.exact.test','evilsuffix.test','unrelated.example')) {Assert ((Proxy-Get $name) -notmatch 'DIRECT_OK') ('native domain set preserves boundary/default for '+$name)}
    Assert ((Proxy-Get '127.0.0.1') -match 'DIRECT_OK') 'bare IP still uses DNS-IP fallback'
    $env:MUKHOMOR_CORE_PATH=Get-CorePath $package
    # Exercise actual wildcard UDP sockets, multiple peers and idle resumption.
    foreach($mode in @('process','domain','blocked')) {
        if($mode -eq 'domain') {$config.rules=@($config.rules|Where-Object {$_ -notmatch '^PROCESS-'});$config['find-process-mode']='off';Apply $config}
        $output=& node.exe (Join-Path $PSScriptRoot 'udp-client.cjs') $fixture $mode 2>&1
        if($LASTEXITCODE -ne 0){throw ($output -join "`n")};$output
        # The existing blocked fixture expects no bare-IP fallback.
        if($mode -eq 'domain') {$config.rules=@($config.rules|Where-Object {$_ -ne 'RULE-SET,dns-ips,DIRECT,no-resolve'});Apply $config}
    }
    $config['rule-providers']['custom-domains'].payload=@($config['rule-providers']['custom-domains'].payload|Where-Object {$_ -ne 'exact.test'})
    Apply $config
    Assert ((Proxy-Get 'exact.test') -notmatch 'DIRECT_OK') 'live inline provider reload removes an exact exception'
    # Controller access must not normalize domain/IP/process lists on each probe.
    # Instrument normalization itself, then exercise the real authenticated API.
    $module=Get-Module SplitVpn
    & $module {
        param($fixture)
        $script:OriginalDomainNormalizer=${function:Normalize-Domain}
        function script:Normalize-Domain {param($Value,[switch]$Suffix);throw 'API probe unexpectedly normalized exceptions'}
        try {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null}
        finally {Set-Item Function:script:Normalize-Domain $script:OriginalDomainNormalizer}
    } $fixture
    Assert $true 'controller probes avoid exception normalization'
    $bad=Read-JsonFile (Join-Path $fixture 'settings.json');$bad.ports.controller='1234'
    Write-AtomicJson (Join-Path $fixture 'settings.json') $bad
    $rejected=$false;try {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null} catch {$rejected=$true}
    Assert $rejected 'lightweight API path still rejects an invalid controller port'
    Write-Host ('PASS: '+$passed+' large-list assertions; real TCP/UDP, no system network changes')
} finally {
    $env:MUKHOMOR_CORE_PATH=$previousCore
    foreach($process in @($core,$origin)){if($process){try {if(!$process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}} finally {$process.Dispose()}}}
}
