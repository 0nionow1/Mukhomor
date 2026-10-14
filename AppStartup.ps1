# Upgrade/removal touch only an existing registration in the installation owner's hive.
function Get-AppStartupCommand([string]$Executable) {
    if ($Executable -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+[\\/])' -or $Executable -match '["\x00\r\n]' -or [IO.Path]::GetFileName($Executable) -cne 'mukhomor.exe') {throw 'Invalid startup executable'}
    $command='"'+$Executable+'" --startup'
    if ($command.Length -gt 260) {throw 'Startup command exceeds Windows length limit'}
    return $command
}
function Test-AppStartupCommand($Command,[string]$Program) {
    try {
    if ($Command -isnot [string] -or $Command -cnotmatch '^"([^"\x00\r\n]+)" --startup$') {return $false}
    $path=$Matches[1]
    if ($path -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+[\\/])' -or [IO.Path]::GetFileName($path) -cne 'mukhomor.exe') {return $false}
    $parent=[IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($path))
    return ([IO.Path]::GetFileName($parent) -cmatch '^app-\d+\.\d+\.\d+-[a-fA-F0-9]{32}$' -and [IO.Path]::GetDirectoryName($parent).Equals([IO.Path]::GetFullPath($Program),[StringComparison]::OrdinalIgnoreCase))
    } catch {return $false}
}
function Update-AppStartupKey($Key,[string]$Program,[string]$Executable,[switch]$Remove) {
    if ($null -eq $Key) {return}
    $old=$Key.GetValue('Mukhomor',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if (!(Test-AppStartupCommand $old $Program)) {return}
    if ($Remove) {$Key.DeleteValue('Mukhomor',$false)}
    else {$Key.SetValue('Mukhomor',(Get-AppStartupCommand $Executable),[Microsoft.Win32.RegistryValueKind]::String)}
}
function Update-AppStartupRegistration([string]$OwnerSid,[string]$Program,[string]$Executable,[switch]$Remove) {
    if ($OwnerSid -notmatch '^S-1-(\d+-)+\d+$') {throw 'Invalid startup owner SID'}
    $owner=[Microsoft.Win32.Registry]::Users.OpenSubKey($OwnerSid)
    if (!$owner) {Write-Warning 'The installation owner is not signed in; their startup registration could not be updated.';return}
    $owner.Dispose()
    $key=[Microsoft.Win32.Registry]::Users.OpenSubKey(($OwnerSid+'\Software\Microsoft\Windows\CurrentVersion\Run'),$true)
    try {Update-AppStartupKey $key $Program $Executable -Remove:$Remove} finally {if ($key) {$key.Dispose()}}
}
