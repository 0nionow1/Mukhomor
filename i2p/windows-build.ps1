param([ValidateSet('windows','android-arm64-v8a','android-x86_64')][string]$Platform='windows',[string]$LinuxNdk=$env:MUKHOMOR_LINUX_NDK,[string]$PrebuiltRoot=$env:MUKHOMOR_I2P_PREBUILT_ROOT)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
if($PrebuiltRoot){
    $artifact=Join-Path ([IO.Path]::GetFullPath($PrebuiltRoot)) $Platform
    $manifest=Get-Content -LiteralPath (Join-Path $artifact 'manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $name=if($Platform -eq 'windows'){'mukhomor-i2pd.exe'}else{'mukhomor-i2pd'}
    if($manifest.schema -ne 1 -or $manifest.platform -cne $Platform -or $manifest.name -cne $name -or $manifest.sha256 -cnotmatch '^[a-f0-9]{64}$' -or $manifest.size -le 0 -or $manifest.size -gt 134217728){throw 'Invalid I2P CI artifact manifest'}
    $inputs=@('i2p/main.cpp','i2p/CMakeLists.txt','i2p/build.py','i2p/prepare.py','i2p/dependencies.sha256')
    if((@($manifest.inputs.PSObject.Properties.Name | Sort-Object) -join '|') -cne (@($inputs | Sort-Object) -join '|')){throw 'Unexpected I2P artifact build inputs'}
    $sha=[Security.Cryptography.SHA256]::Create()
    try {
        foreach($relative in $inputs){
            $text=[IO.File]::ReadAllText((Join-Path $repo $relative)).Replace("`r`n","`n")
            $actual=([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($text)))).Replace('-','').ToLowerInvariant()
            if($manifest.inputs.$relative -cne $actual){throw ('I2P CI artifact source mismatch: '+$relative)}
        }
    } finally {$sha.Dispose()}
    $binary=Join-Path $artifact $name
    $item=Get-Item -LiteralPath $binary
    if($item.Length -ne $manifest.size -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() -cne $manifest.sha256){throw 'I2P CI artifact checksum mismatch'}
    $destination=Join-Path $repo ('target\i2p-sources\'+$Platform+'\native')
    [IO.Directory]::CreateDirectory($destination)|Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $destination $name) -Force
    Write-Host ('Verified same-run I2P artifact: '+$Platform)
    return
}
$linuxRoot=(& wsl.exe -d Ubuntu -- wslpath -a $repo.Replace('\','/')).Trim()
if($LASTEXITCODE -ne 0 -or !$linuxRoot.StartsWith('/')){throw 'Cannot locate the I2P source tree in WSL'}
$arguments=@('-d','Ubuntu','--','python3',($linuxRoot+'/i2p/build.py'),'--platform',$Platform,'--jobs','4')
if($Platform.StartsWith('android-')){
    if(!$LinuxNdk){$LinuxNdk=$linuxRoot+'/target/i2p-sources/android-ndk-r27c'}
    $arguments+=@('--ndk',$LinuxNdk)
}
& wsl.exe @arguments
if($LASTEXITCODE -ne 0){throw 'I2P native build failed'}
