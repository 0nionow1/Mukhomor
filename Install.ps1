param([string]$OwnerSid,[switch]$VerifyOnly,[switch]$StartOnly)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$manifest=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'release-manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if($manifest.version -notmatch '^\d+\.\d+\.\d+$' -or @($manifest.files).Count -eq 0){throw 'Invalid release manifest'}
$seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
foreach ($file in $manifest.files) {
    $path=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot $file.path))
    if (!$path.StartsWith(([IO.Path]::GetFullPath($PSScriptRoot)+'\'),[StringComparison]::OrdinalIgnoreCase) -or !$seen.Add($path) -or $file.sha256 -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) {throw ('Package verification failed: '+$file.path)}
}
foreach($required in @('mukhomor.exe','NativeBridge.ps1','SplitVpn.psm1','SplitWindows.psm1','ProfileImport.psm1','bin\mihomo-windows-amd64-compatible.exe','bin\wintun.dll','assets\settings.default.json')){if(!$seen.Contains((Join-Path $PSScriptRoot $required))){throw ('Missing application file: '+$required)}}
if($VerifyOnly){Write-Host ('Verified release '+$manifest.version+'; '+$seen.Count+' files');return}
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
if (!(New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {throw 'Windows administrator approval is required to configure the VPN'}
if (!$OwnerSid) {$OwnerSid=$identity.User.Value}
if ($OwnerSid -notmatch '^S-1-(\d+-)+\d+$') {throw 'Invalid owner SID'}
$program=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Mukhomor'))
$data=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Mukhomor'))
if ((Test-Path -LiteralPath (Join-Path $data 'owner.sid')) -and [IO.File]::ReadAllText((Join-Path $data 'owner.sid')).Trim() -ne $OwnerSid) {throw 'The existing installation belongs to another Windows user'}
if($StartOnly) {
    $record=Get-Content -LiteralPath (Join-Path $data 'installation.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $expected='"'+[IO.Path]::GetFullPath($record.executable)+'" --service --root "'+$data+'"'
    if([IO.Path]::GetFullPath($record.executable) -ne (Join-Path $PSScriptRoot 'mukhomor.exe')){throw 'Repair must run from the protected installed application'}
    $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    if($definition.PathName -ne $expected){throw 'The controller path does not match the installed application'}
    $service=Get-Service -Name 'Mukhomor'
    if($definition.StartMode -eq 'Disabled') {
        & sc.exe config 'Mukhomor' 'start=' 'delayed-auto' | Out-Null
        if($LASTEXITCODE -ne 0){throw 'Unable to restore controller startup'}
    }
    if($service.Status -eq 'Stopped'){Start-Service -Name 'Mukhomor'}
    $service.WaitForStatus('Running',[timespan]::FromSeconds(45))
    return
}
function Set-ManagedAcl([string]$Path,[bool]$Data) {
    $acl=New-Object Security.AccessControl.DirectorySecurity
    $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
    $acl.SetAccessRuleProtection($true,$false)
    foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)),'FullControl','ContainerInherit,ObjectInherit','None','Allow')))}
    $readSid=if($Data){$OwnerSid}else{'S-1-5-32-545'}
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($readSid)),'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow')))
    Set-Acl -LiteralPath $Path -AclObject $acl
}
function Write-ManagedBytes([string]$Path,[byte[]]$Bytes) {
    $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    $backup=$Path+'.'+[guid]::NewGuid().ToString('N')+'.backup'
    try {
        [IO.File]::WriteAllBytes($temporary,$Bytes)
        if([IO.File]::Exists($Path)){[IO.File]::Replace($temporary,$Path,$backup)}
        else{[IO.File]::Move($temporary,$Path)}
    } finally {
        foreach($ownedFile in @($temporary,$backup)){if([IO.File]::Exists($ownedFile)){[IO.File]::Delete($ownedFile)}}
    }
}
function Set-AppEntries([string]$Executable,[string]$ReleaseVersion) {
    $appDirectory=[IO.Path]::GetDirectoryName($Executable)
    $shell=New-Object -ComObject WScript.Shell
    foreach($shortcut in @((Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Mukhomor.lnk'),(Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Mukhomor.lnk'))) {
        $link=$shell.CreateShortcut($shortcut)
        $link.TargetPath=$Executable; $link.WorkingDirectory=$appDirectory; $link.Description='Mukhomor'; $link.IconLocation=$Executable+',0'; $link.Save()
    }
    $uninstallKey='HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Mukhomor'
    New-Item -Path $uninstallKey -Force | Out-Null
    foreach($entry in @{DisplayName='Mukhomor';DisplayVersion=$ReleaseVersion;Publisher='Mukhomor';InstallLocation=$appDirectory;DisplayIcon=$Executable+',0';UninstallString='"'+$Executable+'" --uninstall'}.GetEnumerator()) {
        New-ItemProperty -LiteralPath $uninstallKey -Name $entry.Key -Value $entry.Value -PropertyType String -Force | Out-Null
    }
    foreach($key in @('NoModify','NoRepair')) {New-ItemProperty -LiteralPath $uninstallKey -Name $key -Value 1 -PropertyType DWord -Force | Out-Null}
}
function Give-OwnerStartAccess([string]$Sid) {
    # Allow only this installation's owner to start/query the immutable own
    # controller. Preserve the existing SYSTEM/admin and service ACL entries.
    $descriptorLines=@(& sc.exe sdshow 'Mukhomor')
    if($LASTEXITCODE -ne 0){throw 'Unable to read controller permissions'}
    $sddl=@($descriptorLines | Where-Object {$_ -match '^[OGDS]:'}) -join ''
    $descriptor=New-Object Security.AccessControl.RawSecurityDescriptor($sddl)
    $identity=New-Object Security.Principal.SecurityIdentifier($Sid)
    $acl=$descriptor.DiscretionaryAcl
    if(!$acl){throw 'The controller has no protected access list'}
    $granted=$false
    foreach($entry in $acl) {
        if($entry -is [Security.AccessControl.CommonAce] -and $entry.AceQualifier -eq [Security.AccessControl.AceQualifier]::AccessAllowed -and $entry.SecurityIdentifier -eq $identity -and ($entry.AccessMask -band 21) -eq 21){$granted=$true}
    }
    if(!$granted) {
        $position=0
        while($position -lt $acl.Count -and $acl[$position].AceType -eq [Security.AccessControl.AceType]::AccessDenied){$position++}
        $entry=New-Object Security.AccessControl.CommonAce([Security.AccessControl.AceFlags]::None,[Security.AccessControl.AceQualifier]::AccessAllowed,21,$identity,$false,$null)
        $acl.InsertAce($position,$entry)
        & sc.exe sdset 'Mukhomor' ($descriptor.GetSddlForm([Security.AccessControl.AccessControlSections]::All)) | Out-Null
        if($LASTEXITCODE -ne 0){throw 'Unable to allow the owner to reopen Mukhomor'}
    }
}
if ([IO.Path]::GetDirectoryName($program) -ne [IO.Path]::GetFullPath([Environment]::GetFolderPath('ProgramFiles')) -or [IO.Path]::GetDirectoryName($data) -ne [IO.Path]::GetFullPath([Environment]::GetFolderPath('CommonApplicationData'))) {throw 'Invalid installation target'}
foreach ($path in @($program,$data)) {
    if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint)) {throw 'Installation target is a reparse point'}
    if($path -eq $data -and (Test-Path -LiteralPath $data) -and @(Get-ChildItem -LiteralPath $data -Force).Count -gt 0 -and (!(Test-Path -LiteralPath (Join-Path $data 'owner.sid')) -or !(Test-Path -LiteralPath (Join-Path $data 'installation.json')))){throw 'An unmanaged non-empty data directory already exists'}
    [IO.Directory]::CreateDirectory($path) | Out-Null
}
foreach($path in @($program,$data)) {
    foreach($child in @(Get-ChildItem -LiteralPath $path -Recurse -Force)) {
        if($child.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'A managed directory contains a reparse point'}
    }
}
if(Test-Path -LiteralPath (Join-Path $data 'installation.json')) {
    $existing=Get-Content -LiteralPath (Join-Path $data 'installation.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $existingPath=[IO.Path]::GetFullPath($existing.executable)
    if(!$existingPath.StartsWith(($program+'\app-'),[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($existingPath) -ne 'mukhomor.exe'){throw 'Existing installation record is not owned by Mukhomor'}
    $owner=(Get-Acl -LiteralPath $data).GetOwner([Security.Principal.SecurityIdentifier]).Value
    if($owner -notin @('S-1-5-18','S-1-5-32-544')){throw 'Existing data directory is not protected by Windows'}
}
Set-ManagedAcl $program $false; Set-ManagedAcl $data $true
$version=Join-Path $program ('app-'+$manifest.version+'-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($version) | Out-Null
foreach ($file in $manifest.files) {
    $target=Join-Path $version $file.path
    [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
    [IO.File]::Copy((Join-Path $PSScriptRoot $file.path),$target,$false)
    if((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256){throw 'The package changed while being installed'}
}
[IO.File]::Copy((Join-Path $PSScriptRoot 'release-manifest.json'),(Join-Path $version 'release-manifest.json'),$false)
$service=Get-Service -Name 'Mukhomor' -ErrorAction SilentlyContinue
$oldExecutable=$null
$oldBinary=$null
$upgradeCommitted=$false
$serviceDisabled=$false
$createdService=$false
$previousRecord=$null
$oldRunning=$false
try {
if ($service) {
    $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    $record=Get-Content -LiteralPath (Join-Path $data 'installation.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $oldExecutable=[IO.Path]::GetFullPath($record.executable)
    if ($definition.PathName -ne ('"'+$oldExecutable+'" --service --root "'+$data+'"') -or [IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($oldExecutable)) -ne $program) {throw 'The Mukhomor service name is owned by another application'}
    $oldBinary=$definition.PathName
    $previousRecord=[IO.File]::ReadAllBytes((Join-Path $data 'installation.json'))
    $oldRunning=$service.Status -ne 'Stopped'
    # Prevent SCM crash recovery from restarting an old controller mid-upgrade.
    $disable=Invoke-CimMethod -InputObject $definition -MethodName Change -Arguments @{StartMode='Disabled'}
    if($disable.ReturnValue -ne 0){throw 'Unable to suspend controller recovery during upgrade'}
    $serviceDisabled=$true
    if ($service.Status -ne 'Stopped') {
        # An older controller may be blocked inside a synchronous operation.
        # Request a normal stop first, then close only the verified owned image.
        $controllerId=[int]$definition.ProcessId
        & sc.exe stop 'Mukhomor' | Out-Null
        if($LASTEXITCODE -notin @(0,1061,1062)){throw 'Unable to request controller shutdown'}
        try {$service.WaitForStatus('Stopped',[timespan]::FromSeconds(15))}
        catch {
            $stalled=Get-Process -Id $controllerId -ErrorAction SilentlyContinue
            $currentDefinition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
            if(!$stalled -or !$stalled.Path -or [IO.Path]::GetFullPath($stalled.Path) -ne $oldExecutable -or [int]$currentDefinition.ProcessId -ne $controllerId -or $currentDefinition.PathName -ne $oldBinary){throw 'The stalled controller could not be safely identified'}
            # Its job object terminates its own helpers and Mihomo. The new
            # controller restores any retained DNS backup before accepting work.
            Stop-Process -Id $controllerId -Force
            $service.WaitForStatus('Stopped',[timespan]::FromSeconds(10))
        }
    }
}
$executable=Join-Path $version 'mukhomor.exe'
$binary='"'+$executable+'" --service --root "'+$data+'"'
if (!$service) {New-Service -Name 'Mukhomor' -DisplayName 'Mukhomor' -BinaryPathName $binary -StartupType Disabled -Description 'Native controller for user-imported AmneziaWG profiles and Mihomo' | Out-Null; $createdService=$true}
else {
    $result=Invoke-CimMethod -InputObject $definition -MethodName Change -Arguments @{PathName=$binary;StartMode='Disabled'}
    if($result.ReturnValue -ne 0){throw ('Unable to update the service: '+$result.ReturnValue)}
}
& sc.exe failure 'Mukhomor' 'reset=' '86400' 'actions=' 'restart/5000/restart/15000/restart/60000' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Unable to configure service recovery'}
& sc.exe failureflag 'Mukhomor' '1' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Unable to enable service error recovery'}
Write-ManagedBytes (Join-Path $data 'owner.sid') ([Text.Encoding]::UTF8.GetBytes($OwnerSid))
Write-ManagedBytes (Join-Path $data 'installation.json') ([Text.Encoding]::UTF8.GetBytes((@{version=$manifest.version;executable=$executable}|ConvertTo-Json)))
Give-OwnerStartAccess $OwnerSid
Set-AppEntries $executable $manifest.version
# All records now point at the complete verified package. Only now can SCM
# recovery or automatic boot startup safely launch the updated controller.
$upgradeCommitted=$true
& sc.exe config 'Mukhomor' 'start=' 'delayed-auto' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Unable to configure automatic service startup'}
try {Start-Service -Name 'Mukhomor'} catch {
    $activeService=Get-Service -Name 'Mukhomor'
    $activeDefinition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    if($activeService.Status -ne 'Running' -or $activeDefinition.PathName -ne $binary){throw}
}
} catch {
    if(!$upgradeCommitted -and $serviceDisabled) {
        $currentService=Get-Service -Name 'Mukhomor'
        if($currentService.Status -ne 'Stopped') {
            # A failed ownership check or shutdown must not replace a live image.
            & sc.exe config 'Mukhomor' 'start=' 'delayed-auto' | Out-Null
        } else {
            $rollback=Invoke-CimMethod -InputObject $definition -MethodName Change -Arguments @{PathName=$oldBinary;StartMode='Disabled'}
            if($rollback.ReturnValue -ne 0){throw 'Upgrade failed; the controller remains disabled because rollback could not be verified'}
            Write-ManagedBytes (Join-Path $data 'installation.json') $previousRecord
            & sc.exe config 'Mukhomor' 'start=' 'delayed-auto' | Out-Null
            if($LASTEXITCODE -ne 0){throw 'Upgrade failed; unable to restore controller startup'}
            if($oldRunning){Start-Service -Name 'Mukhomor'}
            Set-AppEntries $oldExecutable $record.version
        }
    } elseif(!$upgradeCommitted -and $createdService) {
        & sc.exe delete 'Mukhomor' | Out-Null
    }
    throw
}
# Retire the old interface gracefully so its singleton cannot reactivate an
# obsolete version after the new executable opens. No service/core is targeted.
if($oldExecutable) {
    Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class MukhomorUpgrade {
 public delegate bool WindowCallback(IntPtr window, IntPtr state);
 [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback, IntPtr state);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder name, int length);
 [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
 public static bool CloseInterface(uint process, bool retire=false) {
  bool found=false;
  EnumWindows(delegate(IntPtr window, IntPtr state) {
   uint owner; GetWindowThreadProcessId(window, out owner);
   if(owner==process) {
    var name=new StringBuilder(256); GetClassName(window,name,name.Capacity);
    if(name.ToString().StartsWith("Mukhomor.Native.",StringComparison.Ordinal)) {
     found=true; PostMessage(window,retire ? 0x8004u : 0x0010u,IntPtr.Zero,IntPtr.Zero);
    }
   }
   return true;
  },IntPtr.Zero);
  return found;
 }
}
'@
    foreach($oldUi in @(Get-Process -Name 'mukhomor' -ErrorAction SilentlyContinue)) {
        if($oldUi.Path -and [IO.Path]::GetFullPath($oldUi.Path) -eq $oldExecutable -and [MukhomorUpgrade]::CloseInterface([uint32]$oldUi.Id,([version]$record.version -ge [version]'0.4.0'))) {
            if(!$oldUi.WaitForExit(5000)){throw 'Close the previous Mukhomor window and open the updated application again'}
        }
    }
}
Write-Host 'Mukhomor is ready. Import your server configuration or link and connect.'
Write-Host 'No VPN connection is started until a user profile has been imported and selected.'
