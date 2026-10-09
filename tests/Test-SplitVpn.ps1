param([switch]$SkipCore)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$passed=0
function Assert($Condition,[string]$Message) { if (!$Condition) { throw "FAIL: $Message" }; $script:passed++ }
function Assert-Throws([scriptblock]$Action,[string]$Message) {
    $thrown=$false; try { & $Action | Out-Null } catch { $thrown=$true }; Assert $thrown $Message
}
$fixture=Join-Path $PSScriptRoot ('run-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture) | Out-Null
Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
[IO.Directory]::CreateDirectory((Join-Path $fixture 'bin')) | Out-Null
if (!$SkipCore) { Copy-Item -LiteralPath (Get-CorePath $package) -Destination (Join-Path $fixture 'bin\mihomo-windows-amd64-compatible.exe') }
Initialize-SplitRoot $fixture
$copySource=@{empty=@();single=@('8.8.8.8');nested=@(@{addresses=@('1.1.1.1')})}
$copy=Convert-ToMap $copySource
Assert ($copy.empty -is [array] -and $copy.empty.Count -eq 0 -and $copy.single -is [array] -and $copy.single.Count -eq 1 -and $copy.nested -is [array] -and $copy.nested[0].addresses -is [array]) 'settings copy preserves empty, single and nested array shapes'
$roundTrip=Convert-ToMap ($copy|ConvertTo-Json -Depth 10|ConvertFrom-Json)
Assert ($roundTrip.single -is [array] -and $roundTrip.single[0] -eq '8.8.8.8' -and $roundTrip.nested[0].addresses[0] -eq '1.1.1.1') 'DNS address arrays remain arrays after JSON serialization'
$copy.single[0]='9.9.9.9'
Assert ($copySource.single[0] -eq '8.8.8.8') 'normalization owns its copied scalar arrays'
$s=Get-SplitSettings $fixture
$s.dns_update.enabled=$true; $s.direct.domain_rule_sets=@(); $s.direct.rule_set_exclusions=@()
Assert ($s.direct.process_names -contains 'qbittorrent.exe') 'qBittorrent preset'
Assert ($s.direct.process_names -contains 'cs2.exe' -and $s.direct.process_names -contains 'TslGame.exe' -and $s.direct.process_names -contains 'dota2.exe') 'competitive game presets'
Assert ((Normalize-Domain '*.Example.COM.' -Suffix) -eq 'example.com') 'wildcard and case normalization'
Assert ((Normalize-Domain 'xn--p1ai' -Suffix) -eq 'xn--p1ai') 'IDN suffix'
Assert (Test-DirectDomain 'service.ru' $s) '.ru match'
Assert (!(Test-DirectDomain 'service.ru.evil.com' $s)) 'suffix boundary'
Assert-Throws { Normalize-Domain 'https://example.com' } 'URLs rejected'
Assert-Throws { Normalize-Domain 'a,b.com' } 'rule injection rejected'
Assert-Throws { Normalize-Cidr '0.0.0.0/0' } 'all-traffic bypass rejected'
Assert-Throws { Normalize-Cidr '1.2.3/24' } 'abbreviated IPv4 rejected'
Assert ((Normalize-Cidr '8.8.8.8/24') -eq '8.8.8.0/24') 'IPv4 canonical CIDR'
Assert ((Normalize-Cidr '2606:4700::1111/64') -eq '2606:4700::/64') 'IPv6 canonical CIDR'
Assert (!(Test-PublicIp '127.0.0.1') -and !(Test-PublicIp '10.0.0.1') -and !(Test-PublicIp '::ffff:8.8.8.8')) 'DNS fallback rejects local and mapped addresses'
Assert ((Test-PublicIp '8.8.8.8') -and (Test-PublicIp '2606:4700:4700::1111')) 'global addresses accepted'

$request={param($provider,$name,$type)
    if ($provider.Contains('cloudflare')) { throw 'primary API unavailable' }
    if ($type -eq 28) { return @{Status=0} }
    return @{Status=0;Answer=@(@{name='example.com.';type=5;TTL=60;data='edge.example.net.'},@{name='edge.example.net.';type=1;TTL=120;data='8.8.8.8'})}
}
$dns=Resolve-DnsApiHost 'example.com' 64 $request
Assert ($dns.provider -eq 'https://dns.google/resolve') 'API failover'
Assert ($dns.ttl -eq 60 -and $dns.addresses[0] -eq '8.8.8.8') 'CNAME chain TTL and result'
$privateReply={param($p,$n,$t) if ($t -eq 28) {return @{Status=0}}; return @{Status=0;Answer=@(@{name=$n;type=1;TTL=60;data='192.168.1.1'})}}
Assert-Throws { Resolve-DnsApiHost 'example.com' 64 $privateReply } 'private DNS answer rejected'
$unrelatedReply={param($p,$n,$t) return @{Status=0;Answer=@(@{name='unrelated.example';type=$t;TTL=60;data='8.8.8.8'})}}
Assert-Throws { Resolve-DnsApiHost 'example.com' 64 $unrelatedReply } 'unrelated DNS answer not imported'
$loopReply={param($p,$n,$t) return @{Status=0;Answer=@(@{name=$n;type=5;TTL=60;data=$n})}}
Assert-Throws { Resolve-DnsApiHost 'example.com' 64 $loopReply } 'CNAME loop rejected'

$s.direct.domain_suffixes=@('example.com'); $s.direct.domains=@(); $s.dns_update.seed_hosts=@('api.example.com'); $s.dns_update.observe_subdomains=$false
Write-AtomicJson (Join-Path $fixture 'settings.json') $s
$now=[datetime]::Parse('2026-10-07T12:00:00Z').ToUniversalTime()
$good={param($h,$m) return @{addresses=@('8.8.8.8');ttl=100;provider='test'}}
$result=Update-SplitDns $fixture -Force -Resolver $good -Now $now
Assert ($result.hosts -eq 2 -and $result.ip_addresses -eq 1) 'deduplicated refreshed DNS state'
$cache=Read-JsonFile (Join-Path $fixture 'runtime\dns-state.json')
Assert (([datetime]::Parse($cache.hosts['api.example.com'].next_refresh).ToUniversalTime()-$now).TotalSeconds -eq 80) 'TTL scheduling'
$fail={param($h,$m) throw 'API outage'}
$result=Update-SplitDns $fixture -Force -Resolver $fail -Now $now.AddMinutes(5)
Assert ($result.api_errors -eq 2 -and $result.ip_addresses -eq 1) 'API failure preserves last working IP'
$newIp={param($h,$m) return @{addresses=@('1.1.1.1');ttl=300;provider='test'}}
$result=Update-SplitDns $fixture -Force -Resolver $newIp -Now $now.AddMinutes(10)
$provider=Get-Content -LiteralPath (Join-Path $fixture 'runtime\rules\dns-ips.yaml') -Raw
Assert ($provider.Contains('1.1.1.1/32') -and !$provider.Contains('8.8.8.8/32')) 'changed IP replaces old IP'
$result=Update-SplitDns $fixture -Force -Resolver $fail -Now $now.AddDays(2)
Assert ($result.ip_addresses -eq 0) 'stale fallback expires during sustained outage'
$result=Update-SplitDns $fixture -Force -Resolver $good -Now $now
$s.direct.domain_suffixes=@('ru'); $s.dns_update.seed_hosts=@()
Write-AtomicJson (Join-Path $fixture 'settings.json') $s
$result=Update-SplitDns $fixture -Force -Resolver $good -Now $now
Assert ($result.hosts -eq 0 -and $result.ip_addresses -eq 0) 'removing domain exception removes cached IPs'
$lock=[IO.File]::Open((Join-Path $fixture 'runtime\update.lock'),'OpenOrCreate','ReadWrite','None')
try { Assert-Throws { Update-SplitDns $fixture -Force -Resolver $good } 'concurrent writes rejected' } finally { $lock.Dispose() }
$before=Get-Content -LiteralPath (Join-Path $fixture 'runtime\rules\dns-ips.yaml') -Raw
Write-AtomicText (Join-Path $fixture 'runtime\dns-state.json') 'broken json'
Assert-Throws { Update-SplitDns $fixture -Force -Resolver $good } 'corrupt DNS state rejected'
Assert ((Get-Content -LiteralPath (Join-Path $fixture 'runtime\rules\dns-ips.yaml') -Raw) -eq $before) 'corrupt cache does not overwrite working rules'
[IO.File]::Delete((Join-Path $fixture 'runtime\dns-state.json'))

$profile=@'
[Interface]
PrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=
Address = 10.2.0.2/32
MTU = 1280
Jc = 5
Jmin = 40
Jmax = 1000
S1 = 122
S2 = 110
H1 = 3309746145
H2 = 1247832263
H3 = 3073505960
H4 = 1410779371
[Peer]
PublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=
Endpoint = 192.0.2.1:51820
AllowedIPs = 0.0.0.0/0, ::/0
PersistentKeepalive = 25
'@
Write-AtomicText (Join-Path $fixture 'private\awg.conf') $profile
$proxy=Convert-AwgProfile (Join-Path $fixture 'private\awg.conf')
Assert ($proxy['amnezia-wg-option'].h1 -eq '3309746145' -and $proxy.mtu -eq 1280) 'AWG unsigned header and MTU retained'
$config=New-CoreConfig $fixture $s $false
$knownRule=[array]::IndexOf($config.rules,'DOMAIN-REGEX,.+,AWG')
$fallbackRule=[array]::IndexOf($config.rules,'RULE-SET,dns-ips,DIRECT,no-resolve')
Assert ($knownRule -lt $fallbackRule) 'known-domain guard precedes shared-IP fallback'
Assert (!(@($config.rules | Where-Object { $_ -match '^NETWORK,udp,DIRECT' }).Count)) 'UDP is not bypassed globally'
Assert ($config.tun['route-exclude-address'] -contains '192.0.2.1/32' -and $config.rules[0] -eq 'IP-CIDR,192.0.2.1/32,DIRECT,no-resolve') 'outer VPN endpoint is excluded from TUN and rule recursion'
$transport=@{server='192.0.2.1';port=51820;addresses=@('192.0.2.1');selected_address='192.0.2.1';interface_name='Fixture Ethernet'}
$bound=New-CoreConfig $fixture $s $false $transport
Assert ($bound.proxies[0]['interface-name'] -eq 'Fixture Ethernet') 'outer AWG UDP socket is bound to the selected ordinary interface'
Assert ($bound.tun.stack -eq 'system' -and $bound.tun['udp-timeout'] -eq 300) 'TUN uses the Windows system stack without shortening UDP session lifetime'
Write-AtomicJson (Join-Path $fixture 'runtime\session.json') @{transport=$transport}
$reloaded=New-CoreConfig $fixture $s $true
Assert ($reloaded.proxies[0]['interface-name'] -eq 'Fixture Ethernet') 'live settings reload retains the protected transport binding'
[IO.File]::Delete((Join-Path $fixture 'runtime\session.json'))
$stale=$transport.Clone(); $stale.server='old.example'
$unbound=New-CoreConfig $fixture $s $false $stale
Assert (!$unbound.proxies[0].ContainsKey('interface-name')) 'a changed VPN profile does not reuse stale transport binding'
Assert ((@(Get-EndpointPrefixes @('2001:db8::1','192.0.2.1','192.0.2.1'))) -join ',' -eq '192.0.2.1/32,2001:db8::1/128') 'endpoint exclusions are exact, dual-stack and deduplicated'
Assert-Throws { Get-EndpointPrefixes @('0.0.0.0') } 'wildcard endpoint cannot bypass all traffic'
Assert (!$config.tun.enable -and $config['external-controller'].StartsWith('127.0.0.1:') -and $config.secret.Length -gt 32) 'isolated proxy mode and authenticated controller'
if (!$SkipCore) {
    Set-SplitConfig $fixture $s $false
    $original=Get-Content -LiteralPath (Join-Path $fixture 'private\config.json') -Raw
    $bad=Get-SplitSettings $fixture; $bad.direct.domains=@('invalid,REJECT')
    Assert-Throws { Set-SplitConfig $fixture $bad $false } 'invalid settings not applied'
    Assert ((Get-Content -LiteralPath (Join-Path $fixture 'private\config.json') -Raw) -eq $original) 'failed apply preserves existing config'
    Assert (!(@(Get-ChildItem -LiteralPath (Join-Path $fixture 'private') -Filter '*.tmp').Count)) 'atomic update leaves no temp files'
}
'PASS: '+$passed+' assertions; fixture '+$fixture
