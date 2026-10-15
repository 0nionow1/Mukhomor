param(
    [string]$WslDistribution='Ubuntu',
    [string]$Sdk=$env:ANDROID_HOME,
    [string]$Ndk=$env:ANDROID_NDK_HOME,
    [string]$Java=$env:JAVA_HOME,
    [string]$Go='go',
    [string]$Python='python',
    [switch]$DebugAndroid,
    [switch]$Mycelia,
    [switch]$Offline,
    [string]$Keystore,
    [string]$KeyAlias='mukhomor',
    [hashtable]$LinuxEnvironment=@{}
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$version=[regex]::Match([IO.File]::ReadAllText((Join-Path $root 'native\Cargo.toml')),'(?m)^version = "(\d+\.\d+\.\d+)"').Groups[1].Value
if(!$version){throw 'Missing version'}
if(!$Sdk -or !$Ndk -or !$Java){throw 'All-platform builds require Android SDK, NDK and JDK paths.'}
$failures=New-Object 'Collections.Generic.List[string]'
function Run-Platform([string]$Name,[string]$Program,[string[]]$Arguments){
    Write-Host ('Building '+$Name+' '+$version)
    try {
        $previous=$ErrorActionPreference
        try{$ErrorActionPreference='Continue'; & $Program @Arguments; $code=$LASTEXITCODE}
        finally{$ErrorActionPreference=$previous}
        if($code -ne 0){throw ($Name+' build failed ('+$code+')')}
    } catch { $failures.Add($_.Exception.Message) }
}
$windows=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $root 'Build-Release.ps1'))
if($Offline){$windows+='-Offline'}
Run-Platform 'Windows x64' 'powershell.exe' $windows
$android=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $root 'Build-Android.ps1'),'-Sdk',$Sdk,'-Ndk',$Ndk,'-Java',$Java,'-Go',$Go,'-Python',$Python)
if($DebugAndroid){$android+='-DebugBuild'}
if($Mycelia){$android+='-Mycelia'}
if($Keystore){$android+=@('-Keystore',$Keystore,'-KeyAlias',$KeyAlias)}
Run-Platform 'Android ARM64 + x86_64' 'powershell.exe' $android
try {
    $linuxRoot=(& wsl.exe -d $WslDistribution -- wslpath -a $root.Replace('\','/')).Trim()
    if($LASTEXITCODE -ne 0 -or !$linuxRoot.StartsWith('/')){throw 'Cannot resolve project path in WSL'}
    $linux=@('-d',$WslDistribution,'--cd',$linuxRoot,'--','env')
    foreach($entry in $LinuxEnvironment.GetEnumerator()){
        if($entry.Key -notmatch '^[A-Z_][A-Z0-9_]*$'){throw 'Invalid Linux environment variable name'}
        $linux+=([string]$entry.Key+'='+[string]$entry.Value)
    }
    if($Mycelia){$linux+='MUKHOMOR_MYCELIA=1'}
    $linux+=@('bash','./Build-Linux.sh')
    Run-Platform 'Linux' 'wsl.exe' $linux
} catch { $failures.Add($_.Exception.Message) }
if($failures.Count){throw ($failures -join "`n")}
Write-Host ('All three platforms built: '+(Join-Path $root ('release-staging\'+$version)))
