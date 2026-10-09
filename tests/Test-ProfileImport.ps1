param([switch]$SkipCore,[string]$CorePath,[string]$Base)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=if($Base){[IO.Path]::GetFullPath($Base)}else{Split-Path -Parent $PSScriptRoot}
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$passed=0
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
function Reject([scriptblock]$Action,[string]$Message){$errorText='';try{& $Action|Out-Null}catch{$errorText=$_.Exception.Message};Assert ($errorText.StartsWith('Profile import:',[StringComparison]::Ordinal) -and !$errorText.Contains('PRIVATE-CANARY')) $Message}
function Parse([string]$Text,[string]$Format='auto'){return ,@(Convert-SplitProfileText $Text $Format 'Fixture')}
function B64([string]$Text){return [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Text))}
function Der([byte]$Tag,[byte[]]$Bytes){if($Bytes.Length -gt 127){throw 'Fixture DER unexpectedly large'};return ,([byte[]]@($Tag,[byte]$Bytes.Length)+$Bytes)}
$fixture=Join-Path $PSScriptRoot ('profiles-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$savedAssets=$env:MUKHOMOR_ASSETS;$savedCore=$env:MUKHOMOR_CORE_PATH
try{
    $env:MUKHOMOR_ASSETS=$package
    if(!$CorePath){$CorePath=Get-CorePath $package};$env:MUKHOMOR_CORE_PATH=$CorePath
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
    Initialize-SplitRoot $fixture
    $settings=Get-SplitSettings $fixture;$settings.direct.domain_rule_sets=@();$settings.direct.rule_set_exclusions=@();$settings.dns_update.enabled=$false
    Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
    $uuid='aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
    $ssAuth=B64 'aes-128-gcm:fixture-password'
    $links=@(
        "vless://$uuid@fixture.invalid:443?security=tls&sni=tls.fixture.invalid&type=ws&host=host.fixture.invalid&path=%2Fsocket#VLESS",
        "trojan://p%40ss%3Aword%2Bok@192.0.2.1:443?sni=fixture.invalid#Trojan",
        "ss://$ssAuth@192.0.2.1:8388#SS",
        'hy2://fixture-password@192.0.2.1:443?sni=fixture.invalid&obfs=salamander&obfs-password=fixture-obfs#Hy2',
        "tuic://${uuid}:fixture-password@192.0.2.1:443?congestion_control=bbr&udp_relay_mode=native&alpn=h3#TUIC",
        'hysteria://192.0.2.1:443?auth=fixture-password&upmbps=100&downmbps=100&sni=fixture.invalid#Hy1',
        'anytls://fixture-password@192.0.2.1:443?sni=fixture.invalid#AnyTLS',
        'https://fixture-user:fixture-password@fixture.invalid:443#HTTPS',
        'socks5://fixture-user:fixture-password@192.0.2.1:1080#SOCKS'
    )
    $uris=Parse ($links -join "`n")
    Assert ($uris.Count -eq 9 -and @($uris|Where-Object {$_.proxy.name -ne 'AWG'}).Count -eq 0) 'nine common URI schemes preserve the internal rule alias'
    Assert ($uris[0].proxy.server -eq 'fixture.invalid' -and $uris[0].proxy.servername -eq 'tls.fixture.invalid' -and $uris[0].proxy['ws-opts'].headers.Host -eq 'host.fixture.invalid') 'VLESS preserves endpoint, TLS identity and WS Host independently'
    Assert ($uris[1].proxy.password -ceq 'p@ss:word+ok') 'percent-encoded punctuation and literal plus are preserved in credentials'
    Assert ($uris[7].proxy.type -eq 'http' -and $uris[7].proxy.tls -and !$uris[7].proxy.ContainsKey('udp')) 'HTTPS remains a TCP-only authenticated HTTP proxy'
    $vmess=@{v='2';ps='VMess';add='192.0.2.1';port='443';id=$uuid;aid='0';scy='auto';net='grpc';type='none';path='service';tls='tls';sni='fixture.invalid'}|ConvertTo-Json -Compress
    $vmessNode=(Parse ('vmess://'+(B64 $vmess)))[0]
    Assert ($vmessNode.protocol -eq 'vmess' -and $vmessNode.proxy['grpc-opts']['grpc-service-name'] -eq 'service' -and $vmessNode.proxy.tls) 'base64 VMess JSON preserves gRPC and TLS settings'
    $ssrPayload='192.0.2.1:443:origin:aes-128-cfb:plain:'+(B64 'fixture-password')+'/?remarks='+(B64 'SSR')
    $ssrNode=(Parse ('ssr://'+(B64 $ssrPayload)))[0]
    Assert ($ssrNode.protocol -eq 'ssr' -and $ssrNode.name -eq 'SSR' -and $ssrNode.proxy.password -eq 'fixture-password') 'ShadowsocksR base64 fields and remarks are decoded'
    Assert ((Parse (B64 ($links[0]+"`n"+$links[2]))).Count -eq 2) 'base64 subscriptions produce separate standalone nodes without fetching URLs'
    $legacySs=(Parse ('ss://'+(B64 'aes-128-gcm:p/ss:word@192.0.2.1:8388')))[0]
    Assert ($legacySs.proxy.password -ceq 'p/ss:word') 'legacy Shadowsocks base64 preserves slashes and colons in passwords'
    $yaml=@'
proxies:
  - name: YAML Fixture
    type: vless
    server: fixture.invalid
    port: 443
    uuid: aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa
    tls: true
    servername: tls.fixture.invalid
    network: ws
    ws-opts:
      path: /socket
      headers:
        Host: host.fixture.invalid
'@
    $yamlNode=(Parse $yaml)[0]
    Assert ($yamlNode.proxy['ws-opts'].headers.Host -eq 'host.fixture.invalid') 'strict two-space YAML supports ordinary nested node options'
    $json=@{proxies=@($uris[0].proxy,$uris[2].proxy)}|ConvertTo-Json -Depth 12
    Assert ((Parse $json).Count -eq 2) 'JSON proxies collections import multiple nodes'
    Assert ((Parse (@($uris[0].proxy,$uris[2].proxy)|ConvertTo-Json -Depth 12)).Count -eq 2) 'JSON arrays import multiple nodes'
    Assert ((Parse (@{schema=1;proxy=$uris[0].proxy}|ConvertTo-Json -Depth 12)).Count -eq 1) 'normalized profile envelopes remain readable'
    $wg=@'
[Interface]
PrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=
Address = 10.2.0.2/32
[Peer]
PublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=
Endpoint = 192.0.2.1:51820
AllowedIPs = 0.0.0.0/0
'@
    $wgNode=(Parse $wg)[0]
    Assert ($wgNode.protocol -eq 'wireguard' -and $wgNode.proxy.udp -and $wgNode.proxy['allowed-ips'].Count -eq 2) 'plain WireGuard remains compatible with common split rules'
    Write-AtomicText (Join-Path $fixture 'private\awg.conf') $wg
    Assert ((Get-SelectedSplitProxy $fixture).type -eq 'wireguard') 'legacy active WG files work without a normalized profile'
    Write-AtomicJson (Join-Path $fixture 'private\selected-profile.json') @{schema=1;proxy=$uris[0].proxy}
    $transport=@{server='fixture.invalid';port=443;addresses=@('192.0.2.1','2001:db8::1');selected_address='192.0.2.1';interface_name='Synthetic Interface'}
    $config=New-CoreConfig $fixture $settings $false $transport
    Assert ($config.proxies[0].server -eq 'fixture.invalid' -and $config.proxies[0]['interface-name'] -eq 'Synthetic Interface' -and $config.proxies[0].servername -eq 'tls.fixture.invalid') 'non-WG binding retains original hostname and TLS identity'
    Assert ($config.dns.ipv6 -and $config.tun['route-exclude-address'] -contains '192.0.2.1/32' -and $config.tun['route-exclude-address'] -contains '2001:db8::1/128') 'generic profiles retain IPv6 DNS and exact dual-stack endpoint exclusions'
    Assert ($config.rules -contains 'PROCESS-NAME,qbittorrent.exe,DIRECT' -and $config.rules -contains 'DOMAIN-SUFFIX,ru,DIRECT' -and $config.rules[-1] -eq 'MATCH,AWG') 'all protocol profiles preserve process/domain exceptions and the VPN fallback'
    [IO.File]::Delete((Join-Path $fixture 'private\selected-profile.json'))
    Assert (!(New-CoreConfig $fixture $settings $false).dns.ipv6) 'IPv4-only WireGuard retains its existing DNS behavior'
    foreach($bad in @(
        '{"type":"ss","type":"vless","password":"PRIVATE-CANARY"}',
        '{"type":"vless","server":"192.0.2.1","port":443,"uuid":"PRIVATE-CANARY"}',
        '{"proxies":[],"tun":{"enable":true},"secret":"PRIVATE-CANARY"}',
        '{"type":"ss","server":"192.0.2.1","port":443,"cipher":"aes-128-gcm","password":"PRIVATE-CANARY","dialer-proxy":"other"}',
        '{"type":"ss","server":"192.0.2.1","port":443,"cipher":"aes-128-gcm","password":"PRIVATE-CANARY","plugin":"v2ray-plugin","plugin-opts":{"certificate":"C:\\private.pem"}}',
        "vless://PRIVATE-CANARY@192.0.2.1:443?script=run",
        'trojan://PRIVATE-CANARY%FF@192.0.2.1:443',
        "proxies: &PRIVATE-CANARY`n  - *PRIVATE-CANARY",
        "proxies:`n  - name: PRIVATE-CANARY`n    type: ss`n    type: vless",
        ('['+('['*13)+'"PRIVATE-CANARY"'+(']'*13)+']')
    )){Reject {Parse $bad} 'malformed or unsafe profile input is rejected without leaking input values'}
    $unsafe=$uris[0].proxy.Clone();$unsafe['certificate']='C:\PRIVATE-CANARY.pem'
    Reject {Parse ($unsafe|ConvertTo-Json -Depth 12)} 'TLS certificate file references are rejected'
    $unsafe=$uris[0].proxy.Clone();$unsafe['grpc-opts']=@{'max-streams'='4294967295'}
    Reject {Parse ($unsafe|ConvertTo-Json -Depth 12)} 'numeric strings cannot bypass resource limits'
    $unsafe=$uris[0].proxy.Clone();$unsafe['ws-opts']=@{headers=@{Host="PRIVATE-CANARY`r`nInjected: yes"}}
    Reject {Parse ($unsafe|ConvertTo-Json -Depth 12)} 'nested headers cannot contain injected line breaks'
    Reject {Parse (('x'*524289))} 'oversized input is rejected before parsing'
    $expanded=$uris[0].proxy.Clone();$expanded['ws-opts']=@{headers=@{'X-Fixture'=@(1..120|ForEach-Object {'a'*4350})}}
    $compact=$expanded|ConvertTo-Json -Depth 12 -Compress
    Assert ([Text.Encoding]::UTF8.GetByteCount($compact) -le 524288) 'storage expansion fixture fits the input budget'
    Reject {Parse $compact} 'normalization cannot create a stored profile exceeding the read limit'
    Reject {Parse ('vless://'+$uuid+'@192.0.2.1:0')} 'zero endpoint ports are rejected'
    Reject {Parse ((@{type='ssh';server='192.0.2.1';port=22;username='fixture';password='PRIVATE-CANARY'})|ConvertTo-Json)} 'SSH cannot silently disable server host-key verification'
    Reject {Parse ((@{type='masque';server='192.0.2.1';port=443;'private-key'='PRIVATE-CANARY';'public-key'='fixture';network='h3-l4proxy';udp=$true})|ConvertTo-Json)} 'MASQUE TCP-only L4 mode cannot silently advertise UDP'
    $rsa=New-Object Security.Cryptography.RSACng(2048)
    try{
        $request=New-Object Security.Cryptography.X509Certificates.CertificateRequest('CN=fixture.invalid',$rsa,[Security.Cryptography.HashAlgorithmName]::SHA256,[Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $certificate=$request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-1),[DateTimeOffset]::UtcNow.AddDays(1))
        try{$pem="-----BEGIN CERTIFICATE-----`n"+[Convert]::ToBase64String($certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert),[Base64FormattingOptions]::InsertLineBreaks)+"`n-----END CERTIFICATE-----`n"}finally{$certificate.Dispose()}
        $keyPem="-----BEGIN PRIVATE KEY-----`n"+[Convert]::ToBase64String($rsa.Key.Export([Security.Cryptography.CngKeyBlobFormat]::Pkcs8PrivateBlob),[Base64FormattingOptions]::InsertLineBreaks)+"`n-----END PRIVATE KEY-----`n"
    }finally{$rsa.Dispose()}
    $pem=$pem.Replace("`r`n","`n")
    $ovpn="client`nremote 192.0.2.1 1194`nproto udp`ndev tun`nremote-cert-tls server`nauth-user-pass`n<auth-user-pass>`nfixture-user`nfixture-password`n</auth-user-pass>`n<ca>`n"+$pem+"</ca>`n"
    $ovpnNode=(Parse $ovpn)[0]
    Assert ($ovpnNode.protocol -eq 'openvpn' -and $ovpnNode.proxy.username -eq 'fixture-user' -and $ovpnNode.proxy.ca -eq $pem) 'OpenVPN accepts a single remote and inline certificates and credentials'
    foreach($directive in @('up PRIVATE-CANARY.cmd','ca C:\PRIVATE-CANARY.pem','auth-user-pass C:\PRIVATE-CANARY.txt','verify-x509-name PRIVATE-CANARY name','remote 192.0.2.2 1194')){Reject {Parse ($ovpn+"`n"+$directive)} 'unsupported OpenVPN scripts, external files and security directives fail explicitly'}
    $chain=$ovpnNode.proxy.Clone();$chain.ca=$pem+$pem
    Assert ((Parse ($chain|ConvertTo-Json -Depth 12))[0].proxy.ca -eq $pem+$pem) 'OpenVPN CA chains are preserved as inline certificate blocks'
    $tlsChain=$uris[0].proxy.Clone();$tlsChain.certificate=$pem+$pem;$tlsChain['private-key']=$keyPem
    Assert ((Parse ($tlsChain|ConvertTo-Json -Depth 12))[0].proxy.certificate -eq $pem+$pem) 'TLS certificate chains and inline client keys are preserved'
    $sshBlob=[byte[]]@(0,0,0,11)+[Text.Encoding]::ASCII.GetBytes('ssh-ed25519')+[byte[]]@(0,0,0,32)+(New-Object byte[] 32)
    $hostKey='ssh-ed25519 '+[Convert]::ToBase64String($sshBlob)
    $ec=New-Object Security.Cryptography.ECDsaCng(256)
    try{$parameters=$ec.ExportParameters($true)}finally{$ec.Dispose()}
    $point=[byte[]]@(4)+$parameters.Q.X+$parameters.Q.Y
    $curveOid=Der 6 ([byte[]]@(0x2a,0x86,0x48,0xce,0x3d,3,1,7));$ecOid=Der 6 ([byte[]]@(0x2a,0x86,0x48,0xce,0x3d,2,1))
    $privateDer=Der 0x30 ((Der 2 ([byte[]]@(1)))+(Der 4 $parameters.D)+(Der 0xa0 $curveOid)+(Der 0xa1 (Der 3 ([byte[]]@(0)+$point))))
    $publicDer=Der 0x30 ((Der 0x30 ($ecOid+$curveOid))+(Der 3 ([byte[]]@(0)+$point)))
    $more=@(
        @{type='snell';server='192.0.2.1';port=443;psk='fixture-password';version=4},
        @{type='ssh';server='192.0.2.1';port=22;username='fixture';password='fixture-password';'host-key'=@($hostKey)},
        @{type='mieru';server='192.0.2.1';port=443;username='fixture';password='fixture-password';transport='TCP'},
        @{type='trusttunnel';server='192.0.2.1';port=443;username='fixture';password='fixture-password';sni='fixture.invalid'},
        @{type='shadowquic';server='192.0.2.1';port=443;username='fixture';password='fixture-password';sni='fixture.invalid'},
        @{type='gost-relay';server='192.0.2.1';port=443;username='fixture';password='fixture-password'},
        @{type='sudoku';server='192.0.2.1';port=443;key='fixture-password'},
        @{type='masque';server='192.0.2.1';port=443;'private-key'=[Convert]::ToBase64String($privateDer);'public-key'=[Convert]::ToBase64String($publicDer);ip='10.2.0.2/32';network='h3';sni='fixture.invalid'}
    )
    $additional=Parse ($more|ConvertTo-Json -Depth 12)
    Assert ($additional.Count -eq 8 -and @($additional|Where-Object {$_.protocol -eq 'ssh'})[0].proxy['host-key'][0] -eq $hostKey) 'advanced outgoing JSON nodes preserve protocol credentials and verified SSH identity'
    $all=@($uris)+@($vmessNode,$ssrNode,$wgNode,$ovpnNode)+@($additional)
    Assert (@($all|ForEach-Object {$_.proxy.type}|Sort-Object -Unique).Count -eq 21) 'synthetic fixtures cover every advertised standalone protocol type'
    Assert ((Get-SplitProfileCapabilities|Measure-Object).Count -eq 21) 'capability metadata advertises the implemented standalone types'
    if(!$SkipCore){
        Test-SplitProfileProxies $fixture (@($all|ForEach-Object {$_.proxy})+@($chain,$tlsChain))
        Assert $true 'all twenty-one protocols pass the actual pinned Mihomo -t in one isolated bulk configuration'
        $bad=$uris[2].proxy.Clone();$bad.cipher='PRIVATE-CANARY-invalid-cipher'
        Reject {Test-SplitProfileProxy $fixture $bad} 'real core validation failures do not expose rejected credentials or option values'
        Assert (@(Get-ChildItem -LiteralPath (Join-Path $fixture 'private') -Directory -Filter 'validation-*').Count -eq 0) 'isolated validation always removes its own temporary credential directory'
        $source=Join-Path $fixture 'library-node.json';Write-AtomicJson $source @{schema=1;proxy=$uris[0].proxy}
        Import-SplitProfile $fixture $source
        Assert ((Get-SelectedSplitProxy $fixture).type -eq 'vless' -and !(Test-Path -LiteralPath (Join-Path $fixture 'private\awg.conf'))) 'generic activation supersedes the active legacy file without starting a VPN'
        $before=[IO.File]::ReadAllText((Join-Path $fixture 'private\selected-profile.json'));$beforeConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))
        $badSource=Join-Path $fixture 'invalid-node.json';Write-AtomicJson $badSource @{schema=1;proxy=$bad}
        $failed=$false;try{Import-SplitProfile $fixture $badSource}catch{$failed=$true}
        Assert ($failed -and [IO.File]::ReadAllText((Join-Path $fixture 'private\selected-profile.json')) -eq $before -and [IO.File]::ReadAllText((Join-Path $fixture 'private\config.json')) -eq $beforeConfig) 'failed generic activation restores the previous selection and generated config'
        $beforeSettings=[IO.File]::ReadAllText((Join-Path $fixture 'settings.json'))
        $rollback=& (Get-Module SplitVpn) {
            param($root,$source)
            $original=(Get-Command Test-CoreConfig).ScriptBlock;$script:heldProfile=$null
            try{
                function script:Test-CoreConfig([string]$Root,[string]$Path){
                    # Fault injection at the real validation boundary: the
                    # selected node is already staged, but no engine is started.
                    $script:heldProfile=[IO.File]::Open((Join-Path $Root 'private\selected-profile.json'),[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None)
                    Write-AtomicText (Join-Path $Root 'private\config.json') '{"synthetic_change":true}'
                    Write-AtomicText (Join-Path $Root 'settings.json') '{"synthetic_change":true}'
                    throw 'Profile import: synthetic validation failure.'
                }
                try{Import-SplitProfile $root $source;return @{error='';flag=$false}}catch{return @{error=$_.Exception.Message;flag=($_.Exception.Data.Contains('MukhomorRollbackFailures') -and $_.Exception.Data['MukhomorRollbackFailures'] -contains 'profiles')}}
            }finally{
                if($script:heldProfile){$script:heldProfile.Dispose();$script:heldProfile=$null}
                Set-Item -Path function:script:Test-CoreConfig -Value $original
            }
        } $fixture $source
        Assert ($rollback.error -eq 'Profile import: synthetic validation failure.' -and $rollback.flag) 'a locked first rollback file cannot hide the original failure and is explicitly marked'
        Assert ([IO.File]::ReadAllText((Join-Path $fixture 'private\config.json')) -eq $beforeConfig -and [IO.File]::ReadAllText((Join-Path $fixture 'settings.json')) -eq $beforeSettings) 'rollback continues restoring later config/settings files after the selected file is locked'
        Write-AtomicText (Join-Path $fixture 'private\selected-profile.json') $before
        $legacySource=Join-Path $fixture 'library-legacy.conf';Write-AtomicText $legacySource $wg
        Import-SplitProfile $fixture $legacySource
        Assert ((Get-SelectedSplitProxy $fixture).type -eq 'wireguard' -and [IO.File]::ReadAllText((Join-Path $fixture 'private\awg.conf')) -eq $wg) 'a legacy conf can be activated again after generic profile imports'
    }
    Write-Host ('PASS: '+$passed+' profile import assertions; no live VPN, TUN, DNS or service changes')
}finally{
    $env:MUKHOMOR_ASSETS=$savedAssets;$env:MUKHOMOR_CORE_PATH=$savedCore
    $resolved=[IO.Path]::GetFullPath($fixture);$prefix=[IO.Path]::GetFullPath($PSScriptRoot)+'\'
    if($resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^profiles-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
