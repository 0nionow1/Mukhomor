param([ValidateSet('windows','android-arm64-v8a','android-x86_64')][string]$Platform='windows',[string]$LinuxNdk=$env:MUKHOMOR_LINUX_NDK)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$linuxRoot=(& wsl.exe -d Ubuntu -- wslpath -a $repo.Replace('\','/')).Trim()
if($LASTEXITCODE -ne 0 -or !$linuxRoot.StartsWith('/')){throw 'Cannot locate the I2P source tree in WSL'}
$arguments=@('-d','Ubuntu','--','python3',($linuxRoot+'/i2p/build.py'),'--platform',$Platform,'--jobs','4')
if($Platform.StartsWith('android-')){
    if(!$LinuxNdk){$LinuxNdk=$linuxRoot+'/target/i2p-sources/android-ndk-r27c'}
    $arguments+=@('--ndk',$LinuxNdk)
}
& wsl.exe @arguments
if($LASTEXITCODE -ne 0){throw 'I2P native build failed'}
