# Test preparation only. Production must reject an unsafe state root rather
# than adopting it. Call before Initialize-SplitRoot or starting a QA worker.
Set-StrictMode -Version Latest
$script:OwnedTestRootBase=[IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')
function Set-OwnedTestRoot([string]$Path,[string]$Fixture) {
    $fixturePath=[IO.Path]::GetFullPath($Fixture).TrimEnd('\')
    $target=[IO.Path]::GetFullPath($Path).TrimEnd('\')
    if ([IO.Path]::GetDirectoryName($fixturePath) -cne $script:OwnedTestRootBase -or [IO.Path]::GetFileName($fixturePath) -cnotmatch '^[A-Za-z][A-Za-z0-9-]*-[a-f0-9]{32}$') {throw 'Test root must be a fresh named fixture directly below tests'}
    if ($target -ine $fixturePath -and !$target.StartsWith($fixturePath+'\',[StringComparison]::OrdinalIgnoreCase)) {throw 'Test root escaped its fixture'}
    $owner=[Security.Principal.WindowsIdentity]::GetCurrent().User
    $expected=New-Object Security.AccessControl.DirectorySecurity
    $expected.SetOwner($owner);$expected.SetAccessRuleProtection($true,$false)
    foreach ($value in @($owner.Value,'S-1-5-18','S-1-5-32-544'|Sort-Object -Unique)) {
        $sid=New-Object Security.Principal.SecurityIdentifier($value)
        $expected.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
    }
    $paths=New-Object 'Collections.Generic.List[string]'
    $paths.Add($fixturePath)
    if ($target -ine $fixturePath) {
        $next=$fixturePath
        foreach ($part in $target.Substring($fixturePath.Length+1).Split('\')) {$next=Join-Path $next $part;$paths.Add($next)}
    }
    foreach ($currentPath in $paths) {
        # Check before querying ACL or descending into the next component.
        $item=Get-Item -LiteralPath $currentPath -Force
        if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {throw 'Test root must contain ordinary directories'}
        $sections=[Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner
        $current=$item.GetAccessControl($sections)
        $currentOwner=$current.GetOwner([Security.Principal.SecurityIdentifier]).Value
        if ($currentOwner -ne $owner.Value) {
            # An elevated Windows runner creates fresh directories with the
            # Administrators group as owner. Normalize only that exact case,
            # inside the already validated fresh fixture, never another user.
            $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
            $principal=New-Object Security.Principal.WindowsPrincipal($identity)
            if($currentOwner -ne 'S-1-5-32-544' -or !$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Test fixture has a foreign owner'}
            $current.SetOwner($owner)
        }
        $current.SetSecurityDescriptorSddlForm($expected.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access),[Security.AccessControl.AccessControlSections]::Access)
        $item.SetAccessControl($current)
        $actual=$item.GetAccessControl($sections)
        $allowed=@($owner.Value,'S-1-5-18','S-1-5-32-544'|Sort-Object -Unique)
        $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
        $rules=@($actual.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
        if (!$actual.AreAccessRulesProtected -or $actual.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $owner.Value -or $rules.Count -ne $allowed.Count) {throw 'Test fixture permissions could not be confirmed'}
        foreach ($rule in $rules) {
            if ($rule.IdentityReference.Value -notin $allowed -or !$seen.Add($rule.IdentityReference.Value) -or $rule.AccessControlType -ne 'Allow' -or $rule.FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or $rule.InheritanceFlags -ne ([Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit) -or $rule.PropagationFlags -ne 'None' -or $rule.IsInherited) {throw 'Test fixture permissions could not be confirmed'}
        }
    }
}
