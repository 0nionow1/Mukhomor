param([switch]$SkipCore)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$passed=0
function Assert($Condition,[string]$Message) { if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++ }
function Reject([scriptblock]$Action,[string]$Message) { $rejected=$false;try{& $Action|Out-Null}catch{$rejected=$true};Assert $rejected $Message }
$fixture=Join-Path $PSScriptRoot ('full-routing-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
Set-OwnedTestRoot $fixture $fixture
Initialize-SplitRoot $fixture
$oldCore=$env:MUKHOMOR_CORE_PATH
try {
    if(!$SkipCore){$env:MUKHOMOR_CORE_PATH=Get-CorePath $package}
    # Public deterministic fixture credential, used for configuration parsing only.
    # Full routing must also validate in a clean checkout with stock mihomo.
    # Mycelium compatibility is exercised by its separate optional-core tests.
    $proxy=@{name='AWG';type='trojan';server='vpn.fixture.invalid';port=443;password='fixture-password';sni='tls.fixture.invalid';udp=$true}
    Write-AtomicJson (Join-Path $fixture 'private\selected-profile.json') @{schema=1;proxy=$proxy}
    $transport=@{server=$proxy.server;port=443;addresses=@('192.0.2.1','2001:db8::1');selected_address='192.0.2.1';interface_name='Fixture Ethernet'}
    $settings=Get-SplitSettings $fixture
    $legacy=Convert-ToMap $settings;$legacy.Remove('routing')
    $migrated=Get-SplitSettings $fixture $legacy
    Assert ($migrated.routing.mode -ceq 'split' -and $migrated.routing.allow_lan -and !$legacy.ContainsKey('routing')) 'legacy settings migrate without modifying the caller'
    foreach($invalid in @(@{mode='FULL';allow_lan=$true},@{mode='full';allow_lan='true'},@{mode='full';allow_lan=$false;unknown=$true},'full')) {
        $bad=Convert-ToMap $settings;$bad.routing=$invalid
        Reject {Get-SplitSettings $fixture $bad} 'malformed routing policy is rejected'
    }
    $settings.routing=@{mode='full';allow_lan=$false}
    $settings.direct.domains=@(1..40|ForEach-Object {'saved'+$_+'.example'})
    $settings.direct.ip_cidrs=@('8.8.8.8/32','2606:4700::/32')
    $settings.dns_update.enabled=$true
    $config=New-CoreConfig $fixture $settings $false $transport
    Assert ($config.rules.Count -eq 1 -and $config.rules[0] -ceq 'MATCH,AWG') 'full routing without LAN access has no direct exceptions'
    Assert ($config['rule-providers'].Count -eq 0 -and $config['find-process-mode'] -ceq 'off') 'full mode omits saved providers and process scanning'
    Assert ($config.hosts[$proxy.server] -eq '192.0.2.1' -and $config.proxies[0].server -ceq $proxy.server -and $config.proxies[0].sni -ceq 'tls.fixture.invalid') 'endpoint IP is pinned without changing TLS identity'
    Assert ($config.proxies[0]['interface-name'] -ceq $transport.interface_name) 'outer socket keeps the selected interface'
    Assert ($config.tun['route-exclude-address'].Count -eq 2 -and $config.tun['route-exclude-address'] -contains '2001:db8::1/128') 'only exact resolved endpoint addresses are excluded from TUN'
    Assert (!$config.tun.enable -and $config.dns['use-hosts'] -and !$config.dns['use-system-hosts']) 'fixture disables TUN and full DNS ignores system hosts'
    foreach($field in @('nameserver','default-nameserver','proxy-server-nameserver','direct-nameserver')) {
        Assert ($config.dns[$field].Count -eq 2 -and @($config.dns[$field]|Where-Object {$_ -cnotmatch '^https://(1\.1\.1\.1|8\.8\.8\.8)/dns-query#AWG$'}).Count -eq 0) ('all '+$field+' DNS paths use explicit tunneled HTTPS resolvers')
    }
    Reject {New-CoreConfig $fixture $settings $false} 'hostname endpoint without a resolved session cannot bootstrap through plaintext DNS'
    $badTransport=Convert-ToMap $transport;$badTransport.selected_address='198.51.100.9'
    Reject {New-CoreConfig $fixture $settings $false $badTransport} 'selected endpoint must belong to the resolved endpoint set'
    $settings.routing.allow_lan=$true
    $lan=New-CoreConfig $fixture $settings $false $transport
    Assert ($lan.rules -contains 'IP-CIDR,192.168.0.0/16,DIRECT,no-resolve' -and $lan.rules -contains 'IP-CIDR6,fc00::/7,DIRECT,no-resolve') 'LAN opt-in permits explicit private IPv4 and IPv6 ranges'
    Assert (@($lan.rules|Where-Object {$_ -match '100\.64|PROCESS-|DOMAIN|RULE-SET|8\.8\.8\.8|2606:4700'}).Count -eq 0) 'LAN opt-in excludes CGNAT and all saved user bypasses'
    $settings.routing.allow_lan=$false
    Write-AtomicJson (Join-Path $fixture 'settings.json') $settings
    $dnsResult=Update-SplitDns $fixture -Force -Resolver {throw 'Full mode must not call exception DNS resolver'}
    Assert ($dnsResult.disabled -and $dnsResult.hosts -eq 0 -and $dnsResult.api_errors -eq 0) 'full mode disables scheduled bypass DNS refresh'
    $providerResult=Update-SplitRuleSets $fixture
    Assert ($providerResult.disabled -and $providerResult.updated -eq 0 -and $providerResult.api_errors -eq 0) 'full mode disables saved split provider refresh'
    if(!$SkipCore) {
        foreach($candidate in @($config,$lan)) {
            $candidatePath=Join-Path $fixture 'private\validation.json'
            Write-AtomicJson $candidatePath $candidate
            Test-CoreConfig $fixture $candidatePath
            Assert $true 'real released core validates full-mode routing and tunneled DNS configuration'
        }
        Set-SplitConfig $fixture $settings $false -Transport $transport
        $beforeSettings=[IO.File]::ReadAllText((Join-Path $fixture 'settings.json'))
        $beforeConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))
        foreach($field in @('mode','allow_lan')) {
            $changed=Convert-ToMap $settings
            if($field -eq 'mode'){$changed.routing.mode='split'}else{$changed.routing.allow_lan=$true}
            Reject {Set-SplitConfig $fixture $changed $false -Reload -Transport $transport} 'routing policy changes require disconnect before reload'
            Assert ([IO.File]::ReadAllText((Join-Path $fixture 'settings.json')) -ceq $beforeSettings -and [IO.File]::ReadAllText((Join-Path $fixture 'private\config.json')) -ceq $beforeConfig) 'rejected live policy change leaves settings and core configuration intact'
        }
    }
    'PASS: '+$passed+' full-routing assertions; isolated fixture '+$fixture
    'Configuration checks only: no system routes, DNS, firewall, trust store or running VPN were changed.'
} finally {$env:MUKHOMOR_CORE_PATH=$oldCore}
