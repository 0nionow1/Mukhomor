Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
if (!(New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {throw 'Windows administrator approval is required to remove Mukhomor'}
$program=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Mukhomor'))
$data=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Mukhomor'))
$service=Get-Service -Name 'Mukhomor' -ErrorAction SilentlyContinue
if($service){
    $definition=Get-CimInstance Win32_Service -Filter "Name='Mukhomor'"
    $record=Get-Content -LiteralPath (Join-Path $data 'installation.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $installed=[IO.Path]::GetFullPath($record.executable)
    if($definition.PathName -ne ('"'+$installed+'" --service --root "'+$data+'"') -or [IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($installed)) -ne $program){throw 'The controller is not owned by this application'}
    if($service.Status -ne 'Stopped'){Stop-Service -Name 'Mukhomor';$service.WaitForStatus('Stopped',[timespan]::FromSeconds(100))}
    if(Test-Path -LiteralPath (Join-Path $data 'runtime\dns-backup.json')) {
        Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -Force -DisableNameChecking
        Import-Module (Join-Path $PSScriptRoot 'SplitWindows.psm1') -Force -DisableNameChecking
        Restore-SessionDns $data
    }
    & sc.exe delete 'Mukhomor'|Out-Null
    if($LASTEXITCODE -ne 0){throw 'Unable to remove service'}
}
$installation=Join-Path $data 'installation.json'
if(Test-Path -LiteralPath $installation) {
    $saved=Get-Content -LiteralPath $installation -Raw -Encoding UTF8 | ConvertFrom-Json
    $savedPath=[IO.Path]::GetFullPath($saved.executable)
    if([IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($savedPath)) -ne $program -or [IO.Path]::GetFileName($savedPath) -ne 'mukhomor.exe'){throw 'Stored application identity is not owned by Mukhomor'}
    Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -Force -DisableNameChecking
    Write-AtomicJson (Join-Path $data 'runtime\connection-intent.json') @{schema=1;generation=0;desired=$false}
}
# Close our remaining GUI instances, keeping the current removal helper alive.
$parent=(Get-CimInstance Win32_Process -Filter ('ProcessId='+$PID)).ParentProcessId
foreach($process in @(Get-Process -Name 'mukhomor' -ErrorAction SilentlyContinue)) {
    if($process.Id -ne $parent -and $process.Path -and $process.Path.StartsWith(($program+'\app-'),[StringComparison]::OrdinalIgnoreCase)) {Stop-Process -Id $process.Id -Force}
}
$shortcuts=@((Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Mukhomor.lnk'),(Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Mukhomor.lnk'))
foreach($shortcut in $shortcuts){if(Test-Path -LiteralPath $shortcut){
    $shell=New-Object -ComObject WScript.Shell
    if($shell.CreateShortcut($shortcut).TargetPath.StartsWith(($program+'\'),[StringComparison]::OrdinalIgnoreCase)){[IO.File]::Delete($shortcut)}
}}
$uninstallKey='HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Mukhomor'
if(Test-Path -LiteralPath $uninstallKey){$record=Get-ItemProperty -LiteralPath $uninstallKey;if($record.InstallLocation.StartsWith(($program+'\app-'),[StringComparison]::OrdinalIgnoreCase)){Remove-Item -LiteralPath $uninstallKey}}
foreach($rule in @(Get-NetFirewallRule -Group 'Mukhomor' -ErrorAction SilentlyContinue)){
    $application=Get-NetFirewallApplicationFilter -AssociatedNetFirewallRule $rule
    $interface=Get-NetFirewallInterfaceFilter -AssociatedNetFirewallRule $rule
    if($rule.Name -match '^Mukhomor-TUN-[A-F0-9]{16}$' -and $application.Program.StartsWith(($program+'\'),[StringComparison]::OrdinalIgnoreCase) -and @($interface.InterfaceAlias) -contains 'Mukhomor') {Remove-NetFirewallRule -InputObject $rule}
}
# Remove only manifest-owned version directories. A running EXE is scheduled for
# deletion by Windows on reboot; the private ProgramData directory is preserved.
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class MukhomorRemoval {
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
 public static extern bool MoveFileEx(string existing, string target, uint flags);
}
'@
if(Test-Path -LiteralPath $program) {
    if((Get-Item -LiteralPath $program).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Application directory is a reparse point'}
    foreach($version in @(Get-ChildItem -LiteralPath $program -Directory)) {
        $target=[IO.Path]::GetFullPath($version.FullName)
        if([IO.Path]::GetDirectoryName($target) -ne $program -or $version.Name -notmatch '^app-\d+\.\d+\.\d+-[a-f0-9]{32}$'){continue}
        if(!(Test-Path -LiteralPath (Join-Path $target 'release-manifest.json'))){continue}
        $items=@(Get-ChildItem -LiteralPath $target -Recurse -Force)
        if(@($items | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}).Count -or ($version.Attributes -band [IO.FileAttributes]::ReparsePoint)){throw 'Application version contains a reparse point'}
        foreach($file in @($items | Where-Object {!$_.PSIsContainer})) {
            try {Remove-Item -LiteralPath $file.FullName -Force} catch {
                if(![MukhomorRemoval]::MoveFileEx($file.FullName,$null,4)){throw ('Unable to schedule removal of '+$file.Name)}
            }
        }
        foreach($directory in @($items | Where-Object {$_.PSIsContainer} | Sort-Object {$_.FullName.Length} -Descending)+@($version)) {
            try {[IO.Directory]::Delete($directory.FullName,$false)} catch {
                if(![MukhomorRemoval]::MoveFileEx($directory.FullName,$null,4)){throw 'Unable to schedule directory removal'}
            }
        }
    }
    try {[IO.Directory]::Delete($program,$false)} catch { [MukhomorRemoval]::MoveFileEx($program,$null,4) | Out-Null }
}
Write-Host 'Mukhomor removed. Profiles and settings were preserved. Locked application files will be removed after restarting Windows.'
