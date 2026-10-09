# Pure, bounded import of standalone Mihomo v1.19.32 outgoing nodes.
# Imported data never controls global DNS, routes, providers, files or scripts.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'

function Get-SplitProfileCapabilities {
    $labels=@{wireguard='WireGuard / AmneziaWG';vless='VLESS';vmess='VMess';trojan='Trojan';ss='Shadowsocks';ssr='ShadowsocksR';hysteria='Hysteria';hysteria2='Hysteria 2';tuic='TUIC';snell='Snell';socks5='SOCKS5';http='HTTP / HTTPS';ssh='SSH';anytls='AnyTLS';mieru='Mieru';trusttunnel='TrustTunnel';shadowquic='ShadowQUIC';'gost-relay'='GOST Relay';sudoku='Sudoku';masque='MASQUE';openvpn='OpenVPN'}
    foreach($type in @('wireguard','vless','vmess','trojan','ss','ssr','hysteria','hysteria2','tuic','snell','socks5','http','ssh','anytls','mieru','trusttunnel','shadowquic','gost-relay','sudoku','masque','openvpn')){
        @{type=$type;label=$labels[$type];json=$true;uri=($type -in @('vless','vmess','trojan','ss','ssr','hysteria','hysteria2','tuic','socks5','http','anytls'));conf=($type -eq 'wireguard');ovpn=($type -eq 'openvpn')}
    }
}

function Convert-ProfileMap($Value) {
    if($null -eq $Value -or $Value -is [string] -or $Value -is [ValueType]){return $Value}
    if($Value -is [System.Collections.IDictionary]){$map=@{};foreach($key in $Value.Keys){$map[$key]=Convert-ProfileMap $Value[$key]};return $map}
    if($Value -is [array]){return ,@($Value|ForEach-Object {Convert-ProfileMap $_})}
    $map=@{};foreach($property in $Value.PSObject.Properties){$map[$property.Name]=Convert-ProfileMap $property.Value};return $map
}

function Assert-ProfileText([string]$Text,[int]$Maximum=524288) {
    if(!$Text -or $Text.Length -gt $Maximum){throw 'Profile import: empty input or size limit exceeded.'}
    try{$size=(New-Object Text.UTF8Encoding($false,$true)).GetByteCount($Text)}catch{throw 'Profile import: invalid Unicode.'}
    if($size -gt $Maximum -or $Text.Contains([char]0)){throw 'Profile import: invalid input or size limit exceeded.'}
}

function Read-ProfileJson([string]$Text) {
    Assert-ProfileText $Text
    if(!('Mukhomor.ProfileJsonGuard' -as [type])){
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Text;
namespace Mukhomor {
 public sealed class ProfileJsonGuard {
  string s; int p, tokens;
  ProfileJsonGuard(string text){s=text;}
  void Bad(){throw new FormatException("Invalid profile JSON");}
  void Ws(){while(p<s.Length && (s[p]==' '||s[p]=='\t'||s[p]=='\r'||s[p]=='\n'))p++;}
  string Str(){
   if(p>=s.Length||s[p++]!='"')Bad(); var b=new StringBuilder();
   while(p<s.Length){char c=s[p++]; if(c=='"')return b.ToString(); if(c<32)Bad();
    if(c=='\\'){if(p>=s.Length)Bad(); c=s[p++];
     if(c=='u'){if(p+4>s.Length)Bad();int n; if(!int.TryParse(s.Substring(p,4),System.Globalization.NumberStyles.HexNumber,System.Globalization.CultureInfo.InvariantCulture,out n))Bad();b.Append((char)n);p+=4;}
     else if(c=='"'||c=='\\'||c=='/')b.Append(c);
     else if(c=='b')b.Append('\b'); else if(c=='f')b.Append('\f'); else if(c=='n')b.Append('\n');else if(c=='r')b.Append('\r');else if(c=='t')b.Append('\t');else Bad();
    }else b.Append(c); if(b.Length>65536)Bad();
   } Bad();return null;
  }
  void Value(int depth){
   if(depth>12||++tokens>16384)Bad();Ws();if(p>=s.Length)Bad();char c=s[p];
   if(c=='{'){
    p++;Ws();if(p<s.Length&&s[p]=='}'){p++;return;}var keys=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
    while(true){Ws();string k=Str();if(k.Length>64||!keys.Add(k)||keys.Count>128)Bad();Ws();if(p>=s.Length||s[p++]!=':')Bad();Value(depth+1);Ws();if(p>=s.Length)Bad();c=s[p++];if(c=='}')return;if(c!=',')Bad();}
   }
   if(c=='['){p++;Ws();if(p<s.Length&&s[p]==']'){p++;return;}int n=0;while(true){if(++n>128)Bad();Value(depth+1);Ws();if(p>=s.Length)Bad();c=s[p++];if(c==']')return;if(c!=',')Bad();}}
   if(c=='"'){Str();return;}
   foreach(string word in new[]{"true","false","null"}){if(s.Substring(p).StartsWith(word,StringComparison.Ordinal)){p+=word.Length;return;}}
   int start=p;if(c=='-')p++;if(p>=s.Length)Bad();
   if(s[p]=='0')p++;else{if(s[p]<'1'||s[p]>'9')Bad();while(p<s.Length&&s[p]>='0'&&s[p]<='9')p++;}
   if(p<s.Length&&s[p]=='.'){p++;int q=p;while(p<s.Length&&s[p]>='0'&&s[p]<='9')p++;if(q==p)Bad();}
   if(p<s.Length&&(s[p]=='e'||s[p]=='E')){p++;if(p<s.Length&&(s[p]=='+'||s[p]=='-'))p++;int q=p;while(p<s.Length&&s[p]>='0'&&s[p]<='9')p++;if(q==p)Bad();}
   if(p-start>64)Bad();
  }
  public static void Validate(string text){var v=new ProfileJsonGuard(text);v.Value(0);v.Ws();if(v.p!=text.Length)v.Bad();}
 }
}
'@
    }
    try{[Mukhomor.ProfileJsonGuard]::Validate($Text);return Convert-ProfileMap ($Text|ConvertFrom-Json)}catch{throw 'Profile import: invalid, duplicate or excessively nested JSON.'}
}

function Convert-ProfileBase64([string]$Text) {
    if(!$Text -or $Text.Length -gt 699052 -or $Text -notmatch '^[A-Za-z0-9_+/=-]+$'){throw 'Profile import: invalid base64 data.'}
    $value=$Text.Replace('-','+').Replace('_','/');$value=$value.TrimEnd('=')
    if($value.Length%4 -eq 1){throw 'Profile import: invalid base64 data.'}
    $value=$value.PadRight($value.Length+((4-$value.Length%4)%4),'=')
    try{return (New-Object Text.UTF8Encoding($false,$true)).GetString([Convert]::FromBase64String($value))}catch{throw 'Profile import: invalid base64 or UTF-8 data.'}
}

