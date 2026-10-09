# Requires Windows PowerShell 5.1. All changes stay in this package except
# the explicitly requested TUN session and its reversible DNS setting.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ProfileImport.psm1') -Force -DisableNameChecking
$script:DomainIdnMapping=New-Object Globalization.IdnMapping

function Read-JsonFile([string]$Path) {
    if (!(Test-Path -LiteralPath $Path)) { throw "File not found: $Path" }
    return Convert-ToMap (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json)
}

function Convert-ToMap($Value) {
    if ($null -eq $Value) { return $null }
    if ($Value -is [string] -or $Value -is [ValueType]) { return $Value }
    if ($Value -is [System.Collections.IDictionary]) {
        $out = @{}; foreach ($key in $Value.Keys) { $out[$key] = Convert-ToMap $Value[$key] }; return $out
    }
    if ($Value.GetType().FullName -eq 'System.Management.Automation.PSCustomObject') {
        $out = @{}; foreach ($p in $Value.PSObject.Properties) { $out[$p.Name] = Convert-ToMap $p.Value }; return $out
    }
    if ($Value -is [array]) {
        # Exception lists are scalar arrays. Preserve their shape and ownership
        # without starting a pipeline and calling this function for every string.
        $out=[object[]]::new($Value.Count)
        for ($i=0;$i -lt $Value.Count;$i++) {
            $item=$Value[$i]
            if ($null -eq $item -or $item -is [string] -or $item -is [ValueType]) { $out[$i]=$item }
            else { $out[$i]=Convert-ToMap $item }
        }
        return ,$out
    }
    return $Value
}

function Write-AtomicText([string]$Path, [string]$Text) {
    $parent = Split-Path -Parent $Path
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $temp = Join-Path $parent ([IO.Path]::GetFileName($Path) + '.' + [guid]::NewGuid().ToString('N') + '.tmp')
    $backup = $temp + '.bak'
    try {
        [IO.File]::WriteAllText($temp, $Text, (New-Object Text.UTF8Encoding($false)))
        if ([IO.File]::Exists($Path)) { [IO.File]::Replace($temp, $Path, $backup, $true) }
        else { [IO.File]::Move($temp, $Path) }
    } finally {
        if ([IO.File]::Exists($temp)) { [IO.File]::Delete($temp) }
        if ([IO.File]::Exists($backup)) { [IO.File]::Delete($backup) }
    }
}

function Write-AtomicJson([string]$Path, $Value) {
    Write-AtomicText $Path ($Value | ConvertTo-Json -Depth 40)
}

