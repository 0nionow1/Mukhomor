Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('transport-'+[guid]::NewGuid().ToString('N'))
$originalCorePath=$env:MUKHOMOR_CORE_PATH
[IO.Directory]::CreateDirectory((Join-Path $fixture 'bin')) | Out-Null
[IO.Directory]::CreateDirectory((Join-Path $fixture 'runtime')) | Out-Null
$corePath=Join-Path $fixture 'bin\mihomo-windows-amd64-compatible.exe'
[IO.File]::WriteAllText($corePath,'not an executable; path validation fixture only')
try {
    & $module {
        param($fixture,$corePath)
        $script:ExpectedProgram=$corePath
        $script:FixtureStart=[datetime]'2026-01-01T00:00:00Z'
        $script:PathFirewallName=''
        $script:PathRuleExists=$false
        $script:PathFirewallCalls=0
        function script:Get-Process {
            param($Id,$ErrorAction)
            if ($Id -eq 42424) { [pscustomobject]@{Id=42424;Path=$script:ExpectedProgram;StartTime=$script:FixtureStart} }
        }
        function script:Get-NetFirewallRule {param($Name,$ErrorAction) if ($script:PathRuleExists) {[pscustomobject]@{Name=$Name}}}
        function script:Assert-ApplicationPath([string]$Program) {
            # MS-FASP application grammar. Filesystem-legal slash/prefix paths
            # are not legal firewall application filter strings.
            if ($Program.Length -lt 1 -or $Program.Length -gt 259 -or $Program -match '[/\*\?"<>|]') { throw 'Application contains invalid characters or has an invalid length' }
            if ($Program -cne $script:ExpectedProgram) { throw 'Normalization changed the scoped executable' }
        }
        function script:New-NetFirewallRule {
            param($Name,$Enabled,$Direction,$Action,$Profile,$Program,$InterfaceAlias,$Protocol,$EdgeTraversalPolicy,$DisplayName,$Group)
            Assert-ApplicationPath $Program
            if ($InterfaceAlias -ne 'Mukhomor' -or $Direction -ne 'Inbound' -or $Action -ne 'Allow' -or $EdgeTraversalPolicy -ne 'Block') { throw 'Normalization broadened firewall scope' }
            $script:PathFirewallName=$Name; $script:PathFirewallCalls++
        }
        function script:Set-NetFirewallRule {
            param($Name,$Enabled,$Direction,$Action,$Profile,$Program,$InterfaceAlias,$Protocol,$EdgeTraversalPolicy)
            Assert-ApplicationPath $Program
            if ($Name -ne $script:PathFirewallName -or $InterfaceAlias -ne 'Mukhomor' -or $Direction -ne 'Inbound' -or $Action -ne 'Allow' -or $EdgeTraversalPolicy -ne 'Block') { throw 'Normalization changed firewall rule identity or scope' }
            $script:PathFirewallCalls++
        }
        Write-AtomicJson (Join-Path $fixture 'runtime\session.json') @{pid=42424;started=$script:FixtureStart.ToUniversalTime().ToString('o')}
        $mixed=$corePath.Replace('\bin\','\bin/')
        $rejected=$false
        try { Assert-ApplicationPath $mixed } catch { $rejected=$true }
        if (!$rejected) { throw 'The fixture did not reproduce the production mixed-separator rejection' }
        'PASS: actual filesystem-legal native bridge path shape fails the official firewall application grammar'
        foreach ($path in @($corePath,$mixed,('\\?\'+$corePath),('\\?\'+$mixed))) {
            $env:MUKHOMOR_CORE_PATH=$path
            if ((Get-CorePath $fixture) -cne $corePath) { throw 'Core path was not normalized consistently' }
            if ((Get-OwnedCore $fixture).Id -ne 42424) { throw 'Equivalent native bridge path failed exact core ownership recognition' }
            Set-SplitTunFirewall $fixture
            $script:PathRuleExists=$true
        }
        if ($script:PathFirewallCalls -ne 4) { throw 'Equivalent paths did not create/repair the same exact-scoped rule' }
        'PASS: raw native bridge environment paths normalize for firewall create/repair and exact owned-core recognition'
        if ((Convert-SplitProgramPath '\\?\UNC\fixture.invalid\share\bin/mihomo.exe') -cne '\\fixture.invalid\share\bin\mihomo.exe') { throw 'Extended UNC conversion changed target identity' }
        foreach ($path in @('\\?\GLOBALROOT\Device\fixture','\\.\fixture')) {
            $failed=$false
            try { Convert-SplitProgramPath $path | Out-Null } catch { $failed=$true }
            if (!$failed) { throw 'Unsupported device namespace was widened into a filesystem path' }
        }
        'PASS: supported extended UNC preserves its target; other device namespaces are refused without file or network access'
    } $fixture $corePath
} finally {
    $env:MUKHOMOR_CORE_PATH=$originalCorePath
    if (![IO.Path]::GetFullPath($fixture).StartsWith(([IO.Path]::GetFullPath($PSScriptRoot)+'\'),[StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture cleanup escaped tests directory' }
    Remove-Item -LiteralPath $fixture -Recurse -Force
}
# Mock the endpoint and route inventory; never query/change the actual network.
& $module {
    function script:Get-SelectedSplitProxy {param($Root) @{server='vpn.example.test';port=443}}
    function script:Find-NetRoute {
        param($RemoteIPAddress,$ErrorAction)
        # Mimic Find-NetRoute returning both address and route objects.
        [pscustomobject]@{IPAddress='192.0.2.123';InterfaceIndex=7}
        [pscustomobject]@{NextHop='192.0.2.254';InterfaceIndex=$(if ($script:TunnelOnly) {12} else {7})}
    }
    function script:Get-NetAdapter {
        param($InterfaceIndex,$ErrorAction)
        [pscustomobject]@{Name=$(if ($InterfaceIndex -eq 12) {'Old VPN'} else {'Fixture Ethernet'});InterfaceDescription=$(if ($InterfaceIndex -eq 12) {'WireGuard Tunnel'} else {'Fixture NIC'});Status='Up';InterfaceGuid=[guid]'11111111-1111-1111-1111-111111111111'}
    }
    $resolver={param($Server) [Net.IPAddress]::Parse('2001:db8::1'); [Net.IPAddress]::Parse('192.0.2.1'); [Net.IPAddress]::Parse('192.0.2.2')}
    $script:TunnelOnly=$false
    $t=Get-SplitTransport 'fixture' $resolver
    if ($t.interface_name -ne 'Fixture Ethernet' -or $t.selected_address -ne '192.0.2.1' -or $t.addresses.Count -ne 3) {throw 'Endpoint resolution/binding failed'}
    'PASS: domain endpoint is resolved before TUN; IPv4 preferred; full A/AAAA set retained'
    $script:TunnelOnly=$true; $failed=$false
    try {Get-SplitTransport 'fixture' $resolver | Out-Null} catch {$failed=$true}
    if (!$failed) {throw 'Outer transport was bound to another VPN'}
    'PASS: transport refuses another WireGuard adapter instead of nesting tunnels'
    $failed=$false
    try {Get-SplitTransport 'fixture' {param($Server)} | Out-Null} catch {$failed=$true}
    if (!$failed) {throw 'Empty endpoint resolution was accepted'}
    'PASS: empty endpoint resolution fails before routing is changed'
    # Check both firewall paths with exact binary/TUN scope, never actual rules.
    function script:Get-CorePath {param($Root) 'D:\Fixture\mihomo.exe'}
    function script:Get-NetFirewallRule {param($Name,$ErrorAction) if ($script:RuleExists) {[pscustomobject]@{Name=$Name}}}
    function script:New-NetFirewallRule {
        param($Name,$Enabled,$Direction,$Action,$Profile,$Program,$InterfaceAlias,$Protocol,$EdgeTraversalPolicy,$DisplayName,$Group)
        if ($Program -ne 'D:\Fixture\mihomo.exe' -or $InterfaceAlias -ne 'Mukhomor' -or $Direction -ne 'Inbound' -or $EdgeTraversalPolicy -ne 'Block' -or $Action -ne 'Allow') {throw 'Firewall permission is not scoped to our binary and TUN'}
        $script:FirewallName=$Name
    }
    function script:Set-NetFirewallRule {
        param($Name,$Enabled,$Direction,$Action,$Profile,$Program,$InterfaceAlias,$Protocol,$EdgeTraversalPolicy)
        if ($Name -ne $script:FirewallName -or $Program -ne 'D:\Fixture\mihomo.exe' -or $InterfaceAlias -ne 'Mukhomor') {throw 'Existing rule repair changed scope'}
        $script:RuleRepaired=$true
    }
    $script:RuleExists=$false; Set-SplitTunFirewall 'fixture'
    $script:RuleExists=$true; $script:RuleRepaired=$false; Set-SplitTunFirewall 'fixture'
    if (!$script:RuleRepaired) {throw 'Existing firewall rule was not repaired'}
    'PASS: firewall rule creation/repair allows only the exact core binary on Mukhomor'
}
