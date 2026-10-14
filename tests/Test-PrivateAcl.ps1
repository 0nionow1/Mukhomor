Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) 'SplitVpn.psm1') -Force -DisableNameChecking
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$owner=[Security.Principal.WindowsIdentity]::GetCurrent().User
$acl=New-Object Security.AccessControl.DirectorySecurity
$acl.SetOwner($owner);$acl.SetAccessRuleProtection($true,$false)
foreach($value in @($owner.Value,'S-1-5-18','S-1-5-32-544'|Sort-Object -Unique)){
    $principal=New-Object Security.Principal.SecurityIdentifier($value)
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($principal,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
}
if (!(Test-SplitPrivateAcl $acl $owner)) {throw 'Exact owned private ACL rejected'}
$world=New-Object Security.Principal.SecurityIdentifier('S-1-1-0')
$worldRule=New-Object Security.AccessControl.FileSystemAccessRule($world,'Modify','ContainerInherit,ObjectInherit','None','Allow')
$acl.AddAccessRule($worldRule)
if (Test-SplitPrivateAcl $acl $owner) {throw 'Protected world-write ACL accepted'}
$acl.RemoveAccessRuleSpecific($worldRule)
$acl.SetAccessRuleProtection($false,$true)
if (Test-SplitPrivateAcl $acl $owner) {throw 'Unprotected inherited ACL accepted'}
$acl.SetAccessRuleProtection($true,$false);$acl.SetOwner($world)
if (Test-SplitPrivateAcl $acl $owner) {throw 'Foreign private state owner accepted'}
# Real upgrade fixtures live only in a fresh owned workspace directory. They
# exercise the old failure case: a correct private parent with unsafe children.
$fixtureBase=[IO.Path]::GetFullPath($PSScriptRoot)
$fixture=[IO.Path]::GetFullPath((Join-Path $fixtureBase ('private-acl-'+[guid]::NewGuid().ToString('N'))))
if (!$fixture.StartsWith($fixtureBase+'\',[StringComparison]::OrdinalIgnoreCase)) {throw 'Unsafe ACL test directory'}
$links=New-Object 'Collections.Generic.List[string]'
$module=Get-Module SplitVpn
function New-ExactDirectoryAcl {
    $result=New-Object Security.AccessControl.DirectorySecurity
    $result.SetOwner($owner);$result.SetAccessRuleProtection($true,$false)
    foreach ($value in @($owner.Value,'S-1-5-18','S-1-5-32-544'|Sort-Object -Unique)) {
        $principal=New-Object Security.Principal.SecurityIdentifier($value)
        $result.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($principal,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
    }
    return $result
}
function New-FixtureRoot([string]$Name) {
    $path=Join-Path $fixture $Name
    [IO.Directory]::CreateDirectory($path)|Out-Null
    Set-SplitPrivatePathAcl $path (New-ExactDirectoryAcl) $true
    return $path
}
function Assert-Rejected([scriptblock]$Action,[string]$Description) {
    $rejected=$false
    try {& $Action|Out-Null} catch {$rejected=$true}
    if (!$rejected) {throw $Description}
}
function Add-UnsafeFileAcl([string]$Path) {
    $fileAcl=New-SplitPrivateFileAcl $owner
    $fileAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($world,'Modify','None','None','Allow')))
    Set-SplitPrivatePathAcl $Path $fileAcl $false
}
function New-PrivateParent([string]$Root) {
    $path=Join-Path $Root 'private'
    [IO.Directory]::CreateDirectory($path)|Out-Null
    Set-SplitPrivatePathAcl $path (New-ExactDirectoryAcl) $true
    return $path
}
try {
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    Set-SplitPrivatePathAcl $fixture (New-ExactDirectoryAcl) $true
    $root=New-FixtureRoot 'upgrade'
    # The installed public root also grants READ to the owning UI user. An
    # unrelated READ-only principal is acceptable; only WRITE is disallowed.
    $rootAcl=New-ExactDirectoryAcl
    $rootAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($world,'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow')))
    Set-SplitPrivatePathAcl $root $rootAcl $true
    $private=New-PrivateParent $root
    $nested=Join-Path $private 'nested'
    [IO.Directory]::CreateDirectory($nested)|Out-Null
    $unsafe=New-ExactDirectoryAcl
    $unsafe.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($world,'Modify','ContainerInherit,ObjectInherit','None','Allow')))
    Set-SplitPrivatePathAcl $nested $unsafe $true
    $file=Join-Path $nested 'fixture.txt'
    [IO.File]::WriteAllText($file,'fixture-only')
    Add-UnsafeFileAcl $file
    $direct=Join-Path $private 'direct.txt'
    [IO.File]::WriteAllText($direct,'direct-fixture')
    Add-UnsafeFileAcl $direct
    if (!(Test-SplitPrivateAcl (Get-Acl -LiteralPath $private) $owner)) {throw 'Test private parent is not exact'}
    Initialize-SplitRoot $root
    if (!(Test-SplitPrivateAcl (Get-Acl -LiteralPath $nested) $owner) -or !(Test-SplitPrivateFileAcl (Get-Acl -LiteralPath $file) $owner) -or !(Test-SplitPrivateFileAcl (Get-Acl -LiteralPath $direct) $owner)) {throw 'Unsafe child ACL survived migration'}
    if ([IO.File]::ReadAllText($file) -cne 'fixture-only' -or [IO.File]::ReadAllText($direct) -cne 'direct-fixture') {throw 'Migration changed fixture bytes'}
    if (!(Test-SplitPrivateMigration $private $owner)) {throw 'Successful migration stamp absent'}
    # The ordinary snapshot path must not enumerate the tree again.
    $original=& $module {(Get-Command Repair-SplitPrivateTree).ScriptBlock}
    try {
        & $module {Set-Item Function:Repair-SplitPrivateTree {throw 'Repeated private-tree traversal'}}
        Initialize-SplitRoot $root
    } finally {& $module {param($body) Set-Item Function:Repair-SplitPrivateTree $body} $original}
    # A correctly worded but untrusted marker cannot bypass the repair.
    $marker=Join-Path $private '.acl-migration-v2'
    Add-UnsafeFileAcl $marker
    Add-UnsafeFileAcl $direct
    Initialize-SplitRoot $root
    if (!(Test-SplitPrivateFileAcl (Get-Acl -LiteralPath $direct) $owner) -or !(Test-SplitPrivateMigration $private $owner)) {throw 'Untrusted marker bypassed migration'}
    [IO.File]::WriteAllText($marker,('x'*65))
    Add-UnsafeFileAcl $direct
    Initialize-SplitRoot $root
    if (!(Test-SplitPrivateFileAcl (Get-Acl -LiteralPath $direct) $owner) -or !(Test-SplitPrivateMigration $private $owner)) {throw 'Oversized marker bypassed migration'}
    # Even a trusted old marker is insufficient once its parent has drifted.
    $parentDrift=New-ExactDirectoryAcl
    $parentDrift.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($world,'Modify','ContainerInherit,ObjectInherit','None','Allow')))
    Set-SplitPrivatePathAcl $private $parentDrift $true
    Add-UnsafeFileAcl $direct
    Initialize-SplitRoot $root
    if (!(Test-SplitPrivateFileAcl (Get-Acl -LiteralPath $direct) $owner)) {throw 'Parent repair reused stale migration stamp'}

    $linkRoot=New-FixtureRoot 'link-rejection'
    $linkPrivate=New-PrivateParent $linkRoot
    $outside=New-FixtureRoot 'outside-private'
    $outsideFile=Join-Path $outside 'sentinel.txt'
    [IO.File]::WriteAllText($outsideFile,'outside-fixture')
    Add-UnsafeFileAcl $outsideFile
    $outsideAcl=(Get-Acl -LiteralPath $outside).Sddl
    $outsideFileAcl=(Get-Acl -LiteralPath $outsideFile).Sddl
    $junction=Join-Path $linkPrivate 'linked'
    New-Item -ItemType Junction -Path $junction -Target $outside|Out-Null
    $links.Add($junction)
    Assert-Rejected {Initialize-SplitRoot $linkRoot} 'Private junction accepted'
    if ((Get-Acl -LiteralPath $outside).Sddl -cne $outsideAcl -or (Get-Acl -LiteralPath $outsideFile).Sddl -cne $outsideFileAcl -or [IO.File]::ReadAllText($outsideFile) -cne 'outside-fixture') {throw 'Migration touched a junction target'}
    if (Test-SplitPrivateMigration $linkPrivate $owner) {throw 'Rejected tree received migration stamp'}
    # A previous stamp must be invalidated if a forced re-migration fails.
    [IO.Directory]::Delete($junction,$false)
    $links.Remove($junction)|Out-Null
    Initialize-SplitRoot $linkRoot
    New-Item -ItemType Junction -Path $junction -Target $outside|Out-Null
    $links.Add($junction)
    Set-SplitPrivatePathAcl $linkPrivate $parentDrift $true
    Assert-Rejected {Initialize-SplitRoot $linkRoot} 'Failed re-migration unexpectedly succeeded'
    if (Test-SplitPrivateMigration $linkPrivate $owner) {throw 'Failed repair retained previous success stamp'}

    $hardRoot=New-FixtureRoot 'hardlink-rejection'
    $hardPrivate=New-PrivateParent $hardRoot
    $unchanged=Join-Path $hardPrivate 'a-not-yet-repaired.txt'
    [IO.File]::WriteAllText($unchanged,'unchanged-fixture')
    Add-UnsafeFileAcl $unchanged
    $unchangedAcl=(Get-Acl -LiteralPath $unchanged).Sddl
    $hardlink=Join-Path $hardPrivate 'outside-hardlink.txt'
    New-Item -ItemType HardLink -Path $hardlink -Target $outsideFile|Out-Null
    Assert-Rejected {Initialize-SplitRoot $hardRoot} 'Private hardlink accepted'
    if ((Get-Acl -LiteralPath $unchanged).Sddl -cne $unchangedAcl) {throw 'File repair began before whole-tree validation'}
    if ((Get-Acl -LiteralPath $outsideFile).Sddl -cne $outsideFileAcl -or [IO.File]::ReadAllText($outsideFile) -cne 'outside-fixture') {throw 'Hardlink target was modified'}
    if (Test-SplitPrivateMigration $hardPrivate $owner) {throw 'Rejected hardlink tree received migration stamp'}
    # A valid literal and exact ACL do not authenticate a multiply-linked stamp.
    $stampRoot=New-FixtureRoot 'hardlink-marker'
    $stampPrivate=New-PrivateParent $stampRoot
    $outsideStamp=Join-Path $outside 'marker-fixture.txt'
    [IO.File]::WriteAllText($outsideStamp,"Mukhomor private ACL migration v2`n")
    Set-SplitPrivatePathAcl $outsideStamp (New-SplitPrivateFileAcl $owner) $false
    $stampAcl=(Get-Acl -LiteralPath $outsideStamp).Sddl
    New-Item -ItemType HardLink -Path (Join-Path $stampPrivate '.acl-migration-v2') -Target $outsideStamp|Out-Null
    Assert-Rejected {Initialize-SplitRoot $stampRoot} 'Multiply-linked migration marker accepted'
    if ((Get-Acl -LiteralPath $outsideStamp).Sddl -cne $stampAcl -or [IO.File]::ReadAllText($outsideStamp) -cne "Mukhomor private ACL migration v2`n") {throw 'Hardlink marker target was modified'}

    $heldRoot=New-FixtureRoot 'existing-writer'
    $heldPrivate=New-PrivateParent $heldRoot
    $heldFile=Join-Path $heldPrivate 'fixture.txt'
    [IO.File]::WriteAllText($heldFile,'writer-fixture')
    Add-UnsafeFileAcl $heldFile
    $heldAcl=(Get-Acl -LiteralPath $heldFile).Sddl
    $writer=[IO.File]::Open($heldFile,'Open','ReadWrite','ReadWrite')
    try {
        Assert-Rejected {Initialize-SplitRoot $heldRoot} 'Migration adopted an existing writable file handle'
        if ((Get-Acl -LiteralPath $heldFile).Sddl -cne $heldAcl) {throw 'Blocked writable file was repaired'}
        if (Test-SplitPrivateMigration $heldPrivate $owner) {throw 'Blocked writer received migration stamp'}
    } finally {$writer.Dispose()}
    $directoryWriter=[MukhomorPrivateFileIdentity]::CreateFileW($heldPrivate,2,7,[IntPtr]::Zero,3,0x2200000,[IntPtr]::Zero)
    if ($directoryWriter.IsInvalid) {$directoryWriter.Dispose();throw 'Writable directory fixture could not be opened'}
    try {
        Assert-Rejected {Initialize-SplitRoot $heldRoot} 'Migration adopted an existing writable directory handle'
    } finally {$directoryWriter.Dispose()}
    Initialize-SplitRoot $heldRoot
    if (!(Test-SplitPrivateMigration $heldPrivate $owner)) {throw 'Migration failed to recover after writer release'}
    $directoryPin=Open-SplitPrivateDirectory $heldPrivate
    try {
        Assert-Rejected {[IO.Directory]::Move($heldPrivate,$heldPrivate+'-moved')} 'Pinned directory was replaceable'
    } finally {$directoryPin.Dispose()}
    $filePin=Open-SplitPrivateFile $heldFile
    try {
        Assert-Rejected {[IO.File]::Move($heldFile,$heldFile+'-moved')} 'Pinned file was replaceable'
        Assert-Rejected {[IO.File]::WriteAllText($heldFile,'changed')} 'Pinned file allowed a conflicting writer'
    } finally {$filePin.Dispose()}
    if ([IO.File]::ReadAllText($heldFile) -cne 'writer-fixture') {throw 'Pinned file bytes changed'}

    $badRoot=New-FixtureRoot 'untrusted-parent'
    Set-SplitPrivatePathAcl $badRoot $parentDrift $true
    Assert-Rejected {Initialize-SplitRoot $badRoot} 'World-writable owned root accepted'
    if (Test-Path -LiteralPath (Join-Path $badRoot 'private')) {throw 'Untrusted parent was modified before rejection'}
    $helperFixture=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('acl-helper-'+[guid]::NewGuid().ToString('N'))))
    $helperLink=Join-Path $helperFixture 'linked'
    try {
        [IO.Directory]::CreateDirectory($helperFixture)|Out-Null
        Set-OwnedTestRoot $helperFixture $helperFixture
        if (!(Test-SplitPrivateAcl (Get-Acl -LiteralPath $helperFixture) $owner)) {throw 'Test fixture helper failed to secure root'}
        Assert-Rejected {Set-OwnedTestRoot $outside $helperFixture} 'Test fixture helper accepted an outside target'
        New-Item -ItemType Junction -Path $helperLink -Target $outside|Out-Null
        Assert-Rejected {Set-OwnedTestRoot $helperLink $helperFixture} 'Test fixture helper accepted a junction'
        if ((Get-Acl -LiteralPath $outside).Sddl -cne $outsideAcl) {throw 'Test fixture helper modified outside ACL'}
    } finally {
        if (Test-Path -LiteralPath $helperLink) {
            if (!((Get-Item -LiteralPath $helperLink -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {throw 'Test helper junction identity changed'}
            [IO.Directory]::Delete($helperLink,$false)
        }
        if ([IO.Path]::GetDirectoryName($helperFixture) -cne [IO.Path]::GetFullPath($PSScriptRoot) -or [IO.Path]::GetFileName($helperFixture) -cnotmatch '^acl-helper-[a-f0-9]{32}$') {throw 'Unsafe test helper cleanup'}
        if (Test-Path -LiteralPath $helperFixture) {Remove-Item -LiteralPath $helperFixture -Recurse -Force}
    }
    'PASS: exact ACL predicates; real child repair, authenticated one-time marker, parent drift, junction/hardlink isolation, conflicting writers, replacement pins and untrusted-root rejection. Only fresh D: workspace fixtures changed.'
} finally {
    foreach ($link in $links) {
        if (!$link.StartsWith($fixture+'\',[StringComparison]::OrdinalIgnoreCase)) {throw 'Unsafe junction cleanup'}
        if (Test-Path -LiteralPath $link) {
            if (!((Get-Item -LiteralPath $link -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {throw 'Fixture junction identity changed'}
            [IO.Directory]::Delete($link,$false)
        }
    }
    if ((Test-Path -LiteralPath $fixture) -and $fixture.StartsWith($fixtureBase+'\',[StringComparison]::OrdinalIgnoreCase)) {
        if ((Get-Item -LiteralPath $fixture -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'Fixture root became a reparse point'}
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
}