function Decode-ProfileUri([string]$Value) {
    if($Value -match '%(?![0-9A-Fa-f]{2})'){throw 'Profile import: invalid URL encoding.'}
    try{
        return [regex]::Replace($Value,'(?:%[0-9A-Fa-f]{2})+',{
            param($match)
            $bytes=New-Object byte[] ($match.Value.Length/3)
            for($n=0;$n -lt $bytes.Length;$n++){$bytes[$n]=[Convert]::ToByte($match.Value.Substring($n*3+1,2),16)}
            (New-Object Text.UTF8Encoding($false,$true)).GetString($bytes)
        })
    }catch{throw 'Profile import: invalid URL encoding or UTF-8.'}
}

function Read-ProfileQuery([string]$Query,[string[]]$Allowed) {
    $result=@{}
    if(!$Query){return $result}
    foreach($pair in $Query.TrimStart('?').Split('&')){
        if(!$pair){continue};$parts=$pair.Split([char[]]@('='),2);$key=(Decode-ProfileUri $parts[0]).ToLowerInvariant()
        if($key -notin $Allowed -or $result.ContainsKey($key)){throw 'Profile import: unsupported or duplicate link parameter.'}
        $result[$key]=if($parts.Count -eq 2){Decode-ProfileUri $parts[1]}else{''}
    }
    return $result
}

function Convert-ProfileBoolean($Value) {
    if($Value -is [bool]){return $Value}
    if([string]$Value -in @('1','true')){return $true}
    if([string]$Value -in @('0','false')){return $false}
    throw 'Profile import: invalid boolean option.'
}

function Normalize-ProfileHost([string]$HostName) {
    if(!$HostName -or $HostName.Length -gt 253 -or $HostName -match '[\s\x00-\x1f/\\@?#]'){throw 'Profile import: invalid server hostname.'}
    $value=$HostName.TrimStart('[').TrimEnd(']');$ip=$null
    if([Net.IPAddress]::TryParse($value,[ref]$ip)){
        if($ip.Equals([Net.IPAddress]::Any) -or $ip.Equals([Net.IPAddress]::IPv6Any) -or $value.Contains('%')){throw 'Profile import: unspecified or scoped endpoint is unsupported.'}
        return $ip.ToString()
    }
    try{$value=(New-Object Globalization.IdnMapping).GetAscii($value.TrimEnd('.')).ToLowerInvariant()}catch{throw 'Profile import: invalid server hostname.'}
    if($value.Length -gt 253 -or $value -notmatch '^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*$'){throw 'Profile import: invalid server hostname.'}
    return $value
}

function Get-ProfileNodeFields([string]$Type) {
    $tls='alpn skip-cert-verify name-cert-verify fingerprint certificate private-key'
    $stream='network ws-opts http-opts h2-opts grpc-opts reality-opts packet-addr xudp packet-encoding'
    $fields=@{
        wireguard='ip ipv6 private-key public-key pre-shared-key reserved allowed-ips workers mtu udp persistent-keepalive amnezia-wg-option ip-stack remote-dns-resolve dns refresh-server-ip-interval'
        ss='password cipher udp plugin plugin-opts udp-over-tcp udp-over-tcp-version client-fingerprint'
        ssr='password cipher obfs obfs-param protocol protocol-param udp'
        vmess="uuid alterId cipher udp tls global-padding authenticated-length servername client-fingerprint $tls $stream"
        vless="uuid flow encryption udp tls ws-headers xhttp-opts servername client-fingerprint $tls $stream"
        trojan="password udp network ws-opts grpc-opts reality-opts ss-opts sni client-fingerprint $tls"
        hysteria="ports protocol obfs-protocol up up-speed down down-speed auth auth-str obfs recv-window-conn recv-window disable-mtu-discovery fast-open hop-interval sni $tls"
        hysteria2="ports hop-interval up down password obfs obfs-password obfs-min-packet-size obfs-max-packet-size cwnd bbr-profile udp-mtu handshake-timeout initial-stream-receive-window max-stream-receive-window initial-connection-receive-window max-connection-receive-window sni $tls"
        tuic="token uuid password ip heartbeat-interval reduce-rtt request-timeout udp-relay-mode congestion-controller disable-sni max-udp-relay-packet-size fast-open max-open-streams cwnd bbr-profile recv-window-conn recv-window disable-mtu-discovery max-datagram-frame-size udp-over-stream udp-over-stream-version sni $tls"
        snell='psk udp version reuse obfs-opts client-fingerprint'
        socks5='username password tls udp skip-cert-verify name-cert-verify fingerprint certificate private-key'
        http='username password tls headers sni skip-cert-verify name-cert-verify fingerprint certificate private-key'
        ssh='username password private-key private-key-passphrase host-key host-key-algorithms'
        anytls="password udp client-metadata idle-session-check-interval idle-session-timeout min-idle-session disable-reuse sni client-fingerprint $tls"
        mieru='port-range transport udp username password multiplexing handshake-mode traffic-pattern'
        trusttunnel="username password udp health-check quic congestion-controller cwnd bbr-profile max-connections min-streams max-streams sni client-fingerprint $tls"
        shadowquic='username password sni alpn quic-versions udp-over-stream zero-rtt keep-alive-interval congestion-controller up down cwnd bbr-profile recv-window-conn recv-window disable-mtu-discovery max-datagram-frame-size max-open-streams'
        'gost-relay'='username password forward udp tls mux sni skip-cert-verify name-cert-verify fingerprint certificate private-key client-fingerprint'
        sudoku='key aead-method padding-min padding-max table-type enable-pure-downlink http-mask http-mask-mode http-mask-tls http-mask-host path-root multiplex http-mask-multiplex httpmask custom-table custom-tables'
        masque='private-key public-key ip ipv6 uri sni mtu udp handshake-timeout skip-cert-verify network congestion-controller cwnd bbr-profile ip-stack remote-dns-resolve dns'
        openvpn='proto dev cipher data-ciphers data-ciphers-fallback auth comp-lzo ca cert key tls-auth key-direction tls-crypt tls-crypt-v2 username password peer-info ping ping-restart tran-window handshake-timeout mtu udp ip-stack remote-dns-resolve dns'
    }
    if(!$fields.ContainsKey($Type)){throw 'Profile import: unsupported protocol.'}
    return @('name','type','server','port','tfo','mptcp','ip-version')+@($fields[$Type].Split(' ')|Where-Object {$_}|Sort-Object -Unique)
}

