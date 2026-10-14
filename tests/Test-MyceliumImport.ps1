param([string]$CorePath)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'ProfileImport.psm1') -Force -DisableNameChecking
$passed=0
$seed='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
$pin='1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef'
$colonPin=([regex]::Matches($pin,'..')|ForEach-Object {$_.Value}) -join ':'
$canary='PRIVATE-MYCELIUM-CANARY'
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
function Parse([string]$Text,[string]$Format='auto'){return ,@(Convert-SplitProfileText $Text $Format 'Fixture')}
function Reject([scriptblock]$Action,[string]$Message){
    $errorText='';try{& $Action|Out-Null}catch{$errorText=$_.Exception.Message}
    Assert ($errorText.StartsWith('Profile import:',[StringComparison]::Ordinal) -and !$errorText.Contains($seed) -and !$errorText.Contains($canary)) $Message
}
function NodeJson($Node){return $Node|ConvertTo-Json -Depth 12 -Compress}

$capabilities=@(Get-SplitProfileCapabilities)
$capability=@($capabilities|Where-Object {$_.type -eq 'mycelium'})
Assert ($capabilities.Count -eq 22 -and $capability.Count -eq 1 -and $capability[0].json -and $capability[0].uri -and !$capability[0].conf -and !$capability[0].ovpn) 'capabilities advertise the native Mycelium JSON/YAML and URI importer'
$basic=@{name='JSON Fixture';type='mycelium';server='VPN.Fixture.Invalid';port=443;seed=$seed.ToUpperInvariant()}
$node=(Parse (NodeJson $basic))[0]
Assert ($node.protocol -eq 'mycelium' -and $node.name -eq 'JSON Fixture' -and $node.proxy.name -eq 'AWG') 'Mycelium keeps the standard display name and internal routing alias'
Assert ($node.proxy.seed -ceq $seed -and $node.proxy.server -ceq 'vpn.fixture.invalid' -and $node.proxy.sni -ceq $node.proxy.server -and $node.proxy.udp) 'JSON defaults to verified server identity and UDP with a canonical 32-byte seed'
Assert ($node.proxy.Count -eq 7 -and !$node.proxy.ContainsKey('fingerprint') -and !$node.proxy.ContainsKey('client-fingerprint')) 'optional TLS pin and browser fingerprint remain absent when not supplied'

$link="mycelium://$seed@192.0.2.1:443?sni=TLS.Fixture.Invalid&fp=$colonPin&client-fingerprint=chrome&udp=false#URI%20Fixture"
$uri=(Parse $link)[0]
Assert ($uri.name -eq 'URI Fixture' -and $uri.proxy.server -eq '192.0.2.1' -and $uri.proxy.sni -ceq 'tls.fixture.invalid') 'share links keep the endpoint and certificate identity independent'
Assert ($uri.proxy.fingerprint -ceq $pin -and $uri.proxy['client-fingerprint'] -eq 'chrome' -and !$uri.proxy.udp) 'share-link fp is a certificate SHA256 pin and UDP can be disabled'
$ipv6=(Parse "mycelium://$seed@[2001:db8::1]:8443?sni=2001%3Adb8%3A%3A2")[0]
Assert ($ipv6.proxy.server -eq '2001:db8::1' -and $ipv6.proxy.sni -eq '2001:db8::2' -and $ipv6.proxy.port -eq 8443) 'bracketed IPv6 endpoints and IP certificate identities are accepted'
$defaultIdentity=(Parse "mycelium://$seed@fixture.invalid:443")[0]
Assert ($defaultIdentity.proxy.sni -eq 'fixture.invalid' -and $defaultIdentity.proxy.udp) 'a share link without optional parameters uses the endpoint identity'
$yaml="name: YAML Fixture`ntype: mycelium`nserver: fixture.invalid`nport: 443`nseed: '$seed'`nsni: tls.fixture.invalid`nfingerprint: '$pin'`nclient-fingerprint: firefox`nudp: true"
$yamlNode=(Parse $yaml)[0]
Assert ($yamlNode.name -eq 'YAML Fixture' -and $yamlNode.proxy.seed -ceq $seed -and $yamlNode.proxy.fingerprint -ceq $pin -and $yamlNode.proxy['client-fingerprint'] -eq 'firefox') 'bounded YAML supports the native adapter fields'
Assert ((Parse (NodeJson @{schema=1;proxy=$uri.proxy}))[0].proxy.fingerprint -ceq $pin) 'a normalized Mycelium envelope can be read again without changing certificate trust'
Assert ((Parse (NodeJson @{proxies=@($basic,$uri.proxy)})).Count -eq 2) 'Mycelium can be imported in a standalone JSON collection'
$encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($link))
Assert ((Parse $encoded)[0].protocol -eq 'mycelium') 'base64 subscriptions also use the bounded Mycelium URI parser'
$coreNodes=New-Object Collections.Generic.List[object]
foreach($mode in @('auto','h3','websocket')){
    $value=$basic.Clone();$value['udp-transport']=$mode.ToUpperInvariant()
    $parsed=(Parse (NodeJson $value))[0].proxy
    Assert ($parsed['udp-transport'] -ceq $mode) 'native UDP selection survives JSON import and normalization'
    $coreNodes.Add($parsed)
    $parsedLink=(Parse ("mycelium://$seed@fixture.invalid:443?udp-transport="+$mode))[0].proxy
    Assert ($parsedLink['udp-transport'] -ceq $mode) 'native UDP selection survives URI import'
}
foreach($badMode in @('quic','h2','h3 ',$true,123,@('h3'))){
    $value=$basic.Clone();$value['udp-transport']=$badMode
    Reject {Parse (NodeJson $value)} 'invalid UDP transport modes are rejected without credential disclosure'
}
foreach($browser in @('chrome','firefox','safari','edge')){
    $value=$basic.Clone();$value['client-fingerprint']=$browser.ToUpperInvariant();$value.fingerprint=$pin.ToUpperInvariant()
    $parsed=(Parse (NodeJson $value))[0].proxy
    Assert ($parsed['client-fingerprint'] -ceq $browser -and $parsed.fingerprint -ceq $pin) 'the fixed browser profiles and hexadecimal pins normalize predictably'
    $coreNodes.Add($parsed)
}
$ipv6Json=$basic.Clone();$ipv6Json.server='[2001:db8::1]'
Assert ((Parse (NodeJson $ipv6Json))[0].proxy.sni -eq '2001:db8::1') 'JSON bracketed IPv6 endpoints normalize before the SNI default'
foreach($modern in @('safari26.3','firefox148-compatible')){
    $value=$basic.Clone();$value['client-fingerprint']=$modern.ToUpperInvariant()
    $parsed=(Parse (NodeJson $value))[0].proxy
    Assert ($parsed['client-fingerprint'] -ceq $modern) 'supported versioned browser profiles normalize without weakening certificate policy'
}

