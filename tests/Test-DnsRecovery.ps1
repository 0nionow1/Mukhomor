Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
$module=Get-Module SplitWindows
$fixture=Join-Path $PSScriptRoot ('dns-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $fixture 'runtime')) | Out-Null
# All adapter, registry and netsh calls below are mocked inside the module.
# No real network configuration is read or written by this test.
& $module {
    $script:Adapter=[pscustomobject]@{InterfaceGuid=[guid]'11111111-1111-1111-1111-111111111111';ifIndex=42;Status='Up';Name='Fixture'}
    $script:Calls=New-Object 'Collections.Generic.List[object]'
    $script:FailIpv6=$false
    $script:FailRestore=$false
    $script:SetupError=$null
    function script:Get-NetAdapter { param([switch]$IncludeHidden) return $script:Adapter }
    function script:Get-NetRoute { param($InterfaceIndex,$ErrorAction) return [pscustomobject]@{DestinationPrefix='0.0.0.0/0'} }
    function script:Get-ItemProperty { param($LiteralPath,$ErrorAction) return [pscustomobject]@{NameServer=$(if ($LiteralPath -match 'Tcpip6') {''} else {'8.8.8.8,8.8.4.4'})} }
    function script:Invoke-NetshDns {
        param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds)
        $script:Calls.Add(@{index=$Index;family=$Family;source=$Source;servers=@($Servers)})
        if ($script:FailIpv6 -and $Family -eq 'ipv6' -and $Source -eq 'static') {
            $script:FailIpv6=$false
            if ($script:SetupError) { throw $script:SetupError }
            throw 'Simulated IPv6 setting failure'
        }
        if ($script:FailRestore) { throw 'Fixture rollback detail must not replace or annotate the original error' }
    }
}
try {
& $module {
    param($fixture)
    Set-SessionDns $fixture
    if($script:Calls[0].servers[0] -ne '198.18.0.2' -or $script:Calls[1].servers[0] -ne 'fdfe:dcba:9876::2'){throw 'DNS must use the Mihomo peer, not the TUN adapter local address'}
    'PASS: Windows DNS targets the IPv4/IPv6 TUN peers instead of local adapter addresses'
    if (!(Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json'))) { throw 'DNS snapshot missing' }
    $backup=Read-JsonFile (Join-Path $fixture 'runtime\dns-backup.json')
    if ($backup.adapters.Count -ne 2 -or $backup.adapters[0].servers.Count -ne 2) { throw 'Manual DNS backup lost servers' }
    $script:Adapter.ifIndex=77
    Restore-SessionDns $fixture
    if (Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json')) { throw 'Completed backup was not cleared' }
    $restore=$script:Calls[$script:Calls.Count-2]
    if ($restore.index -ne 77 -or $restore.source -ne 'static' -or $restore.servers[1] -ne '8.8.4.4') { throw 'Restore ignored GUID or manual server list' }
    if ($script:Calls[$script:Calls.Count-1].source -ne 'dhcp') { throw 'Automatic IPv6 DNS was not restored' }
    'PASS: complete DNS snapshot; manual and automatic settings restored by adapter GUID'
    $script:FailIpv6=$true
    $failed=$false
    try { Set-SessionDns $fixture } catch { $failed=$true }
    if (!$failed -or (Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json'))) { throw 'Partial DNS change did not roll back' }
    'PASS: partial DNS change rolls back both families'
    Set-SessionDns $fixture
    $script:SavedAdapter=$script:Adapter; $script:Adapter=$null
    $failed=$false
    try { Restore-SessionDns $fixture } catch { $failed=$true }
    if (!$failed -or !(Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json'))) { throw 'Unavailable-adapter recovery backup was lost' }
    $script:Adapter=$script:SavedAdapter
    Restore-SessionDns $fixture
    if (Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json')) { throw 'Deferred recovery failed' }
    'PASS: unavailable adapter keeps recovery data until reconnected'
    $original=New-Object Management.Automation.ErrorRecord (
        (New-Object InvalidOperationException 'Original DNS setup failure'),
        'FixtureDnsSetup',
        [Management.Automation.ErrorCategory]::InvalidOperation,
        'Fixture adapter'
    )
    $script:SetupError=$original
    $script:FailIpv6=$true
    # Allow the first setup call, then fail every recovery call. The actual
    # Restore-SessionDns transaction retains the failed rows in its JSON.
    $script:FailRestore=$false
    function script:Invoke-NetshDns {
        param($Index,$Family,$Source,$Servers,$TimeoutMilliseconds)
        $script:Calls.Add(@{index=$Index;family=$Family;source=$Source;servers=@($Servers)})
        $isSetup=$Source -eq 'static' -and $Servers[0] -in @('198.18.0.2','fdfe:dcba:9876::2')
        if ($isSetup -and $Family -eq 'ipv6') { throw $script:SetupError }
        if (!$isSetup -and $script:FailRestore) { throw 'Fixture rollback detail must not replace or annotate the original error' }
        if ($isSetup) { $script:FailRestore=$true }
    }
    $failure=$null
    try { Set-SessionDns $fixture } catch { $failure=$_ }
    if (!$failure -or $failure.Exception.Message -ne $original.Exception.Message -or $failure.FullyQualifiedErrorId -notmatch '^FixtureDnsSetup') { throw 'DNS rollback replaced the original setup ErrorRecord' }
    if (![object]::ReferenceEquals($failure.Exception,$original.Exception)) { throw 'DNS rollback replaced the original exception instance' }
    $annotations=@($failure.Exception.Data['MukhomorRollbackFailures'])
    if ($annotations.Count -ne 1 -or $annotations[0] -cne 'dns') { throw 'DNS rollback failures require only the safe dns stage label' }
    $backup=Read-JsonFile (Join-Path $fixture 'runtime\dns-backup.json')
    if ($backup.adapters.Count -ne 2 -or $backup.adapters[0].servers.Count -ne 2) { throw 'DNS rollback failure lost the original recovery snapshot' }
    'PASS: DNS setup ErrorRecord survives rollback failure and retains recovery data'
    $script:FailRestore=$false
    Restore-SessionDns $fixture
    if (Test-Path -LiteralPath (Join-Path $fixture 'runtime\dns-backup.json')) { throw 'Retry after the failed rollback did not clear its snapshot' }
    'PASS: retained DNS rollback snapshot can be recovered on a later retry'
} $fixture
'All network changes in this test were mocked.'
} finally {
    $resolved=[IO.Path]::GetFullPath($fixture);$expected=([IO.Path]::GetFullPath($PSScriptRoot)+'\dns-')
    if($resolved.StartsWith($expected,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^dns-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