function Assert-ProfileOptionValue($Value,[string]$Key,[int]$Depth=0) {
    if($Depth -gt 12 -or $null -eq $Value){throw 'Profile import: invalid node option.'}
    if($Value -is [string]){
        if($Value.Length -gt 65536 -or $Value.Contains([char]0)){throw 'Profile import: node option exceeds its limit.'}
        try{[void](New-Object Text.UTF8Encoding($false,$true)).GetByteCount($Value)}catch{throw 'Profile import: invalid Unicode option.'}
        if($Key -match 'window|cwnd|streams|connections|workers|concurrency'){
            if($Value -notmatch '^\d{1,9}(?:-\d{1,9})?$'){throw 'Profile import: invalid numeric resource limit.'}
            foreach($part in $Value.Split('-')){Assert-ProfileOptionValue ([long]$part) $Key ($Depth+1)}
        }
        return
    }
    if($Value -is [bool]){return}
    if($Value -is [ValueType]){
        $number=[double]$Value
        if([double]::IsNaN($number) -or [double]::IsInfinity($number) -or [Math]::Abs($number) -gt 4294967295){throw 'Profile import: numeric option exceeds its limit.'}
        if($Key -match 'window|cwnd' -and ($number -lt 0 -or $number -gt 134217728)){throw 'Profile import: receive window exceeds its limit.'}
        if($Key -eq 'workers' -and ($number -lt 0 -or $number -gt 32)){throw 'Profile import: worker count exceeds its limit.'}
        if($Key -match 'streams|connections|concurrency' -and ($number -lt 0 -or $number -gt 1024)){throw 'Profile import: concurrency exceeds its limit.'}
        return
    }
    if($Value -is [array]){
        if($Value.Count -gt 128){throw 'Profile import: option list exceeds its limit.'}
        foreach($item in $Value){Assert-ProfileOptionValue $item $Key ($Depth+1)};return
    }
    if($Value -is [System.Collections.IDictionary]){
        if($Value.Count -gt 64){throw 'Profile import: option object exceeds its limit.'}
        foreach($field in $Value.Keys){if($field -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$'){throw 'Profile import: invalid option key.'};Assert-ProfileOptionValue $Value[$field] $field ($Depth+1)};return
    }
    throw 'Profile import: unsupported option value.'
}

function Assert-ProfileNestedOptions($Proxy) {
    $nested=@{
        'ws-opts'='path headers max-early-data early-data-header-name v2ray-http-upgrade v2ray-http-upgrade-fast-open'
        'http-opts'='method path headers';'h2-opts'='host path';'grpc-opts'='grpc-service-name grpc-user-agent max-connections min-streams max-streams ping-interval'
        'reality-opts'='public-key short-id support-x25519mlkem768'
        'xhttp-opts'='host path mode headers no-grpc-header x-padding-bytes x-padding-obfs-mode x-padding-key x-padding-header x-padding-placement uplink-http-method session-id-length session-id-placement session-id-key session-length seq-placement seq-key uplink-data-placement uplink-data-key uplink-chunk-size sc-max-each-post-bytes sc-min-posts-interval-ms reuse-settings'
        'reuse-settings'='max-concurrency max-connections c-max-reuse-times h-max-request-times h-max-reusable-secs h-keep-alive-period'
        'ss-opts'='enabled method password';'obfs-opts'='mode host'
        'httpmask'='disable mode tls host path-root multiplex'
        'ip-stack'='mode congestion-controller'
        'amnezia-wg-option'='version jc jmin jmax s1 s2 s3 s4 h1 h2 h3 h4 i1 i2 i3 i4 i5 j1 j2 j3 itime header-protection-key content-padding-addition rekey-after-time rekey-timeout reject-after-time keepalive-timeout max-handshake-attempts random-trailers disable-cookies'
    }
    foreach($key in $Proxy.Keys){
        $value=$Proxy[$key]
        if($nested.ContainsKey($key)){
            if($value -isnot [System.Collections.IDictionary]){throw 'Profile import: invalid transport options.'}
            foreach($field in $value.Keys){if($field -notin $nested[$key].Split(' ')){throw 'Profile import: unsupported transport option.'}}
            Assert-ProfileNestedOptions $value
        }elseif($key -eq 'plugin-opts'){
            if($value -isnot [System.Collections.IDictionary] -or !$Proxy.ContainsKey('plugin')){throw 'Profile import: invalid plugin options.'}
            $plugins=@{obfs='mode host';'v2ray-plugin'='mode tls host path mux skip-cert-verify headers';'shadow-tls'='host password version'}
            if(!$plugins.ContainsKey($Proxy.plugin)){throw 'Profile import: unsupported built-in plugin.'}
            foreach($field in $value.Keys){if($field -notin $plugins[$Proxy.plugin].Split(' ')){throw 'Profile import: unsupported plugin option.'}}
            Assert-ProfileNestedOptions $value
        }elseif($value -is [System.Collections.IDictionary] -and $key -notin @('headers','ws-headers','peer-info')){throw 'Profile import: unsupported nested options.'}
        if($key -in @('headers','ws-headers','peer-info')){
            if($value -isnot [System.Collections.IDictionary]){throw 'Profile import: invalid header options.'}
            foreach($field in $value.Keys){foreach($part in @($value[$field])){if($part -isnot [string] -or $part -match '[\r\n\x00]'){throw 'Profile import: invalid header option.'}}}
        }
    }
}

function Test-ProfileInlinePem([string]$Value,[string]$Kind='key') {
    if($Kind -eq 'certificate'){
        return $Value -match '(?s)^\s*(?:-----BEGIN CERTIFICATE-----\r?\n[A-Za-z0-9+/=\r\n]+-----END CERTIFICATE-----\s*)+$'
    }
    if($Kind -eq 'static'){return $Value -match '(?s)^\s*-----BEGIN (OpenVPN Static key V1|OpenVPN tls-crypt-v2 client key)-----\r?\n[A-Za-z0-9+/=\r\n]+-----END \1-----\s*$'}
    return $Value -match '(?s)^\s*-----BEGIN (PRIVATE KEY|ENCRYPTED PRIVATE KEY|RSA PRIVATE KEY|EC PRIVATE KEY|OPENSSH PRIVATE KEY)-----\r?\n[A-Za-z0-9+/=\r\n]+-----END \1-----\s*$'
}

function Convert-SplitNode($Node) {
    $node=Convert-ProfileMap $Node
    if($node -isnot [System.Collections.IDictionary] -or !$node.ContainsKey('type') -or $node.type -isnot [string]){throw 'Profile import: expected one outgoing node.'}
    $type=$node.type.ToLowerInvariant();$allowed=@(Get-ProfileNodeFields $type);$proxy=@{}
    foreach($key in $node.Keys){
        $canonical=@($allowed|Where-Object {$_ -ieq $key})
        if(!$canonical.Count){throw 'Profile import: unsupported node option or configuration directive.'}
        Assert-ProfileOptionValue $node[$key] $key
        $proxy[$canonical[0]]=$node[$key]
    }
    $proxy.type=$type;$proxy.name='AWG'
    foreach($key in @('name','username','password','token','sni','servername','uuid','cipher','psk')){if($node.ContainsKey($key) -and $node[$key] -isnot [string]){throw 'Profile import: credential or identity option has an invalid type.'}}
    foreach($key in @('sni','servername')){if($proxy.ContainsKey($key) -and $proxy[$key] -match '[\r\n\x00]'){throw 'Profile import: invalid TLS server identity.'}}
    if(!$proxy.ContainsKey('server') -or $proxy.server -isnot [string]){throw 'Profile import: server is missing.'}
    $proxy.server=Normalize-ProfileHost $proxy.server
    if(!$proxy.ContainsKey('port') -or [string]$proxy.port -notmatch '^\d{1,5}$' -or [int]$proxy.port -lt 1 -or [int]$proxy.port -gt 65535){throw 'Profile import: endpoint port must be between 1 and 65535.'}
    $proxy.port=[int]$proxy.port
    foreach($key in @('tls','udp','skip-cert-verify','tfo','mptcp','packet-addr','xudp','global-padding','authenticated-length','reuse','disable-mtu-discovery','fast-open','reduce-rtt','disable-sni','udp-over-stream','udp-over-tcp','zero-rtt','disable-reuse','health-check','quic','remote-dns-resolve','enable-pure-downlink','http-mask','http-mask-tls','forward','mux')){if($proxy.ContainsKey($key) -and $proxy[$key] -isnot [bool]){throw 'Profile import: boolean node option has an invalid type.'}}
    $required=@{ss=@('password','cipher');ssr=@('password','cipher','protocol','obfs');vmess=@('uuid');vless=@('uuid');trojan=@('password');snell=@('psk');anytls=@('password');mieru=@('username','password','transport');ssh=@('username');sudoku=@('key');wireguard=@('private-key','public-key','ip');masque=@('private-key','public-key');openvpn=@('ca')}
    if($required.ContainsKey($type)){foreach($key in $required[$type]){if(!$proxy.ContainsKey($key) -or $proxy[$key] -isnot [string] -or !$proxy[$key]){throw 'Profile import: a required protocol credential or option is missing.'}}}
    if($type -in @('vmess','vless') -or ($type -eq 'tuic' -and $proxy.ContainsKey('uuid'))){$uuid=[guid]::Empty;if(![guid]::TryParse([string]$proxy.uuid,[ref]$uuid)){throw 'Profile import: invalid UUID.'};$proxy.uuid=$uuid.ToString()}
    if($type -eq 'vmess'){if(!$proxy.ContainsKey('alterId')){$proxy.alterId=0};if(!$proxy.ContainsKey('cipher')){$proxy.cipher='auto'}}
    if($type -eq 'tuic' -and !$proxy.ContainsKey('token') -and (!$proxy.ContainsKey('uuid') -or !$proxy.ContainsKey('password'))){throw 'Profile import: TUIC authentication is missing.'}
    if($type -eq 'ssh' -and !$proxy.ContainsKey('password') -and !$proxy.ContainsKey('private-key')){throw 'Profile import: SSH authentication is missing.'}
    if($type -eq 'ssh' -and (!$proxy.ContainsKey('host-key') -or $proxy['host-key'] -isnot [array] -or !$proxy['host-key'].Count)){throw 'Profile import: SSH requires explicit server host keys.'}
    if($type -eq 'wireguard'){
        foreach($key in @('private-key','public-key','pre-shared-key')){if($proxy.ContainsKey($key)){try{$size=[Convert]::FromBase64String($proxy[$key]).Length}catch{throw 'Profile import: invalid WireGuard key.'};if($size -ne 32){throw 'Profile import: invalid WireGuard key.'}}}
        $ip=$null;if(![Net.IPAddress]::TryParse($proxy.ip,[ref]$ip) -or $ip.AddressFamily -ne 'InterNetwork'){throw 'Profile import: IPv4 tunnel address is required.'}
        $proxy['allowed-ips']=@('0.0.0.0/0','::/0')
    }
    foreach($key in @('certificate','private-key')){
        if($proxy.ContainsKey($key) -and !($key -eq 'private-key' -and $type -in @('wireguard','masque'))){
            $kind=if($key -eq 'certificate'){'certificate'}else{'key'}
            if($proxy[$key] -isnot [string] -or !(Test-ProfileInlinePem $proxy[$key] $kind)){throw 'Profile import: keys and certificates must be inline PEM; file paths are unsupported.'}
        }
    }
    if($type -eq 'openvpn'){
        foreach($key in @('ca','cert','key','tls-auth','tls-crypt','tls-crypt-v2')){
            $kind=if($key -in @('ca','cert')){'certificate'}elseif($key -match '^tls-'){'static'}else{'key'}
            if($proxy.ContainsKey($key) -and !(Test-ProfileInlinePem $proxy[$key] $kind)){throw 'Profile import: OpenVPN certificates and keys must be inline.'}
        }
        if($proxy.ContainsKey('dev') -and $proxy.dev -ne 'tun'){throw 'Profile import: only OpenVPN TUN mode is supported.'}
    }
    Assert-ProfileNestedOptions $proxy
    if($type -eq 'snell'){
        $version=if($proxy.ContainsKey('version')){[int]$proxy.version}else{1}
        if($version -le 2 -and $proxy.ContainsKey('udp') -and $proxy.udp){throw 'Profile import: Snell versions 1 and 2 do not support UDP.'}
        if(!$proxy.ContainsKey('udp')){$proxy.udp=($version -ge 3)}
    }elseif($type -eq 'masque' -and $proxy.ContainsKey('network') -and $proxy.network -eq 'h3-l4proxy'){
        if($proxy.ContainsKey('udp') -and $proxy.udp){throw 'Profile import: MASQUE h3-l4proxy does not support UDP.'};$proxy.udp=$false
    }elseif($type -in @('wireguard','ss','ssr','vmess','vless','trojan','socks5','anytls','mieru','trusttunnel','gost-relay','masque','openvpn') -and !$proxy.ContainsKey('udp')){$proxy.udp=$true}
    return $proxy
}

function Convert-AwgProfileText([string]$Text) {
    Assert-ProfileText $Text
    $sections=@{};$section='';$peers=0
    foreach($line in $Text -split '\r?\n'){
        $line=$line.Trim();if(!$line -or $line.StartsWith('#') -or $line.StartsWith(';')){continue}
        if($line -match '^\[(Interface|Peer)\]$'){$section=$Matches[1];if($section -eq 'Peer'){$peers++};if($sections.ContainsKey($section)){throw 'Profile import: only one WireGuard peer is supported.'};$sections[$section]=@{};continue}
        if(!$section -or $line -notmatch '^([^=]+?)\s*=\s*(.*)$'){throw 'Profile import: invalid WireGuard profile line.'}
        $key=$Matches[1].Trim();if($sections[$section].ContainsKey($key)){throw 'Profile import: duplicate WireGuard field.'};$sections[$section][$key]=$Matches[2].Trim()
    }
    if($peers -ne 1 -or !$sections.ContainsKey('Interface')){throw 'Profile import: expected Interface and one Peer.'}
    $iface=$sections.Interface;$peer=$sections.Peer
    $fields=@('PrivateKey','Address','DNS','MTU','ListenPort','Jc','Jmin','Jmax','S1','S2','S3','S4','H1','H2','H3','H4','I1','I2','I3','I4','I5','J1','J2','J3','ITime')
    foreach($key in $iface.Keys){if($key -notin $fields){throw 'Profile import: unsupported WireGuard or AmneziaWG field.'}}
    foreach($key in $peer.Keys){if($key -notin @('PublicKey','PresharedKey','Endpoint','AllowedIPs','PersistentKeepalive')){throw 'Profile import: unsupported WireGuard peer field.'}}
    if(!$iface.ContainsKey('PrivateKey') -or !$iface.ContainsKey('Address') -or !$peer.ContainsKey('PublicKey') -or !$peer.ContainsKey('Endpoint')){throw 'Profile import: WireGuard credentials or endpoint are missing.'}
    if($peer.Endpoint -notmatch '^(?:\[([^\]]+)\]|([^:]+)):(\d+)$'){throw 'Profile import: invalid WireGuard endpoint.'}
    $server=if($Matches[1]){$Matches[1]}else{$Matches[2]};$port=$Matches[3]
    $proxy=@{name='AWG';type='wireguard';server=$server;port=$port;'private-key'=$iface.PrivateKey;'public-key'=$peer.PublicKey;udp=$true;mtu=1280}
    if($peer.ContainsKey('PresharedKey')){$proxy['pre-shared-key']=$peer.PresharedKey}
    if($peer.ContainsKey('PersistentKeepalive')){$proxy['persistent-keepalive']=[int]$peer.PersistentKeepalive}
    foreach($address in $iface.Address.Split(',')){$ip=$null;if(![Net.IPAddress]::TryParse($address.Trim().Split('/')[0],[ref]$ip)){throw 'Profile import: invalid tunnel address.'};if($ip.AddressFamily -eq 'InterNetwork'){$proxy.ip=$ip.ToString()}else{$proxy.ipv6=$ip.ToString()}}
    if($iface.ContainsKey('MTU')){$proxy.mtu=[int]$iface.MTU};if($proxy.mtu -lt 576 -or $proxy.mtu -gt 9000){throw 'Profile import: invalid tunnel MTU.'}
    $awg=@{};foreach($key in @('Jc','Jmin','Jmax','S1','S2','S3','S4','H1','H2','H3','H4','I1','I2','I3','I4','I5','J1','J2','J3','ITime')){if($iface.ContainsKey($key)){$awg[$key.ToLowerInvariant()]=if($key -match '^(Jc|Jmin|Jmax|S[1-4]|ITime)$'){[int]$iface[$key]}else{$iface[$key]}}}
    if($awg.Count){foreach($key in @('Jc','Jmin','Jmax','S1','S2','H1','H2','H3','H4')){if(!$iface.ContainsKey($key)){throw 'Profile import: required AmneziaWG field is missing.'}};$awg.version=if($iface.ContainsKey('J1') -or $iface.ContainsKey('ITime')){1}else{2};$proxy['amnezia-wg-option']=$awg}
    return Convert-SplitNode $proxy
}

function Set-ProfileLinkTransport($Proxy,$Query) {
    if($Query.ContainsKey('security')){if($Query.security -notin @('none','tls','reality') -or ($Proxy.type -eq 'trojan' -and $Query.security -eq 'none')){throw 'Profile import: unsupported link security mode.'};if($Proxy.type -ne 'trojan'){$Proxy.tls=$Query.security -ne 'none'}}
    foreach($key in @('sni','peer','servername')){if($Query.ContainsKey($key)){if($Proxy.type -in @('vmess','vless')){$Proxy.servername=$Query[$key]}else{$Proxy.sni=$Query[$key]}}}
    foreach($key in @('insecure','allowinsecure','allow_insecure','skip-cert-verify')){if($Query.ContainsKey($key)){$Proxy['skip-cert-verify']=Convert-ProfileBoolean $Query[$key]}}
    if($Query.ContainsKey('fp')){$Proxy['client-fingerprint']=$Query.fp}
    if($Query.ContainsKey('alpn')){$Proxy.alpn=@($Query.alpn.Split(','))}
    if($Query.ContainsKey('flow')){$Proxy.flow=$Query.flow}
    if($Query.ContainsKey('encryption')){$Proxy.encryption=$Query.encryption}
    if($Query.ContainsKey('security') -and $Query.security -eq 'reality'){
        if(!$Query.ContainsKey('pbk')){throw 'Profile import: Reality public key is missing.'}
        $Proxy['reality-opts']=@{'public-key'=$Query.pbk};if($Query.ContainsKey('sid')){$Proxy['reality-opts']['short-id']=$Query.sid}
        if(!$Proxy.ContainsKey('client-fingerprint')){$Proxy['client-fingerprint']='chrome'}
    }elseif($Query.ContainsKey('pbk') -or $Query.ContainsKey('sid')){throw 'Profile import: Reality parameters require Reality security.'}
    $network=if($Query.ContainsKey('type')){$Query.type}elseif($Query.ContainsKey('network')){$Query.network}else{'tcp'}
    if($network -notin @('tcp','ws','grpc','http','h2','xhttp','httpupgrade')){throw 'Profile import: unsupported link transport.'}
    if($network -ne 'tcp'){$Proxy.network=if($network -eq 'httpupgrade'){'ws'}else{$network}}
    switch($network){
        {$_ -in @('ws','httpupgrade')} {$opts=@{};if($Query.ContainsKey('path')){$opts.path=$Query.path};if($Query.ContainsKey('host')){$opts.headers=@{Host=$Query.host}};if($network -eq 'httpupgrade'){$opts['v2ray-http-upgrade']=$true};$Proxy['ws-opts']=$opts}
        'grpc' {$opts=@{};if($Query.ContainsKey('servicename')){$opts['grpc-service-name']=$Query.servicename}elseif($Query.ContainsKey('path')){$opts['grpc-service-name']=$Query.path};$Proxy['grpc-opts']=$opts}
        'http' {$opts=@{};if($Query.ContainsKey('path')){$opts.path=@($Query.path)};if($Query.ContainsKey('host')){$opts.headers=@{Host=@($Query.host)}};$Proxy['http-opts']=$opts}
        'h2' {$opts=@{};if($Query.ContainsKey('path')){$opts.path=$Query.path};if($Query.ContainsKey('host')){$opts.host=@($Query.host)};$Proxy['h2-opts']=$opts}
        'xhttp' {$opts=@{};foreach($key in @('path','host','mode')){if($Query.ContainsKey($key)){$opts[$key]=$Query[$key]}};$Proxy['xhttp-opts']=$opts}
    }
}

function Convert-ProfileLink([string]$Link) {
    Assert-ProfileText $Link
    if($Link -notmatch '^([a-zA-Z0-9]+)://(.+)$'){throw 'Profile import: invalid share link.'}
    $scheme=$Matches[1].ToLowerInvariant();$body=$Matches[2];$name=''
    if($body.Contains('#')){$parts=$body.Split([char[]]@('#'),2);$body=$parts[0];$name=Decode-ProfileUri $parts[1]}
    if($scheme -eq 'vmess'){
        $value=Read-ProfileJson (Convert-ProfileBase64 $body)
        if($value -isnot [System.Collections.IDictionary]){throw 'Profile import: invalid VMess link.'}
        foreach($key in $value.Keys){if($key -notin @('v','ps','add','port','id','aid','scy','net','type','host','path','tls','sni','alpn','fp','allowInsecure')){throw 'Profile import: unsupported VMess link option.'}}
        foreach($key in @('add','port','id')){if(!$value.ContainsKey($key)){throw 'Profile import: incomplete VMess link.'}}
        $proxy=@{type='vmess';server=$value.add;port=$value.port;uuid=$value.id;alterId=0;cipher='auto';udp=$true}
        if($value.ContainsKey('aid')){$proxy.alterId=[int]$value.aid};if($value.ContainsKey('scy') -and $value.scy){$proxy.cipher=$value.scy}
        $query=@{};foreach($pair in @(@('net','type'),@('host','host'),@('path','path'),@('sni','sni'),@('alpn','alpn'),@('fp','fp'),@('allowInsecure','allowinsecure'))){if($value.ContainsKey($pair[0]) -and [string]$value[$pair[0]]){$query[$pair[1]]=[string]$value[$pair[0]]}}
        if($value.ContainsKey('type') -and $value.type -notin @('','none')){throw 'Profile import: unsupported VMess header camouflage.'}
        if($value.ContainsKey('tls') -and $value.tls){if($value.tls -ne 'tls'){throw 'Profile import: unsupported VMess security.'};$query.security='tls'}
        Set-ProfileLinkTransport $proxy $query
        if(!$name -and $value.ContainsKey('ps')){$name=[string]$value.ps}
        return @{name=$name;proxy=(Convert-SplitNode $proxy)}
    }
    if($scheme -eq 'ssr'){
        $decoded=Convert-ProfileBase64 $body;$parts=$decoded.Split([string[]]@('/?'),[StringSplitOptions]::None)
        if($parts.Count -gt 2 -or $parts[0] -notmatch '^(.+):(\d+):([^:]+):([^:]+):([^:]+):([^:]+)$'){throw 'Profile import: invalid ShadowsocksR link.'}
        $proxy=@{type='ssr';server=$Matches[1];port=$Matches[2];protocol=$Matches[3];cipher=$Matches[4];obfs=$Matches[5];password=(Convert-ProfileBase64 $Matches[6]);udp=$true}
        if($parts.Count -eq 2){$query=Read-ProfileQuery $parts[1] @('obfsparam','protoparam','remarks','group');foreach($pair in @(@('obfsparam','obfs-param'),@('protoparam','protocol-param'))){if($query.ContainsKey($pair[0])){$proxy[$pair[1]]=Convert-ProfileBase64 $query[$pair[0]]}};if(!$name -and $query.ContainsKey('remarks')){$name=Convert-ProfileBase64 $query.remarks}}
        return @{name=$name;proxy=(Convert-SplitNode $proxy)}
    }
    $queryText='';if($body.Contains('?')){$parts=$body.Split([char[]]@('?'),2);$body=$parts[0];$queryText=$parts[1]}
    if($scheme -eq 'ss' -and !$body.Contains('@')){$body=Convert-ProfileBase64 $body}
    $at=$body.LastIndexOf('@');$user=if($at -ge 0){Decode-ProfileUri $body.Substring(0,$at)}else{''};$endpoint=if($at -ge 0){$body.Substring($at+1)}else{$body}
    $endpoint=$endpoint.TrimEnd('/');if($endpoint.Contains('/')){throw 'Profile import: unexpected share-link path.'}
    $portDefault=switch($scheme){'http'{80};'https'{443};'socks5'{1080};'socks5s'{1080};'hysteria2'{443};'hy2'{443};'anytls'{443};default{0}}
    if($endpoint -match '^\[([^\]]+)\](?::(\d+))?$'){$server=$Matches[1];$port=if($Matches[2]){$Matches[2]}else{$portDefault}}
    elseif($endpoint -match '^([^:]+)(?::(\d+))?$'){$server=$Matches[1];$port=if($Matches[2]){$Matches[2]}else{$portDefault}}
    else{throw 'Profile import: invalid share-link endpoint.'}
    $type=switch($scheme){'https'{'http'};'socks5s'{'socks5'};'hy2'{'hysteria2'};default{$scheme}}
    $proxy=@{type=$type;server=$server;port=$port}
    $common=@('sni','peer','servername','insecure','allowinsecure','allow_insecure','skip-cert-verify','alpn','fp')
    switch($type){
        {$_ -in @('vless','trojan')} {
            $query=Read-ProfileQuery $queryText ($common+@('security','type','network','host','path','servicename','flow','encryption','pbk','sid','mode'))
            if($type -eq 'vless'){$proxy.uuid=$user}else{$proxy.password=$user}
            Set-ProfileLinkTransport $proxy $query
        }
        'ss' {
            $query=Read-ProfileQuery $queryText @('plugin','udp-over-tcp')
            if(!$user.Contains(':')){$user=Convert-ProfileBase64 $user};$parts=$user.Split([char[]]@(':'),2);if($parts.Count -ne 2){throw 'Profile import: invalid Shadowsocks credentials.'};$proxy.cipher=$parts[0];$proxy.password=$parts[1]
            if($query.ContainsKey('udp-over-tcp')){$proxy['udp-over-tcp']=Convert-ProfileBoolean $query['udp-over-tcp']}
            if($query.ContainsKey('plugin')){
                $parts=$query.plugin.Split(';');$plugin=$parts[0];if($plugin -eq 'obfs-local'){$plugin='obfs'};$proxy.plugin=$plugin;$opts=@{}
                foreach($part in $parts|Select-Object -Skip 1){$pair=$part.Split([char[]]@('='),2);$key=switch($pair[0]){'obfs'{'mode'};'obfs-host'{'host'};default{$pair[0]}};if($opts.ContainsKey($key)){throw 'Profile import: duplicate plugin option.'};$opts[$key]=if($pair.Count -eq 2){$pair[1]}else{$true}}
                $proxy['plugin-opts']=$opts
            }
        }
        'hysteria' {
            $query=Read-ProfileQuery $queryText ($common+@('auth','upmbps','downmbps','up','down','obfs','protocol'))
            foreach($pair in @(@('auth','auth-str'),@('upmbps','up'),@('downmbps','down'),@('up','up'),@('down','down'),@('obfs','obfs'),@('protocol','protocol'))){if($query.ContainsKey($pair[0])){$proxy[$pair[1]]=$query[$pair[0]]}}
            if($user){$proxy['auth-str']=$user};Set-ProfileLinkTransport $proxy $query
        }
        'hysteria2' {
            $query=Read-ProfileQuery $queryText ($common+@('obfs','obfs-password','mport','up','down'));$proxy.password=$user
            foreach($key in @('obfs','obfs-password','up','down')){if($query.ContainsKey($key)){$proxy[$key]=$query[$key]}};if($query.ContainsKey('mport')){$proxy.ports=$query.mport};Set-ProfileLinkTransport $proxy $query
        }
        'tuic' {
            $query=Read-ProfileQuery $queryText ($common+@('congestion_control','udp_relay_mode','disable_sni','reduce_rtt','heartbeat_interval'))
            $parts=$user.Split([char[]]@(':'),2);if($parts.Count -eq 2){$proxy.uuid=$parts[0];$proxy.password=$parts[1]}else{$proxy.token=$user}
            foreach($pair in @(@('congestion_control','congestion-controller'),@('udp_relay_mode','udp-relay-mode'),@('heartbeat_interval','heartbeat-interval'))){if($query.ContainsKey($pair[0])){$proxy[$pair[1]]=$query[$pair[0]]}}
            foreach($pair in @(@('disable_sni','disable-sni'),@('reduce_rtt','reduce-rtt'))){if($query.ContainsKey($pair[0])){$proxy[$pair[1]]=Convert-ProfileBoolean $query[$pair[0]]}};Set-ProfileLinkTransport $proxy $query
        }
        'anytls' {$query=Read-ProfileQuery $queryText $common;$proxy.password=$user;Set-ProfileLinkTransport $proxy $query}
        {$_ -in @('http','socks5')} {
            if($queryText){throw 'Profile import: HTTP and SOCKS share links do not accept global parameters.'}
            if($user){$parts=$user.Split([char[]]@(':'),2);$proxy.username=$parts[0];if($parts.Count -eq 2){$proxy.password=$parts[1]}}
            if($scheme -in @('https','socks5s')){$proxy.tls=$true}
        }
        default {throw 'Profile import: unsupported share-link scheme.'}
    }
    return @{name=$name;proxy=(Convert-SplitNode $proxy)}
}

function Read-ProfileYamlScalar([string]$Value) {
    $value=$Value.Trim()
    if($value.StartsWith('"') -or $value.StartsWith('[') -or $value.StartsWith('{')){return Read-ProfileJson $value}
    if($value.StartsWith("'")){if(!$value.EndsWith("'") -or $value.Length -lt 2){throw 'Profile import: invalid YAML quoted value.'};return $value.Substring(1,$value.Length-2).Replace("''","'")}
    if($value -match '^[&*!|>]' -or $value.Contains(' #') -or $value -match '^[\[\]{}]'){throw 'Profile import: YAML aliases, tags, blocks and non-JSON flow syntax are unsupported.'}
    if($value -eq 'true'){return $true};if($value -eq 'false'){return $false};if($value -in @('null','~')){return $null}
    if($value -match '^-?(?:0|[1-9][0-9]*)$'){try{return [long]$value}catch{throw 'Profile import: YAML integer exceeds its limit.'}}
    return $value
}

function Read-ProfileYamlBlock($Lines,[ref]$Index,[int]$Indent,[int]$Depth=0) {
    if($Depth -gt 12){throw 'Profile import: YAML nesting exceeds its limit.'}
    $sequence=$Lines[$Index.Value].text -match '^-(?: |$)';$result=if($sequence){New-Object Collections.Generic.List[object]}else{@{}}
    while($Index.Value -lt $Lines.Count -and $Lines[$Index.Value].indent -eq $Indent){
        $line=$Lines[$Index.Value].text
        if($sequence){
            if($line -notmatch '^-(?: +(.*))?$'){throw 'Profile import: mixed YAML containers are unsupported.'};$value=$Matches[1]
            if($value -match '^[A-Za-z][A-Za-z0-9_-]*:'){$Lines[$Index.Value]=@{indent=$Indent+2;text=$value};$item=Read-ProfileYamlBlock $Lines $Index ($Indent+2) ($Depth+1)}
            else{$Index.Value++;if(!$value){if($Index.Value -ge $Lines.Count -or $Lines[$Index.Value].indent -ne $Indent+2){throw 'Profile import: empty YAML item.'};$item=Read-ProfileYamlBlock $Lines $Index ($Indent+2) ($Depth+1)}else{$item=Read-ProfileYamlScalar $value}}
            $result.Add($item);if($result.Count -gt 128){throw 'Profile import: YAML list exceeds its limit.'}
        }else{
            if($line -notmatch '^([A-Za-z][A-Za-z0-9_-]{0,63}):(?: +(.*))?$'){throw 'Profile import: unsupported YAML mapping syntax.'};$key=$Matches[1];$value=if($Matches.ContainsKey(2)){$Matches[2]}else{''}
            if($result.ContainsKey($key)){throw 'Profile import: duplicate YAML option.'};$Index.Value++
            if(!$value){if($Index.Value -ge $Lines.Count -or $Lines[$Index.Value].indent -ne $Indent+2){throw 'Profile import: empty YAML option.'};$result[$key]=Read-ProfileYamlBlock $Lines $Index ($Indent+2) ($Depth+1)}else{$result[$key]=Read-ProfileYamlScalar $value}
            if($result.Count -gt 128){throw 'Profile import: YAML object exceeds its limit.'}
        }
        if($Index.Value -lt $Lines.Count -and $Lines[$Index.Value].indent -gt $Indent){throw 'Profile import: YAML indentation must use two-space levels.'}
    }
    if($sequence){return ,$result.ToArray()};return $result
}

function Read-ProfileYaml([string]$Text) {
    $lines=New-Object Collections.Generic.List[object]
    foreach($line in $Text -split '\r?\n'){
        if($line -match '\t'){throw 'Profile import: YAML tabs are unsupported.'};if(!$line.Trim() -or $line.TrimStart().StartsWith('#')){continue}
        if($line -match '^\s*(---|\.\.\.|%|<<:)'){throw 'Profile import: YAML documents and merge directives are unsupported.'}
        $indent=$line.Length-$line.TrimStart(' ').Length;if($indent%2 -or $indent -gt 24){throw 'Profile import: YAML indentation must use two-space levels.'};$lines.Add(@{indent=$indent;text=$line.Trim()})
    }
    if(!$lines.Count -or $lines.Count -gt 8192 -or $lines[0].indent -ne 0){throw 'Profile import: invalid or excessively large YAML.'}
    $index=0;$value=Read-ProfileYamlBlock $lines ([ref]$index) 0
    if($index -ne $lines.Count){throw 'Profile import: mixed YAML roots are unsupported.'};return $value
}

function Convert-ProfileOpenVpn([string]$Text) {
    $proxy=@{type='openvpn';dev='tun';udp=$true};$lines=@($Text -split '\r?\n');$remote=$false;$authRequired=$false
    for($n=0;$n -lt $lines.Count;$n++){
        $line=$lines[$n].Trim();if(!$line -or $line.StartsWith('#') -or $line.StartsWith(';')){continue}
        if($line -match '^<(ca|cert|key|tls-auth|tls-crypt|tls-crypt-v2|auth-user-pass)>$'){
            $tag=$Matches[1];if($proxy.ContainsKey($tag)){throw 'Profile import: duplicate OpenVPN inline option.'};$block=New-Object Collections.Generic.List[string];$closed=$false
            for($n++;$n -lt $lines.Count;$n++){if($lines[$n].Trim() -eq ('</'+$tag+'>')){$closed=$true;break};$block.Add($lines[$n])}
            if(!$closed){throw 'Profile import: unterminated OpenVPN inline option.'}
            if($tag -eq 'auth-user-pass'){if($block.Count -ne 2){throw 'Profile import: inline OpenVPN authentication requires username and password.'};$proxy.username=$block[0];$proxy.password=$block[1]}else{$proxy[$tag]=($block -join "`n")+"`n"};continue
        }
        $parts=@($line -split '\s+');$key=$parts[0]
        switch($key){
            'remote' {if($remote -or $parts.Count -notin @(3,4)){throw 'Profile import: OpenVPN requires exactly one explicit remote and port.'};$remote=$true;$proxy.server=$parts[1];$proxy.port=$parts[2];if($parts.Count -eq 4){$proxy.proto=$parts[3]}}
            'auth-user-pass' {if($parts.Count -ne 1){throw 'Profile import: external OpenVPN authentication files are unsupported.'};$authRequired=$true}
            'remote-cert-tls' {if($parts.Count -ne 2 -or $parts[1] -ne 'server'){throw 'Profile import: unsupported OpenVPN certificate role.'}}
            {$_ -in @('client','tls-client','nobind','persist-key','persist-tun')} {if($parts.Count -ne 1){throw 'Profile import: invalid OpenVPN client directive.'}}
            'verb' {if($parts.Count -ne 2 -or $parts[1] -notmatch '^[0-9]$'){throw 'Profile import: invalid OpenVPN verbosity.'}}
            {$_ -in @('proto','dev','cipher','data-ciphers','data-ciphers-fallback','auth','comp-lzo','key-direction','ping','ping-restart','tran-window','handshake-timeout','tun-mtu')} {
                if($parts.Count -ne 2){throw 'Profile import: invalid OpenVPN directive.'};$target=if($key -eq 'tun-mtu'){'mtu'}else{$key}
                if($proxy.ContainsKey($target) -and $target -ne 'dev'){throw 'Profile import: duplicate OpenVPN option.'}
                $proxy[$target]=if($key -eq 'data-ciphers'){@($parts[1].Split(':'))}elseif($key -in @('ping','ping-restart','tran-window','handshake-timeout','tun-mtu')){if($parts[1] -notmatch '^\d{1,8}$'){throw 'Profile import: invalid OpenVPN numeric directive.'};[int]$parts[1]}else{$parts[1]}
            }
            default {throw 'Profile import: unsupported OpenVPN directive, file reference or script.'}
        }
    }
    if(!$remote -or ($authRequired -and !$proxy.ContainsKey('username'))){throw 'Profile import: OpenVPN remote or inline authentication is missing.'}
    return Convert-SplitNode $proxy
}

function Convert-SplitProfileText([string]$Text,[string]$Format='auto',[string]$Name='Server') {
    Assert-ProfileText $Text
    if(!$Name){$Name='Server'};$text=$Text.Trim().TrimStart([char]0xfeff);$format=$Format.ToLowerInvariant()
    $nodes=New-Object Collections.Generic.List[object]
    try{
        if($format -eq 'auto'){
            $format=if($text -match '^\[Interface\]'){'conf'}elseif($text -match '^[\[{]'){'json'}elseif($text -match '^[A-Za-z0-9]+://'){'uri'}elseif($text -match '(?m)^\s*(client|<ca>|<cert>|remote\s)'){'ovpn'}elseif($text -match '^[A-Za-z][A-Za-z0-9_-]*:|^- '){'yaml'}else{'base64'}
        }
        switch($format){
            'conf' {$proxy=Convert-AwgProfileText $text;$nodes.Add(@{name=$Name;proxy=$proxy})}
            'ovpn' {$nodes.Add(@{name=$Name;proxy=(Convert-ProfileOpenVpn $text)})}
            'uri' {foreach($line in $text -split '\r?\n'){if(!$line.Trim()){continue};$nodes.Add((Convert-ProfileLink $line.Trim()));if($nodes.Count -gt 128){throw 'Profile import: node count exceeds 128.'}}}
            'base64' {
                $decoded=Convert-ProfileBase64 ($text -replace '\s','')
                if($decoded -notmatch '^\s*[A-Za-z0-9]+://'){throw 'Profile import: base64 subscription must contain share links.'}
                foreach($node in @(Convert-SplitProfileText $decoded 'uri' $Name)){$nodes.Add($node)}
            }
            {$_ -in @('json','node','yaml','yml')} {
                $value=if($format -in @('yaml','yml')){Read-ProfileYaml $text}else{Read-ProfileJson $text}
                if($value -is [System.Collections.IDictionary] -and $value.ContainsKey('schema') -and $value.ContainsKey('proxy')){
                    if($value.schema -ne 1 -or @($value.Keys|Where-Object {$_ -notin @('schema','proxy')}).Count){throw 'Profile import: invalid normalized profile envelope.'};$value=$value.proxy
                }
                if($value -is [System.Collections.IDictionary] -and $value.ContainsKey('proxies')){if($value.Count -ne 1 -or $value.proxies -isnot [array]){throw 'Profile import: only a standalone proxies collection is accepted; global settings are unsupported.'};$value=$value.proxies}
                foreach($item in @($value)){
                    if($item -isnot [System.Collections.IDictionary]){throw 'Profile import: expected outgoing node objects.'}
                    $display=if($item.ContainsKey('name')){[string]$item.name}else{$Name};$nodes.Add(@{name=$display;proxy=(Convert-SplitNode $item)})
                }
            }
            default {throw 'Profile import: unsupported input format.'}
        }
        if(!$nodes.Count -or $nodes.Count -gt 128){throw 'Profile import: node count must be between 1 and 128.'}
        foreach($node in $nodes){
            if(!$node.name){$node.name=$Name};$node.name=$node.name.Trim()
            if(!$node.name -or $node.name.Length -gt 80 -or $node.name -match '[\x00-\x1f]'){throw 'Profile import: server name must contain 1 to 80 printable characters.'}
            $node.protocol=if($node.proxy.type -eq 'wireguard' -and $node.proxy.ContainsKey('amnezia-wg-option')){'amneziawg'}else{$node.proxy.type}
            # Match the persisted envelope serialization, so an accepted node
            # cannot become unreadable after indentation or JSON escaping.
            $envelope=@{schema=1;proxy=$node.proxy}|ConvertTo-Json -Depth 40
            if([Text.Encoding]::UTF8.GetByteCount($envelope) -gt 524288){throw 'Profile import: normalized profile exceeds its storage limit.'}
            $node
        }
    }catch{
        if($_.Exception.Message.StartsWith('Profile import:',[StringComparison]::Ordinal)){throw}
        # Never surface JSON, URI, certificate, key or command text from parser errors.
        throw 'Profile import: malformed input or incompatible protocol options.'
    }
}

Export-ModuleMember -Function Convert-SplitProfileText,Convert-SplitNode,Convert-AwgProfileText,Get-SplitProfileCapabilities
