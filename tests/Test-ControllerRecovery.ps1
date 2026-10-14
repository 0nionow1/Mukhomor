Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
# Extract only the reviewed permission helper. No Install/SCM/filesystem action
# runs; sc.exe is an inert function that accepts and records an in-memory SDDL.
$source=Join-Path (Split-Path -Parent $PSScriptRoot) 'Install.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Install parser error'}
$function=$ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Give-OwnerStartAccess'},$true)
if(@($function).Count -ne 1){throw 'Missing exact controller permission helper'}
. ([scriptblock]::Create($function.Extent.Text))
$script:Shown='';$script:Written='';$script:Writes=0
function sc.exe {
    param($Action,$Name,$Descriptor)
    if($Name -cne 'Mukhomor'){throw 'Unexpected mock service'}
    switch($Action){
        'sdshow' {$global:LASTEXITCODE=0;return $script:Shown}
        'sdset' {$global:LASTEXITCODE=0;$script:Written=$Descriptor;$script:Writes++;return}
        default {throw 'Unreviewed mock service action'}
    }
}
$sid='S-1-5-21-123-456-789-1001'
$identity=New-Object Security.Principal.SecurityIdentifier($sid)
foreach($mask in @(0,21,131093)){
    $script:Writes=0;$script:Written=''
    $script:Shown='O:SYG:SYD:(A;;GA;;;SY)(A;;GA;;;BA)'+$(if($mask){'(A;;0x'+$mask.ToString('x')+';;;'+$sid+')'}else{''})
    Give-OwnerStartAccess $sid
    $after=if($script:Writes){$script:Written}else{$script:Shown}
    $descriptor=New-Object Security.AccessControl.RawSecurityDescriptor($after)
    $owned=@($descriptor.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier -eq $identity})
    if($owned.Count -ne 1 -or $owned[0].AccessMask -ne 131093){throw 'Owner recovery grant contains wrong or duplicated rights'}
    if(@($descriptor.DiscretionaryAcl).Count -ne 3){throw 'Trusted existing ACL entries changed'}
    foreach($trustedSid in @('S-1-5-18','S-1-5-32-544')){
        $trusted=@($descriptor.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier.Value -eq $trustedSid})
        if($trusted.Count -ne 1 -or $trusted[0].AccessMask -ne 268435456){throw 'SYSTEM/admin rights changed'}
    }
    if($script:Writes -ne $(if($mask -eq 131093){0}else{1})){throw 'Recovery permission upgrade was not idempotent'}
    $script:Shown=$after;$script:Writes=0
    Give-OwnerStartAccess $sid
    if($script:Writes){throw 'Repeated recovery grant wrote unchanged permissions'}
}
'PASS: extracted controller helper upgrades only owner READ_CONTROL/query/start; mock-only, no SCM actions'

# Extract the cancellation ACL constructor too. A temporary empty D: tree is
# created; Set-Acl is inert, so no ownership/service/host state is modified.
$constructor=$ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Set-RecoveryRequestAcl'},$true)
if(@($constructor).Count -ne 1){throw 'Missing exact cancellation ACL constructor'}
. ([scriptblock]::Create($constructor.Extent.Text))
$fixture=Join-Path $PSScriptRoot ('recovery-acl-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$script:CapturedAcl=$null
function Set-Acl {param($LiteralPath,$AclObject)
    if($LiteralPath -cne (Join-Path $fixture 'recovery-request')){throw 'Unscoped mock ACL target'}
    $script:CapturedAcl=$AclObject
}
try {
    Set-RecoveryRequestAcl $fixture $sid
    if(!$script:CapturedAcl.AreAccessRulesProtected -or $script:CapturedAcl.GetOwner([Security.Principal.SecurityIdentifier]).Value -cne 'S-1-5-32-544'){throw 'Cancellation directory owner/protection drift'}
    $raw=New-Object Security.AccessControl.RawSecurityDescriptor($script:CapturedAcl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::All))
    if($raw.DiscretionaryAcl.Count -ne 4){throw 'Cancellation constructor granted extra identities/rights'}
    foreach($trustedSid in @('S-1-5-18','S-1-5-32-544')){
        $entry=@($raw.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier.Value -eq $trustedSid})
        if($entry.Count -ne 1 -or $entry[0].AccessMask -ne 2032127 -or [int]$entry[0].AceFlags -ne 3){throw 'Cancellation SYSTEM/admin ACL drift'}
    }
    $entries=@($raw.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier.Value -eq $sid})
    if($entries.Count -ne 2 -or @($entries|Where-Object {$_.AccessMask -eq 1179851 -and [int]$_.AceFlags -eq 0}).Count -ne 1 -or @($entries|Where-Object {$_.AccessMask -eq 1245599 -and [int]$_.AceFlags -eq 9}).Count -ne 1){throw 'Cancellation owner access exceeds ordinary-file add/read/delete scope'}
    'PASS: extracted cancellation ACL grants fixed owner only file add/delete/read; runtime/private unchanged'
}finally {
    $child=Join-Path $fixture 'recovery-request'
    if([IO.Directory]::Exists($child)){[IO.Directory]::Delete($child,$false)}
    [IO.Directory]::Delete($fixture,$false)
}
