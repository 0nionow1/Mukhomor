Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
function Get-RemovalField($Value,[string]$Name) {
    if($Value -is [Collections.IDictionary]) {
        if($Value.Contains($Name)){return $Value[$Name]}
    } elseif($null -ne $Value) {
        $property=$Value.PSObject.Properties[$Name]
        if($null -ne $property){return $property.Value}
    }
    throw 'Removal proof is incomplete. Repair or upgrade Mukhomor and disconnect before retrying; files were preserved.'
}
function Assert-RemovalBoolean($Value,[string]$Name,[bool]$Expected) {
    $actual=Get-RemovalField $Value $Name
    if($actual -isnot [bool] -or $actual -ne $Expected){throw 'Removal proof is not verified; files were preserved.'}
}
function Assert-RemovalString($Value,[string]$Name,[string]$Expected) {
    $actual=Get-RemovalField $Value $Name
    if($actual -isnot [string] -or $actual -cne $Expected){throw 'Removal receipt identity is invalid; files were preserved.'}
}
function Assert-RemovalReceipt($Reply,[string]$RequestId) {
    # Disk state and an absent controller cannot prove persistent WFP is clear.
    # Require the authenticated controller's completed, uniquely bound Exit.
    Assert-RemovalBoolean $Reply 'ok' $true
    Assert-RemovalString $Reply 'request_id' $RequestId
    $snapshot=Get-RemovalField $Reply 'data'
    foreach($field in @('running','starting','busy')){Assert-RemovalBoolean $snapshot $field $false}
    Assert-RemovalBoolean $snapshot 'shutting_down' $true
    $operation=Get-RemovalField $snapshot 'last_operation'
    Assert-RemovalBoolean $operation 'ok' $true
    Assert-RemovalString $operation 'action' 'Exit'
    Assert-RemovalString $operation 'request_id' $RequestId
    $operationId=Get-RemovalField $Reply 'operation_id'
    if($operationId -isnot [int] -and $operationId -isnot [long]){throw 'Removal operation identity is invalid; files were preserved.'}
    $terminalId=Get-RemovalField $operation 'id'
    if(($terminalId -isnot [int] -and $terminalId -isnot [long]) -or $operationId -le 0 -or $terminalId -ne $operationId){throw 'Removal operation identity is invalid; files were preserved.'}
    $guard=Get-RemovalField $snapshot 'traffic_guard'
    foreach($field in @('armed','configured','protected')){Assert-RemovalBoolean $guard $field $false}
    Assert-RemovalBoolean $guard 'verified' $true
}
function Assert-RemovalPath([string]$Path,[string]$Root) {
    $pathFull=[IO.Path]::GetFullPath($Path)
    $rootFull=[IO.Path]::GetFullPath($Root)
    if($pathFull -ne $rootFull -and !$pathFull.StartsWith(($rootFull+'\'),[StringComparison]::OrdinalIgnoreCase)){throw 'Removal path is outside the managed installation'}
    $ancestors=New-Object 'Collections.Generic.Stack[string]'
    $cursor=$pathFull
    while($cursor -ne $rootFull) {
        $ancestors.Push($cursor)
        $cursor=[IO.Path]::GetDirectoryName($cursor)
        if(!$cursor){throw 'Removal path is not contained in the managed installation'}
    }
    if((Get-Item -LiteralPath $rootFull -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Removal root is a reparse point'}
    $writeMask=[Security.AccessControl.FileSystemRights]::Write -bor [Security.AccessControl.FileSystemRights]::Delete -bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor [Security.AccessControl.FileSystemRights]::ChangePermissions -bor [Security.AccessControl.FileSystemRights]::TakeOwnership
    while($ancestors.Count) {
        $cursor=$ancestors.Pop()
        $item=Get-Item -LiteralPath $cursor -Force
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Removal path contains a reparse point'}
        $acl=Get-Acl -LiteralPath $cursor
        if($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin @('S-1-5-18','S-1-5-32-544')){throw 'Removal path does not have a protected owner'}
        foreach($rule in $acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
            if($rule.AccessControlType -eq 'Allow' -and ($rule.FileSystemRights -band $writeMask) -and $rule.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544')){throw 'Removal path permits untrusted changes'}
        }
    }
}
function Read-RemovalJson([string]$Path,[int]$Limit=1048576) {
    $item=Get-Item -LiteralPath $Path -Force
    if($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.Length -gt $Limit){throw 'Removal metadata is invalid'}
    Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}
function Get-RemovalVersionFiles([string]$Program,[string]$Directory) {
    if([IO.Path]::GetDirectoryName($Directory) -ne $Program -or [IO.Path]::GetFileName($Directory) -cnotmatch '^app-(\d+\.\d+\.\d+)-[a-f0-9]{32}$'){throw 'Application version directory is not owned by Mukhomor'}
    $version=$Matches[1]
    Assert-RemovalPath $Directory $Program
    $manifestPath=Join-Path $Directory 'release-manifest.json'
    Assert-RemovalPath $manifestPath $Program
    $manifest=Read-RemovalJson $manifestPath
    if((Get-RemovalField $manifest 'version') -cne $version){throw 'Installed manifest version does not match the application'}
    $files=@(Get-RemovalField $manifest 'files')
    if(!$files.Count -or $files.Count -gt 4096){throw 'Installed manifest is invalid'}
    $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach($file in $files) {
        $relative=Get-RemovalField $file 'path';$hash=Get-RemovalField $file 'sha256'
        if($relative -isnot [string] -or !$relative -or [IO.Path]::IsPathRooted($relative) -or $relative -match '[:"\x00-\x1f]' -or $hash -isnot [string] -or $hash -cnotmatch '^[a-f0-9]{64}$'){throw 'Installed manifest entry is invalid'}
        $target=[IO.Path]::GetFullPath((Join-Path $Directory $relative))
        if(!$target.StartsWith(($Directory+'\'),[StringComparison]::OrdinalIgnoreCase) -or !$seen.Add($target)){throw 'Installed manifest contains an unsafe or duplicate path'}
        Assert-RemovalPath $target $Program
        if((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -cne $hash){throw 'Installed application verification failed; files were preserved.'}
    }
    if(!$seen.Contains((Join-Path $Directory 'mukhomor.exe'))){throw 'Installed release has no application executable'}
    [void]$seen.Add($manifestPath)
    @{files=$seen;version=$version;manifestHash=(Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash}
}
function Get-RemovalIdentity([string]$Program,[string]$Data) {
    Assert-RemovalPath $Program ([IO.Path]::GetDirectoryName($Program))
    Assert-RemovalPath $Data ([IO.Path]::GetDirectoryName($Data))
    # The native RPC test-worker authentication must never be used to uninstall.
    if(Test-Path -LiteralPath (Join-Path $Data 'qa.marker')){throw 'A test-worker installation cannot be removed by this helper'}
    $recordPath=Join-Path $Data 'installation.json'
    Assert-RemovalPath $recordPath $Data
    $record=Read-RemovalJson $recordPath 16384
    $version=Get-RemovalField $record 'version'
    $saved=Get-RemovalField $record 'executable'
    if($version -isnot [string] -or $version -notmatch '^\d+\.\d+\.\d+$' -or $saved -isnot [string] -or $saved -match '["\x00-\x1f]'){throw 'Stored application identity is invalid'}
    $executable=[IO.Path]::GetFullPath($saved)
    $directory=[IO.Path]::GetDirectoryName($executable)
    if([IO.Path]::GetFileName($executable) -cne 'mukhomor.exe'){throw 'Stored application executable is not owned by Mukhomor'}
    $release=Get-RemovalVersionFiles $Program $directory
    if($release.version -cne $version){throw 'Installed release does not match the stored identity'}
    foreach($required in @('mukhomor.exe','NativeBridge.ps1','SplitVpn.psm1','SplitWindows.psm1','TrafficGuard.psm1','ProfileImport.psm1','bin\mihomo-windows-amd64-compatible.exe','bin\wintun.dll','assets\settings.default.json')) {
        if(!$release.files.Contains((Join-Path $directory $required))){throw 'The installed release is incomplete; repair or upgrade Mukhomor before removal.'}
    }
    @{executable=$executable;command='"'+$executable+'" --service --root "'+$Data+'"';recordHash=(Get-FileHash -LiteralPath $recordPath -Algorithm SHA256).Hash;manifestHash=$release.manifestHash}
}
function Get-RemovalVersionTree([string]$Program,[string]$Directory) {
    $release=Get-RemovalVersionFiles $Program $Directory
    $pending=New-Object 'Collections.Generic.Queue[string]'
    $pending.Enqueue($Directory)
    $items=New-Object 'Collections.Generic.List[object]'
    $count=0
    while($pending.Count) {
        $parent=$pending.Dequeue()
        Assert-RemovalPath $parent $Program
        # Non-recursive enumeration only after each ancestor is verified.
        foreach($entry in [IO.Directory]::EnumerateFileSystemEntries($parent)) {
            $count++
            if($count -gt 10000){throw 'Application version exceeds the removal bound'}
            $full=[IO.Path]::GetFullPath($entry)
            if([IO.Path]::GetDirectoryName($full) -ne $parent){throw 'Application version contains an unsafe entry'}
            Assert-RemovalPath $full $Program
            $item=Get-Item -LiteralPath $full -Force
            if($item.PSIsContainer) {
                $ownedDirectory=$false
                foreach($file in $release.files){if($file.StartsWith(($full+'\'),[StringComparison]::OrdinalIgnoreCase)){$ownedDirectory=$true;break}}
                if(!$ownedDirectory){throw 'Application version contains an unlisted directory'}
                $pending.Enqueue($full)
            } elseif(!$release.files.Contains($full)){throw 'Application version contains an unlisted file'}
            $items.Add($item)
        }
    }
    @{items=$items;root=(Get-Item -LiteralPath $Directory -Force)}
}
function Test-RemovalOwnedFile($Path,[string]$Program) {
    try {
        if($Path -isnot [string] -or !$Path){return $false}
        $full=[IO.Path]::GetFullPath($Path)
        if(!$full.StartsWith(($Program+'\'),[StringComparison]::OrdinalIgnoreCase)){return $false}
        $directory=[IO.Path]::GetDirectoryName($full)
        while($directory -and [IO.Path]::GetDirectoryName($directory) -ne $Program) {
            if($directory -eq $Program){return $false}
            $directory=[IO.Path]::GetDirectoryName($directory)
        }
        if(!$directory){return $false}
        $release=Get-RemovalVersionFiles $Program $directory
        return $release.files.Contains($full)
    } catch {return $false}
}
function Assert-RemovalService($Definition,$Identity,[switch]$RequireStopped) {
    if(!$Definition -or $Definition.Name -cne 'Mukhomor' -or $Definition.PathName -cne $Identity.command -or $Definition.StartName -notin @('LocalSystem','NT AUTHORITY\SYSTEM') -or $Definition.ServiceType -ne 'Own Process'){throw 'The controller is not the verified Mukhomor SYSTEM service'}
    if($Definition.StartMode -notin @('Auto','Automatic','Manual','Disabled') -or $Definition.ProcessId -lt 0){throw 'The controller state is invalid'}
    if($RequireStopped) {
        if($Definition.State -ne 'Stopped' -or $Definition.ProcessId -ne 0 -or $Definition.StartMode -ne 'Disabled'){throw 'The verified controller has not stopped; files were preserved.'}
    } elseif($Definition.ProcessId -gt 0) {
        $process=Get-Process -Id $Definition.ProcessId -ErrorAction Stop
        if(!$process.Path -or [IO.Path]::GetFullPath($process.Path) -cne $Identity.executable){throw 'The running controller image does not match the installed application'}
    } elseif($Definition.State -ne 'Stopped'){throw 'The controller process cannot be verified'}
}
function Set-RemovalStartMode($Identity,[string]$Mode) {
    $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    Assert-RemovalService $definition $Identity
    $result=Invoke-CimMethod -InputObject $definition -MethodName Change -Arguments @{StartMode=$Mode}
    if($result.ReturnValue -ne 0){throw 'Unable to suspend or restore the verified controller startup'}
    $updated=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    Assert-RemovalService $updated $Identity
    $expected=if($Mode -eq 'Automatic'){'Auto'}else{$Mode}
    if($updated.StartMode -ne $expected -and $updated.StartMode -ne $Mode){throw 'Controller startup change could not be verified'}
}
function Write-RemovalIntent([string]$Data) {
    $runtime=Join-Path $Data 'runtime'
    [IO.Directory]::CreateDirectory($runtime)|Out-Null
    Assert-RemovalPath $runtime $Data
    $path=Join-Path $runtime 'connection-intent.json'
    $generation=[long]0
    if(Test-Path -LiteralPath $path) {
        Assert-RemovalPath $path $Data
        $old=Read-RemovalJson $path 16384
        $generation=Get-RemovalField $old 'generation'
        if(($generation -isnot [int] -and $generation -isnot [long]) -or $generation -lt 0 -or $generation -eq [long]::MaxValue){throw 'Saved connection intent is invalid'}
        $generation=[long]$generation+1
    }
    $temporary=$path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    $backup=$path+'.'+[guid]::NewGuid().ToString('N')+'.backup'
    try {
        [IO.File]::WriteAllText($temporary,(@{schema=1;generation=$generation;desired=$false}|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
        if([IO.File]::Exists($path)){[IO.File]::Replace($temporary,$path,$backup)}else{[IO.File]::Move($temporary,$path)}
    } finally {foreach($ownedFile in @($temporary,$backup)){if([IO.File]::Exists($ownedFile)){[IO.File]::Delete($ownedFile)}}}
}
function Invoke-RemovalExit($Identity,[string]$Data) {
    $private=Join-Path $Data 'private'
    Assert-RemovalPath $private $Data
    $requestId='uninstall-'+[guid]::NewGuid().ToString('N')
    $request=Join-Path $private ($requestId+'.request.json')
    $output=Join-Path $private ($requestId+'.reply.json')
    $client=$null
    try {
        [IO.File]::WriteAllText($request,(@{action='Exit';request_id=$requestId}|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
        $client=Start-Process -FilePath $Identity.executable -ArgumentList @('--rpc','--root',('"'+$Data+'"'),'--request',('"'+$request+'"'),'--output',('"'+$output+'"')) -WindowStyle Hidden -PassThru
        [void]$client.Handle
        if(!$client.WaitForExit(110000)){throw 'The verified Exit request timed out; files were preserved.'}
        $client.Refresh()
        if($client.ExitCode -isnot [int] -or $client.ExitCode -ne 0){throw 'The verified Exit request failed; files were preserved.'}
        Assert-RemovalPath $output $Data
        Assert-RemovalReceipt (Read-RemovalJson $output 4194304) $requestId
    } finally {
        # This is only the short-lived, owned RPC client, never the controller.
        if($client -and !$client.HasExited){$client.Kill();[void]$client.WaitForExit(5000)}
        foreach($file in @($request,$output)){if([IO.File]::Exists($file)){[IO.File]::Delete($file)}}
        if($client){$client.Dispose()}
    }
}
function Remove-VerifiedController([string]$Program,[string]$Data) {
    $owned=Get-RemovalIdentity $Program $Data
    $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    Assert-RemovalService $definition $owned
    $initialMode=if($definition.StartMode -eq 'Auto'){'Automatic'}else{$definition.StartMode}
    $startupChanged=$false
    try {
        $service=Get-Service -Name 'Mukhomor' -ErrorAction Stop
        if($definition.State -eq 'Stopped') {
            # A stopped controller must not reconnect while being started solely
            # to perform authenticated SYSTEM-owned WFP teardown.
            Write-RemovalIntent $Data
            if($initialMode -eq 'Disabled'){$startupChanged=$true;Set-RemovalStartMode $owned 'Manual'}
            Start-Service -Name 'Mukhomor'
            $service.WaitForStatus('Running',[timespan]::FromSeconds(45))
        } elseif($definition.State -ne 'Running'){throw 'Wait for the controller operation to finish before removing Mukhomor'}
        $startupChanged=$true
        Set-RemovalStartMode $owned 'Disabled'
        Invoke-RemovalExit $owned $Data
        $service.WaitForStatus('Stopped',[timespan]::FromSeconds(60))
        $current=Get-RemovalIdentity $Program $Data
        if($current.executable -cne $owned.executable -or $current.recordHash -cne $owned.recordHash -or $current.manifestHash -cne $owned.manifestHash){throw 'Installed identity changed during removal; files were preserved.'}
        $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
        Assert-RemovalService $definition $owned -RequireStopped
        & sc.exe delete 'Mukhomor'|Out-Null
        if($LASTEXITCODE -ne 0){throw 'Unable to remove the verified service; files were preserved.'}
    } catch {
        $failure=$_
        # Never restart or restore desired=true. Restore startup only while the
        # exact original service identity remains verifiable.
        if($startupChanged){try {Set-RemovalStartMode $owned $initialMode} catch {}}
        throw $failure
    }
}
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
if (!(New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {throw 'Windows administrator approval is required to remove Mukhomor'}
$program=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Mukhomor'))
$data=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Mukhomor'))
Remove-VerifiedController $program $data
# The controller identity and teardown are verified before registry removal.
. (Join-Path $PSScriptRoot 'AppStartup.ps1')
$startupOwnerPath=Join-Path $data 'owner.sid'
Assert-RemovalPath $startupOwnerPath $data
$startupOwner=[IO.File]::ReadAllText($startupOwnerPath).Trim()
Update-AppStartupRegistration $startupOwner $program '' -Remove
# The completed Exit has already persisted disconnect intent and restored DNS.
# Preserve that monotonic private state together with profiles and settings.
# Close our remaining GUI instances, keeping the current removal helper alive.
$parent=(Get-CimInstance Win32_Process -Filter ('ProcessId='+$PID)).ParentProcessId
foreach($process in @(Get-Process -Name 'mukhomor' -ErrorAction SilentlyContinue)) {
    if($process.Id -ne $parent -and $process.Path -and [IO.Path]::GetFileName($process.Path) -ceq 'mukhomor.exe' -and (Test-RemovalOwnedFile $process.Path $program)) {Stop-Process -Id $process.Id -Force}
}
$shortcuts=@((Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Mukhomor.lnk'),(Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Mukhomor.lnk'))
foreach($shortcut in $shortcuts){if(Test-Path -LiteralPath $shortcut){
    $shell=New-Object -ComObject WScript.Shell
    $shortcutTarget=$shell.CreateShortcut($shortcut).TargetPath
    if([IO.Path]::GetFileName($shortcutTarget) -ceq 'mukhomor.exe' -and (Test-RemovalOwnedFile $shortcutTarget $program)){[IO.File]::Delete($shortcut)}
}}
$uninstallKey='HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Mukhomor'
if(Test-Path -LiteralPath $uninstallKey){$record=Get-ItemProperty -LiteralPath $uninstallKey;if(Test-RemovalOwnedFile (Join-Path $record.InstallLocation 'mukhomor.exe') $program){Remove-Item -LiteralPath $uninstallKey}}
foreach($rule in @(Get-NetFirewallRule -Group 'Mukhomor' -ErrorAction SilentlyContinue)){
    $application=Get-NetFirewallApplicationFilter -AssociatedNetFirewallRule $rule
    $interface=Get-NetFirewallInterfaceFilter -AssociatedNetFirewallRule $rule
    if($rule.Name -match '^Mukhomor-TUN-[A-F0-9]{16}$' -and (Test-RemovalOwnedFile $application.Program $program) -and @($interface.InterfaceAlias) -contains 'Mukhomor') {Remove-NetFirewallRule -InputObject $rule}
}
# Remove only manifest-owned version directories. A running EXE is scheduled for
# deletion by Windows on reboot; the private ProgramData directory is preserved.
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class MukhomorRemoval {
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
 public static extern bool MoveFileEx(string existing, string target, uint flags);
 // Pass a true native null inside C#: PowerShell converts a string $null to "".
 public static bool ScheduleDelete(string existing) { return MoveFileEx(existing, null, 4); }
}
'@
if(Test-Path -LiteralPath $program) {
    Assert-RemovalPath $program ([IO.Path]::GetDirectoryName($program))
    $validated=New-Object 'Collections.Generic.List[object]'
    $preservedVersions=$false
    foreach($version in @(Get-ChildItem -LiteralPath $program -Directory)) {
        $target=[IO.Path]::GetFullPath($version.FullName)
        if([IO.Path]::GetDirectoryName($target) -ne $program -or $version.Name -notmatch '^app-\d+\.\d+\.\d+-[a-f0-9]{32}$'){continue}
        try {$validated.Add((Get-RemovalVersionTree $program $target))} catch {
            $preservedVersions=$true
            Write-Warning 'An unverified application version was preserved; no files in that version were removed.'
        }
    }
    foreach($tree in $validated) {
        $items=@($tree.items)
        foreach($file in @($items | Where-Object {!$_.PSIsContainer})) {
            try {Remove-Item -LiteralPath $file.FullName -Force} catch {
                if(![MukhomorRemoval]::ScheduleDelete($file.FullName)){throw ('Unable to schedule removal of '+$file.Name)}
            }
        }
        foreach($directory in @($items | Where-Object {$_.PSIsContainer} | Sort-Object {$_.FullName.Length} -Descending)+@($tree.root)) {
            try {[IO.Directory]::Delete($directory.FullName,$false)} catch {
                if(![MukhomorRemoval]::ScheduleDelete($directory.FullName)){throw 'Unable to schedule directory removal'}
            }
        }
    }
    if(!$preservedVersions){try {[IO.Directory]::Delete($program,$false)} catch { [MukhomorRemoval]::ScheduleDelete($program) | Out-Null }}
}
Write-Host 'Mukhomor removed. Profiles and settings were preserved. Locked application files will be removed after restarting Windows.'