function Initialize-SplitRoot([string]$Root) {
    foreach ($dir in @('private', 'runtime', 'runtime\rules', 'runtime\rules\services', 'runtime\logs')) {
        [IO.Directory]::CreateDirectory((Join-Path $Root $dir)) | Out-Null
    }
    # Private files contain the VPN keys and API token. Do not print them.
    $private = Join-Path $Root 'private'
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    foreach ($principal in @($sid, (New-Object Security.Principal.SecurityIdentifier('S-1-5-18')), (New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))) {
        $rule = New-Object Security.AccessControl.FileSystemAccessRule($principal, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    if (!(Get-Acl -LiteralPath $private).AreAccessRulesProtected) { Set-Acl -LiteralPath $private -AclObject $acl }
    $tokenPath = Join-Path $private 'api-token.txt'
    if (!(Test-Path -LiteralPath $tokenPath)) {
        $bytes = New-Object byte[] 32
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
        Write-AtomicText $tokenPath ([Convert]::ToBase64String($bytes))
    }
    $provider = Join-Path $Root 'runtime\rules\dns-ips.yaml'
    if (!(Test-Path -LiteralPath $provider)) { Write-AtomicText $provider "payload: []`n" }
    # Mihomo owns subsequent HTTP-provider updates. Seeds allow an offline start.
    foreach ($name in (Get-DomainRuleSetCatalog).Keys) {
        $seedRoot=if ($env:MUKHOMOR_ASSETS) { $env:MUKHOMOR_ASSETS } else { $Root }
        $seed = Join-Path $seedRoot "lists\seed-$name.list"
        $cache = Join-Path $Root "runtime\rules\services\$name.list"
        if ((Test-Path -LiteralPath $seed) -and !(Test-Path -LiteralPath $cache)) {
            [IO.File]::Copy($seed, $cache, $false)
        }
    }
}

function Invoke-SplitLock([string]$Root, [scriptblock]$Action) {
    $stream = $null
    try {
        $stream = [IO.File]::Open((Join-Path $Root 'runtime\update.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
        & $Action
    } finally { if ($stream) { $stream.Dispose() } }
}

function Normalize-Domain([string]$Value, [switch]$Suffix) {
    $value = $Value.Trim().TrimEnd('.').ToLowerInvariant()
    if ($Suffix) { $value = $value -replace '^(\*\.|\+\.|\.)', '' }
    if (!$value -or $value -match '[/:,\s*+]' -or $value.Length -gt 253) { throw "Invalid domain: $Value" }
    try { $value = $script:DomainIdnMapping.GetAscii($value) } catch { throw "Invalid domain: $Value" }
    $parsed = $null
    if ([Net.IPAddress]::TryParse($value, [ref]$parsed)) { throw "Use the IP list for IP addresses: $Value" }
    foreach ($label in $value.Split('.')) {
        if ($label.Length -gt 63 -or $label -notmatch '^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$') { throw "Invalid domain: $Value" }
    }
    if (!$Suffix -and !$value.Contains('.')) { throw "Expected a full domain: $Value" }
    return $value
}

function Normalize-Cidr([string]$Value) {
    $parts = $Value.Trim().Split('/')
    if ($parts.Count -gt 2) { throw "Invalid IP/CIDR: $Value" }
    $ip = $null
    if (![Net.IPAddress]::TryParse($parts[0], [ref]$ip) -or $parts[0] -match '%' -or ($ip.AddressFamily -eq 'InterNetwork' -and $parts[0] -notmatch '^\d+\.\d+\.\d+\.\d+$')) { throw "Invalid IP: $Value" }
    $width = if ($ip.AddressFamily -eq 'InterNetwork') { 32 } else { 128 }
    $prefix = $width
    if ($parts.Count -eq 2) {
        if ($parts[1] -notmatch '^\d+$') { throw "Invalid prefix: $Value" }
        $prefix = [int]$parts[1]
    }
    if ($prefix -lt 1 -or $prefix -gt $width) { throw "Prefix would bypass all traffic or is invalid: $Value" }
    $bytes = $ip.GetAddressBytes()
    for ($i=0; $i -lt $bytes.Length; $i++) {
        $bits = [Math]::Max(0, [Math]::Min(8, $prefix - 8*$i))
        $mask = if ($bits -eq 0) { 0 } else { (255 -shl (8-$bits)) -band 255 }
        $bytes[$i] = $bytes[$i] -band $mask
    }
    $network = New-Object Net.IPAddress(,$bytes)
    return $network.ToString() + '/' + $prefix
}

function Get-CidrSortKey([string]$Cidr) {
    $parts=$Cidr.Split('/'); $bytes=[Net.IPAddress]::Parse($parts[0]).GetAddressBytes()
    return $bytes.Length.ToString('D2') + ':' + (($bytes | ForEach-Object { $_.ToString('D3') }) -join '.') + '/' + ([int]$parts[1]).ToString('D3')
}

function Get-DomainRuleSetCatalog {
    return @{
        russia=@{url='https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/meta/geo/geosite/category-ru.list'}
        ozon=@{url='https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/meta/geo/geosite/ozon.list'}
        steam=@{url='https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/meta/geo/geosite/steam.list'}
    }
}

function Test-PublicIp([string]$Value) {
    $ip = $null
    if (![Net.IPAddress]::TryParse($Value, [ref]$ip) -or $Value -match '%') { return $false }
    $b = $ip.GetAddressBytes()
    if ($ip.AddressFamily -eq 'InterNetwork') {
        return !($b[0] -eq 0 -or $b[0] -eq 10 -or $b[0] -eq 127 -or $b[0] -ge 224 -or
            ($b[0] -eq 169 -and $b[1] -eq 254) -or ($b[0] -eq 172 -and $b[1] -ge 16 -and $b[1] -le 31) -or
            ($b[0] -eq 192 -and $b[1] -eq 168) -or ($b[0] -eq 100 -and $b[1] -ge 64 -and $b[1] -le 127) -or
            ($b[0] -eq 198 -and $b[1] -in @(18,19)) -or ($b[0] -eq 192 -and $b[1] -eq 0) -or
            ($b[0] -eq 198 -and $b[1] -eq 51 -and $b[2] -eq 100) -or ($b[0] -eq 203 -and $b[1] -eq 0 -and $b[2] -eq 113))
    }
    # Only global unicast; reject mapped IPv4, local/link-local and documentation.
    return (($b[0] -band 224) -eq 32 -and !($b[0] -eq 32 -and $b[1] -eq 1 -and $b[2] -eq 13 -and $b[3] -eq 184))
}

function Get-SplitSettings([string]$Root, $InputSettings = $null) {
    $s = if ($null -eq $InputSettings) { Read-JsonFile (Join-Path $Root 'settings.json') } else { Convert-ToMap $InputSettings }
    if ($s.schema -ne 1) { throw 'Unsupported settings schema' }
    # Backwards-compatible: old configurations have no remote domain lists.
    if (!$s.direct.ContainsKey('domain_rule_sets')) { $s.direct.domain_rule_sets=@() }
    if (!$s.direct.ContainsKey('rule_set_exclusions')) { $s.direct.rule_set_exclusions=@() }
    if ($s.direct.rule_set_exclusions -isnot [array] -or $s.direct.rule_set_exclusions.Count -gt 100) { throw 'direct.rule_set_exclusions must be an array of at most 100 domains' }
    $s.direct.rule_set_exclusions=@($s.direct.rule_set_exclusions | ForEach-Object {Normalize-Domain $_ -Suffix} | Sort-Object -Unique)
    if ($s.direct.domain_rule_sets -isnot [array]) { throw 'direct.domain_rule_sets must be an array' }
    $catalog=Get-DomainRuleSetCatalog
    $s.direct.domain_rule_sets=@($s.direct.domain_rule_sets | ForEach-Object {
        $name=([string]$_).Trim().ToLowerInvariant()
        if (!$catalog.ContainsKey($name)) { throw "Unknown domain rule set: $name" }; $name
    } | Sort-Object -Unique)
    foreach ($key in @('process_names','process_paths','domains','domain_suffixes','ip_cidrs')) {
        if (!$s.direct.ContainsKey($key) -or $s.direct[$key] -isnot [array]) { throw "direct.$key must be an array" }
        if ($s.direct[$key].Count -gt 5000) { throw 'Exception list exceeds 5000 entries' }
    }
    $s.direct.process_names = @($s.direct.process_names | ForEach-Object {
        if ($_ -notmatch '^[^\\/:,*?\r\n]+\.exe$') { throw "Invalid executable name: $_" }; $_.Trim()
    } | Sort-Object -Unique)
    $s.direct.process_paths = @($s.direct.process_paths | ForEach-Object {
        if ($_ -match '[,\r\n*?]' -or $_ -notmatch '^[A-Za-z]:\\.+\.exe$') { throw "Expected an absolute EXE path: $_" }; $_.Trim()
    } | Sort-Object -Unique)
    $s.direct.domains = @($s.direct.domains | ForEach-Object { Normalize-Domain $_ } | Sort-Object -Unique)
    $s.direct.domain_suffixes = @($s.direct.domain_suffixes | ForEach-Object { Normalize-Domain $_ -Suffix } | Sort-Object -Unique)
    $s.direct.ip_cidrs = @($s.direct.ip_cidrs | ForEach-Object { Normalize-Cidr $_ } | Sort-Object -Unique | Sort-Object { Get-CidrSortKey $_ })
    $s.dns_update.seed_hosts = @($s.dns_update.seed_hosts | ForEach-Object { Normalize-Domain $_ } | Sort-Object -Unique)
    foreach ($seed in $s.dns_update.seed_hosts) { if (!(Test-DirectDomain $seed $s)) { throw "DNS seed must match a domain exception: $seed" } }
    foreach ($pair in @(@('min_refresh_seconds',15,3600), @('max_refresh_seconds',30,86400), @('max_stale_seconds',60,604800), @('max_hosts',1,1000), @('max_addresses_per_host',1,256))) {
        $key=$pair[0]; if ($s.dns_update[$key] -isnot [int] -or $s.dns_update[$key] -lt $pair[1] -or $s.dns_update[$key] -gt $pair[2]) { throw "Invalid dns_update.$key" }
    }
    if ($s.dns_update.min_refresh_seconds -gt $s.dns_update.max_refresh_seconds) { throw 'Minimum refresh exceeds maximum' }
    foreach ($key in @('enabled','observe_subdomains')) { if ($s.dns_update[$key] -isnot [bool]) { throw "dns_update.$key must be boolean" } }
    foreach ($key in @('proxy','controller','dns')) { if ($s.ports[$key] -isnot [int] -or $s.ports[$key] -lt 1024 -or $s.ports[$key] -gt 65535) { throw "Invalid port: $key" } }
    if (@($s.ports.Values | Sort-Object -Unique).Count -ne 3) { throw 'Ports must be distinct' }
    return $s
}

function Test-DirectDomain([string]$HostName, $Settings) {
    try { $hostName = Normalize-Domain $HostName } catch { return $false }
    if ($hostName -in $Settings.direct.domains) { return $true }
    foreach ($suffix in $Settings.direct.domain_suffixes) {
        if ($hostName -eq $suffix -or $hostName.EndsWith('.' + $suffix, [StringComparison]::Ordinal)) { return $true }
    }
    return $false
}

function Convert-AwgProfile([string]$Path) {
    if((Get-Item -LiteralPath $Path).Length -gt 524288){throw 'Profile import: size limit exceeded.'}
    return @(Convert-SplitProfileText ([IO.File]::ReadAllText($Path)) 'conf')[0].proxy
}

function Get-SelectedSplitProxy([string]$Root) {
    $selected=Join-Path $Root 'private\selected-profile.json'
    if(Test-Path -LiteralPath $selected){
        if((Get-Item -LiteralPath $selected).Length -gt 524288){throw 'Profile import: size limit exceeded.'}
        $nodes=@(Convert-SplitProfileText ([IO.File]::ReadAllText($selected)) 'node')
        if($nodes.Count -ne 1){throw 'Profile import: active profile must contain one outgoing node.'}
        return $nodes[0].proxy
    }
    return Convert-AwgProfile (Join-Path $Root 'private\awg.conf')
}

function Test-SplitProfileProxies([string]$Root,[object[]]$Proxies) {
    if(!$Proxies -or $Proxies.Count -gt 128){throw 'Profile import: node count must be between 1 and 128.'}
    $nodes=@();$n=0
    foreach($value in $Proxies){$proxy=Convert-SplitNode $value;$proxy.name='Validation-'+($n++);$nodes+=@($proxy)}
    $parent=Join-Path $Root 'private';[IO.Directory]::CreateDirectory($parent)|Out-Null
    $temp=Join-Path $parent ('validation-'+[guid]::NewGuid().ToString('N'));[IO.Directory]::CreateDirectory($temp)|Out-Null
    try{
        $path=Join-Path $temp 'config.json'
        Write-AtomicJson $path @{proxies=$nodes;rules=@('MATCH,DIRECT');mode='rule';'mixed-port'=0;'external-controller'='';'allow-lan'=$false;'log-level'='silent';'geo-auto-update'=$false;dns=@{enable=$false};tun=@{enable=$false}}
        $result=Invoke-SplitBoundedProcess (Get-CorePath $Root) @('-t','-d',$temp,'-f',$path) 5000 'Profile validation'
        if($result.exit_code -ne 0){throw 'Profile import: Mihomo rejected incompatible protocol options.'}
    }catch{
        if($_.Exception.Message.StartsWith('Profile import:',[StringComparison]::Ordinal)){throw}
        throw 'Profile import: protocol validation failed or timed out.'
    }finally{
        $resolved=[IO.Path]::GetFullPath($temp);$prefix=[IO.Path]::GetFullPath($parent)+'\'
        if($resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^validation-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
    }
}

function Test-SplitProfileProxy([string]$Root,$Proxy) {
    Test-SplitProfileProxies $Root @($Proxy)
}

function Convert-SplitProgramPath([string]$Path) {
    # Windows filesystem APIs accept mixed separators and extended namespaces;
    # Windows Firewall application filters require an ordinary Win32 path.
    $path=$Path.Replace('/','\')
    if ($path.StartsWith('\\?\UNC\',[StringComparison]::OrdinalIgnoreCase)) { $path='\\'+$path.Substring(8) }
    elseif ($path -match '^\\\\\?\\[A-Za-z]:\\') { $path=$path.Substring(4) }
    elseif ($path.StartsWith('\\?\',[StringComparison]::OrdinalIgnoreCase) -or $path.StartsWith('\\.\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsupported executable path namespace' }
    return [IO.Path]::GetFullPath($path)
}

function Get-CorePath([string]$Root) {
    $exe=if ($env:MUKHOMOR_CORE_PATH) { $env:MUKHOMOR_CORE_PATH } else { Join-Path $Root 'bin\mihomo-windows-amd64-compatible.exe' }
    $exe=Convert-SplitProgramPath $exe
    if (!(Test-Path -LiteralPath $exe)) { throw 'Mihomo executable is missing' }
    return $exe
}

function Get-EndpointPrefixes([string[]]$Addresses) {
    foreach ($address in @($Addresses | Sort-Object -Unique)) {
        $ip=$null
        if (![Net.IPAddress]::TryParse($address,[ref]$ip) -or $ip.Equals([Net.IPAddress]::Any) -or $ip.Equals([Net.IPAddress]::IPv6Any)) { throw 'Invalid VPN transport address' }
        $bits=if ($ip.AddressFamily -eq 'InterNetwork') { 32 } else { 128 }
        $ip.ToString()+"/$bits"
    }
}

function New-CoreConfig([string]$Root, $Settings, [bool]$TunEnabled=$true, $Transport=$null) {
    New-ValidatedCoreConfig $Root (Get-SplitSettings $Root $Settings) $TunEnabled $Transport
}

# Module-private builder: both public entry points validate once before using it.
function New-ValidatedCoreConfig([string]$Root, $s, [bool]$TunEnabled, $Transport) {
    $proxy=Get-SelectedSplitProxy $Root
    # The outer AWG socket must never be routed back into its own TUN.
    # Only the VPN endpoint is excluded here; service IPs remain domain driven.
    if (!$Transport) {
        $sessionPath=Join-Path $Root 'runtime\session.json'
        if (Test-Path -LiteralPath $sessionPath) {
            $session=Read-JsonFile $sessionPath
            if ($session.ContainsKey('transport')) { $Transport=$session.transport }
        }
    }
    $endpointAddresses=@(); $literal=$null
    if ([Net.IPAddress]::TryParse($proxy.server,[ref]$literal)) { $endpointAddresses=@($literal.ToString()) }
    if ($Transport -and $Transport.server -eq $proxy.server -and $Transport.port -eq $proxy.port) {
        $endpointAddresses=@($Transport.addresses)
        if ($endpointAddresses -notcontains $Transport.selected_address) { throw 'VPN transport address is not in the endpoint set' }
        # Preserve the hostname for native TLS/SNI/transport Host defaults.
        if($proxy.type -eq 'wireguard'){$proxy.server=$Transport.selected_address}
        $proxy['interface-name']=$Transport.interface_name
    }
    $endpointPrefixes=@(Get-EndpointPrefixes $endpointAddresses)
    $rules=New-Object 'Collections.Generic.List[string]'
    foreach ($cidr in $endpointPrefixes) {
        $kind=if ($cidr.Contains(':')) { 'IP-CIDR6' } else { 'IP-CIDR' }
        $rules.Add("$kind,$cidr,DIRECT,no-resolve")
    }
    foreach ($name in $s.direct.process_names) { $rules.Add("PROCESS-NAME,$name,DIRECT") }
    foreach ($path in $s.direct.process_paths) { $rules.Add("PROCESS-PATH,$path,DIRECT") }
    $providers=@{}
    # Compile larger custom lists into the core's immutable domain set. Exact
    # entries stay exact; +. includes both a suffix and its subdomains. Keep the
    # group at its original priority, ahead of rule-set exclusions and IP rules.
    # Inline payloads are part of the validated candidate, so a failed reload
    # cannot leave separately written provider files in a partially applied state.
    if (($s.direct.domains.Count+$s.direct.domain_suffixes.Count) -ge 32) {
        $payload=@($s.direct.domains)+@($s.direct.domain_suffixes | ForEach-Object {'+.'+$_})
        $providers['custom-domains']=@{type='inline';behavior='domain';payload=$payload}
        $rules.Add('RULE-SET,custom-domains,DIRECT')
    } else {
        foreach ($d in $s.direct.domains) { $rules.Add("DOMAIN,$d,DIRECT") }
        foreach ($d in $s.direct.domain_suffixes) { $rules.Add("DOMAIN-SUFFIX,$d,DIRECT") }
    }
    if ($s.dns_update.enabled) { $providers['dns-ips']=@{type='file';behavior='ipcidr';format='yaml';path='./runtime/rules/dns-ips.yaml';interval=60} }
    $catalog=Get-DomainRuleSetCatalog
    if ($s.direct.domain_rule_sets.Count) {
        foreach ($d in $s.direct.rule_set_exclusions) { $rules.Add("DOMAIN-SUFFIX,$d,AWG") }
    }
    foreach ($name in $s.direct.domain_rule_sets) {
        $rules.Add("RULE-SET,service-$name,DIRECT")
        $providers["service-$name"]=@{type='http';behavior='domain';format='text';url=$catalog[$name].url;path="./runtime/rules/services/$name.list";interval=21600;proxy='AWG';'size-limit'=1048576}
    }
    foreach ($cidr in @('127.0.0.0/8','10.0.0.0/8','172.16.0.0/12','192.168.0.0/16','169.254.0.0/16','100.64.0.0/10','::1/128','fc00::/7','fe80::/10')) {
        $kind=if ($cidr.Contains(':')) { 'IP-CIDR6' } else { 'IP-CIDR' }; $rules.Add("$kind,$cidr,DIRECT,no-resolve")
    }
    if ($s.direct.ip_cidrs.Count -ge 32) {
        # The native merged IP set supports IPv4 and IPv6 together. Deliberately
        # retain resolution here: explicit IP exceptions also apply to domains.
        $providers['custom-ips']=@{type='inline';behavior='ipcidr';payload=@($s.direct.ip_cidrs)}
        $rules.Add('RULE-SET,custom-ips,DIRECT')
    } else {
        foreach ($cidr in $s.direct.ip_cidrs) {
            $kind=if ($cidr.Contains(':')) { 'IP-CIDR6' } else { 'IP-CIDR' }; $rules.Add("$kind,$cidr,DIRECT")
        }
    }
    # Protect known unrelated domains on shared CDN IPs from the DNS-IP fallback.
    $rules.Add('DOMAIN-REGEX,.+,AWG')
    if ($s.dns_update.enabled) { $rules.Add('RULE-SET,dns-ips,DIRECT,no-resolve') }
    $rules.Add('MATCH,AWG')
    $dns=@{enable=$true;listen="127.0.0.1:$($s.ports.dns)";ipv6=($proxy.type -ne 'wireguard' -or $proxy.ContainsKey('ipv6'));'enhanced-mode'='fake-ip';'fake-ip-range'='198.18.0.1/16';'fake-ip-filter'=@('*.lan','*.local','localhost');'default-nameserver'=@('1.1.1.1','8.8.8.8');nameserver=@('https://cloudflare-dns.com/dns-query','https://dns.google/dns-query');'proxy-server-nameserver'=@('https://cloudflare-dns.com/dns-query#DIRECT','https://dns.google/dns-query#DIRECT');'direct-nameserver'=@('https://cloudflare-dns.com/dns-query#DIRECT','https://dns.google/dns-query#DIRECT');'respect-rules'=$true}
    return @{
        'mixed-port'=$s.ports.proxy;'allow-lan'=$false;'bind-address'='127.0.0.1';mode='rule';'log-level'='warning';ipv6=$true
        # Socket-table enumeration is unnecessary when there are no EXE rules.
        # With EXE rules, strict performs the same lookup when their turn arrives.
        'find-process-mode'=$(if ($s.direct.process_names.Count -or $s.direct.process_paths.Count) {'strict'} else {'off'});'external-controller'="127.0.0.1:$($s.ports.controller)";secret=(Get-Content -LiteralPath (Join-Path $Root 'private\api-token.txt') -Raw).Trim()
        'geo-auto-update'=$false;profile=@{'store-fake-ip'=$true};proxies=@($proxy);rules=@($rules.ToArray());dns=$dns
        tun=@{enable=$TunEnabled;stack='system';device='Mukhomor';'auto-route'=$true;'auto-detect-interface'=$true;'strict-route'=$true;'dns-hijack'=@('any:53','tcp://any:53');mtu=1500;'inet6-address'=@('fdfe:dcba:9876::1/126');'route-exclude-address'=$endpointPrefixes;'udp-timeout'=300}
        sniffer=@{enable=$true;'force-dns-mapping'=$true;'parse-pure-ip'=$true;'override-destination'=$false;sniff=@{HTTP=@{ports=@(80,'8080-8880')};TLS=@{ports=@(443,8443)};QUIC=@{ports=@(443,8443)}}}
        'rule-providers'=$providers
    }
}

function Invoke-SplitBoundedProcess([string]$FilePath, [string[]]$Arguments, [int]$TimeoutMilliseconds=5000, [string]$Purpose='Background command') {
    if ($TimeoutMilliseconds -lt 1 -or $TimeoutMilliseconds -gt 15000) { throw 'Invalid background command deadline' }
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=$FilePath
    # Windows PowerShell 5.1 has no ProcessStartInfo.ArgumentList. Quote each
    # argument using the Windows argv convention, never through a shell.
    $info.Arguments=(@($Arguments | ForEach-Object {
        '"'+([regex]::Replace([regex]::Replace([string]$_,'(\\*)"','$1$1\"'),'(\\+)$','$1$1'))+'"'
    }) -join ' ')
    $info.UseShellExecute=$false; $info.CreateNoWindow=$true
    $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
    $process=New-Object Diagnostics.Process
    $process.StartInfo=$info
    try {
        if (!$process.Start()) { throw "$Purpose could not be started" }
        $stdout=$process.StandardOutput.ReadToEndAsync()
        $stderr=$process.StandardError.ReadToEndAsync()
        if (!$process.WaitForExit($TimeoutMilliseconds)) { throw "$Purpose timed out" }
        if (!$stdout.Wait(1000) -or !$stderr.Wait(1000)) { throw "$Purpose output timed out" }
        return @{exit_code=$process.ExitCode;stdout=$stdout.Result;stderr=$stderr.Result}
    } finally {
        try { if (!$process.HasExited) { $process.Kill(); $process.WaitForExit(1000) | Out-Null } } catch {}
        $process.Dispose()
    }
}

function Test-CoreConfig([string]$Root, [string]$Path) {
    $result=Invoke-SplitBoundedProcess (Get-CorePath $Root) @('-t','-d',$Root,'-f',$Path) 5000 'Mihomo configuration validation'
    if ($result.exit_code -ne 0) { throw ('Mihomo rejected the configuration: ' + $result.stdout + ' ' + $result.stderr) }
}

function Invoke-CoreApi([string]$Root, [string]$Route, [string]$Method='GET', $Body=$null, [int]$TimeoutSeconds=10) {
    if ($TimeoutSeconds -lt 1 -or $TimeoutSeconds -gt 10) { throw 'Invalid controller request timeout' }
    # Settings are validated on import/apply. API probes only need the controller
    # port; normalizing thousands of exception entries for every probe, provider
    # update and DNS observation needlessly competes with the VPN for CPU.
    $s=[IO.File]::ReadAllText((Join-Path $Root 'settings.json'),[Text.Encoding]::UTF8) | ConvertFrom-Json
    if ($s.schema -ne 1 -or $s.ports.controller -isnot [int] -or $s.ports.controller -lt 1024 -or $s.ports.controller -gt 65535) { throw 'Invalid controller settings' }
    $token=(Get-Content -LiteralPath (Join-Path $Root 'private\api-token.txt') -Raw).Trim()
    $params=@{Uri="http://127.0.0.1:$($s.ports.controller)$Route";Method=$Method;Headers=@{Authorization="Bearer $token"};TimeoutSec=$TimeoutSeconds;UseBasicParsing=$true}
    if ($null -ne $Body) { $params.Body=($Body | ConvertTo-Json -Depth 40); $params.ContentType='application/json' }
    return Invoke-RestMethod @params
}

function Set-SplitConfig([string]$Root, $Settings, [bool]$TunEnabled=$true, [switch]$Reload, $Transport=$null, [switch]$PassThru) {
    Initialize-SplitRoot $Root
    Invoke-SplitLock $Root {
        $normalized=Get-SplitSettings $Root $Settings
        $candidate=Join-Path $Root 'private\candidate.json'
        $live=Join-Path $Root 'private\config.json'
        Write-AtomicJson $candidate (New-ValidatedCoreConfig $Root $normalized $TunEnabled $Transport)
        Test-CoreConfig $Root $candidate
        $old=if (Test-Path -LiteralPath $live) { [IO.File]::ReadAllText($live) } else { $null }
        $oldSettings=[IO.File]::ReadAllText((Join-Path $Root 'settings.json'))
        try {
            Write-AtomicJson (Join-Path $Root 'settings.json') $normalized
            Write-AtomicText $live ([IO.File]::ReadAllText($candidate))
            if ($Reload) { Invoke-CoreApi $Root '/configs?force=true' 'PUT' @{path=$live} | Out-Null }
            if ($old) { Write-AtomicText (Join-Path $Root 'private\last-good.json') $old }
        } catch {
            Write-AtomicText (Join-Path $Root 'settings.json') $oldSettings
            if ($old) { Write-AtomicText $live $old }
            if ($Reload -and $old) { try { Invoke-CoreApi $Root '/configs?force=true' 'PUT' @{path=$live} | Out-Null } catch {} }
            throw
        }
        if ($PassThru) { return $normalized }
    }
}

function Update-SplitRuleSets([string]$Root) {
    $settings=Get-SplitSettings $Root; $updated=0; $errors=0
    foreach ($name in $settings.direct.domain_rule_sets) {
        try {
            Invoke-CoreApi $Root ("/providers/rules/service-$name") 'PUT' -TimeoutSeconds 2 | Out-Null
            $updated++
        } catch {
            $errors++
            Write-SplitLog $Root ("Rule set $name update failed; cached rules retained: "+$_.Exception.Message)
        }
    }
    return @{updated=$updated;api_errors=$errors}
}

function Import-SplitProfile([string]$Root, [string]$Source) {
    if((Get-Item -LiteralPath $Source).Length -gt 524288){throw 'Profile import: size limit exceeded.'}
    $text=[IO.File]::ReadAllText($Source);$nodes=@(Convert-SplitProfileText $text)
    if($nodes.Count -ne 1){throw 'Profile import: activation requires exactly one outgoing node.'}
    Initialize-SplitRoot $Root
    $files=@('private\selected-profile.json','private\awg.conf','private\profile-info.json','private\config.json','private\last-good.json','settings.json');$backup=@{}
    foreach($file in $files){$path=Join-Path $Root $file;$backup[$file]=if([IO.File]::Exists($path)){[IO.File]::ReadAllText($path)}else{$null}}
    try{
        Write-AtomicJson (Join-Path $Root 'private\selected-profile.json') @{schema=1;proxy=$nodes[0].proxy}
        if($text.Trim().StartsWith('[Interface]',[StringComparison]::Ordinal)){Write-AtomicText (Join-Path $Root 'private\awg.conf') $text}else{[IO.File]::Delete((Join-Path $Root 'private\awg.conf'))}
        Set-SplitConfig $Root (Get-SplitSettings $Root)
        Write-AtomicJson (Join-Path $Root 'private\profile-info.json') @{source_name=[IO.Path]::GetFileName($Source);protocol=$nodes[0].protocol;imported=[datetime]::UtcNow.ToString('o')}
    }catch{
        $failure=$_
        $rollbackFailed=$false
        foreach($file in $files){
            try{$path=Join-Path $Root $file;if($null -ne $backup[$file]){Write-AtomicText $path $backup[$file]}elseif([IO.File]::Exists($path)){[IO.File]::Delete($path)}}catch{$rollbackFailed=$true}
        }
        if($rollbackFailed){
            $stages=@('profiles');if($failure.Exception.Data.Contains('MukhomorRollbackFailures')){$stages+=@($failure.Exception.Data['MukhomorRollbackFailures'])}
            $failure.Exception.Data['MukhomorRollbackFailures']=@($stages|Sort-Object -Unique)
        }
        throw $failure
    }
}

function Invoke-DnsJson([string]$Provider, [string]$HostName, [int]$Type, [int]$TimeoutSeconds=6) {
    [Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
    $uri=$Provider + '?name=' + [Uri]::EscapeDataString($HostName) + '&type=' + $Type
    return Convert-ToMap (Invoke-RestMethod -Uri $uri -Headers @{Accept='application/dns-json'} -TimeoutSec $TimeoutSeconds -UseBasicParsing)
}

function Resolve-DnsApiHost([string]$HostName, [int]$MaxAddresses=64, [scriptblock]$Request=${function:Invoke-DnsJson}, [int]$BudgetMilliseconds=8000, [string]$Root='') {
    if ($BudgetMilliseconds -lt 1 -or $BudgetMilliseconds -gt 12000) { throw 'Invalid DNS lookup deadline' }
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $assertDeadline={
        if ($Root -and (Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) -and (Test-Path -LiteralPath (Join-Path $Root 'runtime\session.json'))) { throw 'DNS update cancelled' }
        if ($clock.ElapsedMilliseconds -ge $BudgetMilliseconds) { throw 'DNS lookup deadline exceeded' }
    }
    $hostName=Normalize-Domain $HostName
    foreach ($provider in @('https://cloudflare-dns.com/dns-query','https://dns.google/resolve')) {
        & $assertDeadline
        try {
            $addresses=New-Object 'Collections.Generic.HashSet[string]'; $ttl=86400; $invalid=$false
            foreach ($type in @(1,28)) {
                $name=$hostName; $visited=New-Object 'Collections.Generic.HashSet[string]'
                for ($depth=0;$depth -lt 8;$depth++) {
                    & $assertDeadline
                    if (!$visited.Add($name)) { throw 'DNS CNAME loop' }
                    $timeout=[Math]::Max(1,[Math]::Min(6,[Math]::Floor(($BudgetMilliseconds-$clock.ElapsedMilliseconds)/1000)))
                    $reply=& $Request $provider $name $type $timeout
                    & $assertDeadline
                    if (!$reply.ContainsKey('Status') -or $reply.Status -ne 0 -or ($reply.ContainsKey('TC') -and $reply.TC)) { throw 'Unsuccessful DNS response' }
                    $aliases=@{}; $records=@{}
                    if ($reply.ContainsKey('Answer')) {
                        foreach ($answer in @($reply.Answer)) {
                            $owner=Normalize-Domain ([string]$answer.name)
                            if ($answer.TTL -isnot [int] -or $answer.TTL -lt 0 -or $answer.TTL -gt 2147483647) { throw 'Invalid DNS TTL' }
                            if ($answer.type -eq 5) { $aliases[$owner]=@{name=(Normalize-Domain ([string]$answer.data));ttl=[int]$answer.TTL} }
                            elseif ($answer.type -eq $type) {
                                if (!$records.ContainsKey($owner)) { $records[$owner]=@() }; $records[$owner]+=@($answer)
                            }
                        }
                    }
                    $current=$name; $chain=New-Object 'Collections.Generic.HashSet[string]'
                    while ($aliases.ContainsKey($current)) {
                        if (!$chain.Add($current) -or $chain.Count -gt 8) { throw 'DNS CNAME loop' }
                        $ttl=[Math]::Min($ttl,$aliases[$current].ttl); $current=$aliases[$current].name
                    }
                    if ($records.ContainsKey($current)) {
                        foreach ($answer in $records[$current]) {
                            if (!(Test-PublicIp ([string]$answer.data))) { $invalid=$true; continue }
                            $ip=[Net.IPAddress]::Parse([string]$answer.data)
                            if (($type -eq 1 -and $ip.AddressFamily -ne 'InterNetwork') -or ($type -eq 28 -and $ip.AddressFamily -ne 'InterNetworkV6')) { throw 'DNS address family mismatch' }
                            $addresses.Add($ip.ToString()) | Out-Null; $ttl=[Math]::Min($ttl,[int]$answer.TTL)
                        }
                        break
                    }
                    if ($current -ne $name) { $name=$current; continue }
                    break
                }
            }
            if ($addresses.Count -gt $MaxAddresses) { throw 'DNS response exceeds address limit' }
            if ($invalid) { throw 'DNS returned a non-public address; fallback was not updated' }
            if ($addresses.Count -eq 0) { throw 'DNS returned no public addresses' }
            return @{addresses=@($addresses | Sort-Object);ttl=$ttl;provider=$provider}
        } catch { $lastError=$_.Exception.Message }
    }
    throw "DNS API lookup failed for $hostName ($lastError)"
}

function Update-SplitDns([string]$Root, [switch]$Force, [scriptblock]$Resolver=${function:Resolve-DnsApiHost}, [datetime]$Now=[datetime]::UtcNow) {
    Initialize-SplitRoot $Root
    Invoke-SplitLock $Root {
        $batchTimer=[Diagnostics.Stopwatch]::StartNew()
        $s=Get-SplitSettings $Root
        $statePath=Join-Path $Root 'runtime\dns-state.json'
        $state=if (Test-Path -LiteralPath $statePath) { Read-JsonFile $statePath } else { @{schema=1;hosts=@{}} }
        if ($state.schema -ne 1 -or $state.hosts -isnot [hashtable]) { throw 'Invalid DNS cache; previous rules retained' }
        $hosts=New-Object 'Collections.Generic.HashSet[string]'
        foreach ($hostName in @($s.direct.domains)+@($s.direct.domain_suffixes | Where-Object { $_.Contains('.') })+@($s.dns_update.seed_hosts)) {
            if (Test-DirectDomain $hostName $s) { $hosts.Add($hostName) | Out-Null }
        }
        foreach ($hostName in @($state.hosts.Keys | Sort-Object)) {
            if ((Test-DirectDomain $hostName $s) -and $hosts.Count -lt $s.dns_update.max_hosts) { $hosts.Add($hostName) | Out-Null }
        }
        if ($s.dns_update.observe_subdomains) {
            try {
                $connections=Invoke-CoreApi $Root '/connections' -TimeoutSeconds 1
                foreach ($c in @($connections.connections)) {
                    $name=[string]$c.metadata.host
                    if ($name -and $hosts.Count -lt $s.dns_update.max_hosts -and (Test-DirectDomain $name $s)) { $hosts.Add((Normalize-Domain $name)) | Out-Null }
                }
            } catch {} # Offline preparation is supported.
        }
        if ($hosts.Count -gt $s.dns_update.max_hosts) { throw 'Explicit DNS seeds exceed max_hosts' }
        # Retain retry metadata even when a host has never resolved or its IPs
        # have expired. Otherwise NXDOMAIN at the start of the alphabet can
        # consume every batch forever. Migrate old entries without last_attempt.
        foreach ($hostName in @($state.hosts.Keys)) {
            $entry=$state.hosts[$hostName]
            if ($entry -isnot [hashtable] -or $entry.addresses -isnot [array] -or $entry.addresses.Count -gt $s.dns_update.max_addresses_per_host -or $entry.failures -isnot [int] -or $entry.failures -lt 0) { throw 'Invalid DNS cache entry; previous rules retained' }
            [datetime]::Parse($entry.next_refresh).ToUniversalTime() | Out-Null
            foreach ($ip in $entry.addresses) { if (!(Test-PublicIp $ip)) { throw 'Invalid DNS cache IP; previous rules retained' } }
            if ($entry.last_success) { [datetime]::Parse($entry.last_success).ToUniversalTime() | Out-Null }
            elseif ($entry.addresses.Count) { throw 'DNS cache IPs have no success timestamp' }
            if (!$entry.ContainsKey('last_attempt')) { $entry.last_attempt=$entry.last_success }
            if ($entry.last_attempt) { [datetime]::Parse($entry.last_attempt).ToUniversalTime() | Out-Null }
        }
        $pending=New-Object 'Collections.Generic.HashSet[string]'
        if ($state.ContainsKey('refresh_pending')) {
            if ($state.refresh_pending -isnot [array]) { throw 'Invalid DNS refresh queue' }
            foreach ($name in $state.refresh_pending) { if ($hosts.Contains($name)) { $pending.Add($name) | Out-Null } }
        }
        if ($Force) { foreach ($name in $hosts) { $pending.Add($name) | Out-Null } }
        if (!$s.dns_update.enabled) { $pending.Clear() }
        $due=@($hosts | Where-Object {
            !$state.hosts.ContainsKey($_) -or $pending.Contains($_) -or [datetime]::Parse($state.hosts[$_].next_refresh).ToUniversalTime() -le $Now
        } | Sort-Object @{Expression={if ($pending.Contains($_)) {0} else {1}}}, @{Expression={
            if ($state.hosts.ContainsKey($_) -and $state.hosts[$_].last_attempt) { [datetime]::Parse($state.hosts[$_].last_attempt).ToUniversalTime() } else { [datetime]::MinValue }
        }}, @{Expression={$_}})
        $selected=New-Object 'Collections.Generic.HashSet[string]'
        foreach ($name in @($due | Select-Object -First 4)) { $selected.Add($name) | Out-Null }
        $next=@{schema=1;hosts=@{};refresh_pending=@()}; $errors=0; $queried=0
        # Iterate the actual queue order, not alphabetical order. Each cycle has
        # at most four lookups; a slow API also prevents starting extra lookups.
        $ordered=@($due | Where-Object {$selected.Contains($_)}) + @($hosts | Where-Object {!$selected.Contains($_)} | Sort-Object)
        foreach ($hostName in $ordered) {
            $previous=if ($state.hosts.ContainsKey($hostName)) { $state.hosts[$hostName] } else { $null }
            $entry=$previous
            $cancelled=(Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) -and (Test-Path -LiteralPath (Join-Path $Root 'runtime\session.json'))
            if (!$cancelled -and $s.dns_update.enabled -and $selected.Contains($hostName) -and $batchTimer.Elapsed.TotalSeconds -lt 12) {
                try {
                    $budget=[Math]::Min(8000,12000-[int]$batchTimer.ElapsedMilliseconds)
                    $answer=& $Resolver $hostName $s.dns_update.max_addresses_per_host ${function:Invoke-DnsJson} $budget $Root
                    if (!$answer.addresses -or $answer.addresses.Count -gt $s.dns_update.max_addresses_per_host -or $answer.ttl -isnot [int] -or $answer.ttl -lt 0) { throw 'Invalid resolver result' }
                    foreach ($ip in $answer.addresses) { if (!(Test-PublicIp $ip)) { throw 'Invalid resolver IP' } }
                    $delay=[Math]::Max($s.dns_update.min_refresh_seconds,[Math]::Min($s.dns_update.max_refresh_seconds,[Math]::Floor($answer.ttl*0.8)))
                    $entry=@{addresses=@($answer.addresses | Sort-Object -Unique);ttl=$answer.ttl;last_success=$Now.ToString('o');last_attempt=$Now.ToString('o');next_refresh=$Now.AddSeconds($delay).ToString('o');provider=$answer.provider;failures=0;last_error=''}
                    $queried++
                } catch {
                    $errors++
                    $entry=if ($previous) { Convert-ToMap $previous } else { @{addresses=@();ttl=0;last_success=$null;provider='';failures=0} }
                    $entry.failures=[Math]::Min(1000000,[int]$entry.failures+1); $entry.last_attempt=$Now.ToString('o')
                    $entry.last_error=$_.Exception.Message.Substring(0,[Math]::Min(500,$_.Exception.Message.Length))
                    $backoff=[Math]::Min(900,30*[Math]::Pow(2,[Math]::Min(5,$entry.failures)))
                    $entry.next_refresh=$Now.AddSeconds($backoff).ToString('o')
                }
                $pending.Remove($hostName) | Out-Null
            }
            if ($entry) {
                if (!$entry.last_success -or ($Now-[datetime]::Parse($entry.last_success).ToUniversalTime()).TotalSeconds -gt $s.dns_update.max_stale_seconds) { $entry.addresses=@() }
                $next.hosts[$hostName]=$entry
            }
        }
        $next.refresh_pending=@($pending | Sort-Object)
        $ips=New-Object 'Collections.Generic.HashSet[string]'
        if ($s.dns_update.enabled) {
            foreach ($entry in $next.hosts.Values) { foreach ($ip in $entry.addresses) { if (Test-PublicIp $ip) { $ips.Add((Normalize-Cidr $ip)) | Out-Null } } }
        }
        $text=if ($ips.Count -eq 0) { "payload: []`n" } else { "payload:`n" + (@($ips | Sort-Object { Get-CidrSortKey $_ } | ForEach-Object { "  - '$_'" }) -join "`n") + "`n" }
        $providerPath=Join-Path $Root 'runtime\rules\dns-ips.yaml'
        $oldText=[IO.File]::ReadAllText($providerPath)
        $oldState=if (Test-Path -LiteralPath $statePath) { [IO.File]::ReadAllText($statePath) } else { $null }
        try {
            Write-AtomicJson $statePath $next
            if ($text -ne $oldText) {
                Write-AtomicText $providerPath $text
                if (Test-Path -LiteralPath (Join-Path $Root 'runtime\session.json')) {
                    Invoke-CoreApi $Root '/providers/rules/dns-ips' 'PUT' -TimeoutSeconds 1 | Out-Null
                }
            }
        } catch {
            Write-AtomicText $providerPath $oldText
            if ($oldState) { Write-AtomicText $statePath $oldState }
            elseif (Test-Path -LiteralPath $statePath) { [IO.File]::Delete($statePath) }
            try { Invoke-CoreApi $Root '/providers/rules/dns-ips' 'PUT' -TimeoutSeconds 1 | Out-Null } catch {}
            throw
        }
        $successful=@($next.hosts.Values | Where-Object {$_.addresses.Count -gt 0}).Count
        return @{hosts=$successful;tracked_hosts=$next.hosts.Count;ip_addresses=$ips.Count;updated=$queried;api_errors=$errors;pending_refresh=$pending.Count;timestamp=$Now.ToString('o')}
    }
}

function Read-SplitBypassLists([string]$Path) {
    # Parse only the four named sections. Chat timestamps/names are metadata,
    # and malformed tokens inside a list must fail rather than vanish silently.
    $groups=@{}; $group=''
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        if ($line -match '^(Polyzium list|Rabbit-List bypass|CS2-Severs-bypass|Steam bypass):\s*$') {
            $group=$Matches[1]
            if ($groups.ContainsKey($group)) { throw "Duplicate list section: $group" }
            $groups[$group]=@{domains=@();ip_cidrs=@();tokens=0}; continue
        }
        if ($line -match 'Polyzium and Rabbit-List') { $group=''; continue }
        if (!$group -or !$line.Trim()) { continue }
        $line=$line -replace '^\[[^\]]+\]\s+[^:]+:\s*',''
        foreach ($item in @($line -split '\s+' | Where-Object {$_})) {
            if ($item -match '^\d+\.\d+\.\d+\.\d+(?:/\d+)?$' -or $item.Contains(':')) {
                $groups[$group].ip_cidrs+=Normalize-Cidr $item
            } else { $groups[$group].domains+=Normalize-Domain $item -Suffix }
            $groups[$group].tokens++
        }
    }
    $domains=@(); $ips=@(); $tokens=0
    foreach ($name in @('Polyzium list','Rabbit-List bypass','CS2-Severs-bypass','Steam bypass')) {
        if (!$groups.ContainsKey($name) -or !$groups[$name].tokens) { throw "Missing or empty list section: $name" }
        $domains+=@($groups[$name].domains); $ips+=@($groups[$name].ip_cidrs); $tokens+=$groups[$name].tokens
        $groups[$name].domains=@($groups[$name].domains | Sort-Object -Unique)
        $groups[$name].ip_cidrs=@($groups[$name].ip_cidrs | Sort-Object -Unique | Sort-Object {Get-CidrSortKey $_})
    }
    $uniqueDomains=@($domains | Sort-Object -Unique)
    $uniqueIps=@($ips | Sort-Object -Unique | Sort-Object {Get-CidrSortKey $_})
    return @{
        schema=1;source_name=[IO.Path]::GetFileName($Path);source_sha256=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
        groups=$groups;domains=$uniqueDomains;ip_cidrs=$uniqueIps
        statistics=@{tokens=$tokens;domains=$uniqueDomains.Count;ip_cidrs=$uniqueIps.Count;duplicates_removed=$tokens-$uniqueDomains.Count-$uniqueIps.Count}
        duplicate_domains=@($domains | Group-Object | Where-Object {$_.Count -gt 1} | ForEach-Object {$_.Name} | Sort-Object)
        duplicate_ip_cidrs=@($ips | Group-Object | Where-Object {$_.Count -gt 1} | ForEach-Object {$_.Name} | Sort-Object {Get-CidrSortKey $_})
    }
}

function Write-SplitLog([string]$Root, [string]$Message) {
    $path=Join-Path $Root 'runtime\logs\supervisor.log'
    if ((Test-Path -LiteralPath $path) -and (Get-Item -LiteralPath $path).Length -gt 2097152) {
        [IO.File]::Copy($path,$path+'.previous',$true); [IO.File]::WriteAllText($path,'')
    }
    [IO.File]::AppendAllText($path,([datetime]::UtcNow.ToString('o')+' '+$Message+"`r`n"),(New-Object Text.UTF8Encoding($false)))
}

Export-ModuleMember -Function ([string[]]@(Get-Command -CommandType Function | Where-Object {$_.ModuleName -in @($ExecutionContext.SessionState.Module.Name,'ProfileImport') -and $_.Name -ne 'New-ValidatedCoreConfig'} | ForEach-Object {$_.Name}))