foreach($badSeed in @('',('a'*63),('a'*65),('g'*64),('0'*64),($seed+"`n"),(' '+$seed),([Convert]::ToBase64String((New-Object byte[] 32))),$canary,123,$true,@($seed))){
    $value=$basic.Clone();$value.seed=$badSeed
    Reject {Parse (NodeJson $value)} 'missing, malformed, non-hexadecimal and non-string seeds are rejected without exposing them'
}
$value=$basic.Clone();$value.Remove('seed')
Reject {Parse (NodeJson $value)} 'the credential cannot be omitted'
foreach($field in @('skip-cert-verify','certificate','private-key','tls','alpn','headers','http-opts','ws-opts','path','proxy','dialer-proxy','tfo','mptcp','ip-version','dns','script')){
    $value=$basic.Clone();$value[$field]=$canary
    Reject {Parse (NodeJson $value)} 'untrusted transport fields, certificate bypasses, global directives and paths are rejected'
}
foreach($badPin in @('',('a'*63),('a'*65),('00:'*31),('gg:'*31+'gg'),('sha256:'+$pin),($pin+"`n"),('C:\'+$canary+'.pem'),@($pin),$true)){
    $value=$basic.Clone();$value.fingerprint=$badPin
    Reject {Parse (NodeJson $value)} 'a certificate pin must be exactly one SHA256 digest'
}
foreach($badBrowser in @('random','randomized','none','chrome_psk','360','chrome155','firefox148','safari26.4','',@('chrome'),$true)){
    $value=$basic.Clone();$value['client-fingerprint']=$badBrowser
    Reject {Parse (NodeJson $value)} 'random and unsupported TLS browser profiles are rejected'
}
foreach($badSni in @('',"$canary`n.invalid",'https://fixture.invalid','fixture.invalid/path','[fixture.invalid]','[2001:db8::1]','0.0.0.0','::','fe80::1%1',123,@('fixture.invalid'))){
    $value=$basic.Clone();$value.sni=$badSni
    Reject {Parse (NodeJson $value)} 'the TLS identity must be a DNS name or an unscoped IP address'
}
foreach($badServer in @('[fixture.invalid]','[fixture.invalid','fixture.invalid]','[192.0.2.1]')){
    $value=$basic.Clone();$value.server=$badServer
    Reject {Parse (NodeJson $value)} 'only IPv6 addresses may use server brackets'
}
foreach($badUdp in @('true',1,@($true))){$value=$basic.Clone();$value.udp=$badUdp;Reject {Parse (NodeJson $value)} 'JSON UDP options require a boolean'}
foreach($badLink in @(
    "mycelium://fixture.invalid:443",
    "mycelium://$canary@fixture.invalid:443",
    ('mycelium://'+('0'*64)+'@fixture.invalid:443'),
    "mycelium://${seed}:$canary@fixture.invalid:443",
    "mycelium://$seed@$canary@fixture.invalid:443",
    "mycelium://$seed@fixture.invalid",
    "mycelium://$seed@fixture.invalid:0",
    "mycelium://$seed@fixture.invalid:65536",
    "mycelium://$seed@fixture.invalid:443/",
    "mycelium://$seed@fixture.invalid:443/$canary",
    "mycelium://$seed@[fixture.invalid]:443",
    "mycelium://$seed@2001:db8::1:443",
    "mycelium://$seed@fixture.invalid:443?sni=",
    "mycelium://$seed@fixture.invalid:443?sni=$canary%FF",
    "mycelium://$seed@fixture.invalid:443?sni=fixture.invalid&sni=$canary",
    "mycelium://$seed@fixture.invalid:443?fp=chrome",
    "mycelium://$seed@fixture.invalid:443?client-fingerprint=random",
    "mycelium://$seed@fixture.invalid:443?skip-cert-verify=true",
    "mycelium://$seed@fixture.invalid:443?insecure=true",
    "mycelium://$seed@fixture.invalid:443?certificate=C%3A%5C$canary.pem",
    "mycelium://$seed@fixture.invalid:443?host=$canary",
    "mycelium://$seed@fixture.invalid:443?udp=$canary"
)){Reject {Parse $badLink} 'malformed or unsafe Mycelium links fail without exposing credentials'}
$escapedSeed='%30'+$seed.Substring(1)
Reject {Parse "mycelium://$escapedSeed@fixture.invalid:443"} 'the seed in share-link credentials must be unescaped hexadecimal'
$duplicate='{"type":"mycelium","server":"fixture.invalid","port":443,"seed":"'+$seed+'","SEED":"'+$canary+'"}'
Reject {Parse $duplicate} 'duplicate JSON credential fields fail before normalization'
Reject {Parse (NodeJson @{proxies=@($basic);dns=@{enable=$true;secret=$canary}})} 'global configuration cannot be imported alongside a Mycelium node'

if($CorePath){
    $core=[IO.Path]::GetFullPath($CorePath)
    if(!(Test-Path -LiteralPath $core -PathType Leaf)){throw 'FAIL: supplied core path does not exist'}
    $fixture=Join-Path $PSScriptRoot ('mycelium-import-'+[guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    try{
        $index=0;foreach($proxy in $coreNodes){$proxy.name='Fixture-'+$index;$index++}
        $coreNodes.Add($defaultIdentity.proxy);$coreNodes.Add($ipv6.proxy)
        $coreNodes[$coreNodes.Count-2].name='Default-Identity';$coreNodes[$coreNodes.Count-1].name='IPv6-Identity'
        $config=@{'mixed-port'=0;'external-controller'='';dns=@{enable=$false};proxies=$coreNodes.ToArray();rules=@('MATCH,DIRECT')}
        $path=Join-Path $fixture 'test.json'
        [IO.File]::WriteAllText($path,($config|ConvertTo-Json -Depth 12),(New-Object Text.UTF8Encoding($false)))
        $info=New-Object Diagnostics.ProcessStartInfo
        $info.FileName=$core;$info.Arguments='-t -d "'+$fixture+'" -f "'+$path+'"'
        $info.UseShellExecute=$false;$info.CreateNoWindow=$true
        $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
        $process=New-Object Diagnostics.Process
        try{
            $process.StartInfo=$info
            Assert ($process.Start()) 'the supplied core checker starts in isolation'
            $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
            if(!$process.WaitForExit(20000)){$process.Kill();$process.WaitForExit();throw 'FAIL: core configuration check timed out'}
            # Core logs can contain profile credentials. Consume both streams
            # without including them in exceptions or test output.
            [void]$stdout.GetAwaiter().GetResult();[void]$stderr.GetAwaiter().GetResult()
            Assert ($process.ExitCode -eq 0) 'Mycelium nodes pass the supplied native core configuration checker'
        }finally{$process.Dispose()}
    }finally{
        $resolved=[IO.Path]::GetFullPath($fixture);$prefix=[IO.Path]::GetFullPath($PSScriptRoot)+'\'
        if($resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^mycelium-import-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
    }
}
Write-Host ('PASS: '+$passed+' Mycelium import assertions; no live VPN, TUN, DNS or service changes')
