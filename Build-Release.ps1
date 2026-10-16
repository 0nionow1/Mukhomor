param([string]$Version='',[switch]$Offline)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -Force -DisableNameChecking
Import-Module (Join-Path $PSScriptRoot 'ReleasePe.psm1') -Force -DisableNameChecking
$native=Join-Path $PSScriptRoot 'native'
if(!$Version){$match=[regex]::Match([IO.File]::ReadAllText((Join-Path $native 'Cargo.toml')),'(?m)^version = "(\d+\.\d+\.\d+)"$');if(!$match.Success){throw 'Missing Cargo version'};$Version=$match.Groups[1].Value}
if($Version -notmatch '^\d+\.\d+\.\d+$'){throw 'Invalid version'}
& powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'tests\Test-PublicSource.ps1')
if($LASTEXITCODE -ne 0){throw 'Public source verification failed'}
if((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'assets\fonts\Tiny5-Regular.ttf')).Hash.ToLowerInvariant() -ne 'cb8168f80cfee2f47f6db59f2a7afbde31cdcdcdcf262e7a993e4d468a5bf4b0'){throw 'Bundled pixel font checksum mismatch'}
if([IO.File]::ReadAllText((Join-Path $native 'Cargo.toml')) -notmatch ('(?m)^version = "'+[regex]::Escape($Version)+'"$')){throw 'Version must match native/Cargo.toml'}
$oldFlags=$env:CARGO_ENCODED_RUSTFLAGS
try {
    $env:CARGO_ENCODED_RUSTFLAGS=@('-C','target-feature=+crt-static',('--remap-path-prefix='+[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))+'=/project'),('--remap-path-prefix='+$env:USERPROFILE+'=/build-user')) -join [char]31
    Push-Location $native
    try {
        $buildArgs=@('build','--release','--locked','--bin','mukhomor')
        if($Offline){$buildArgs+='--offline'}
        & cargo.exe @buildArgs
        if($LASTEXITCODE -ne 0){throw 'Native build failed'}
    }finally{Pop-Location}
}finally{$env:CARGO_ENCODED_RUSTFLAGS=$oldFlags}
$releasePePin=$null
function Remove-OwnedBuildDirectory([string]$Path,[string]$Parent){
    if(!(Test-Path -LiteralPath $Path)){return}
    $resolved=(Resolve-Path -LiteralPath $Path).ProviderPath
    $expectedParent=[IO.Path]::GetFullPath($Parent).TrimEnd('\')
    if([IO.Path]::GetDirectoryName($resolved) -cne $expectedParent -or (Get-Item -LiteralPath $resolved).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Unsafe build cleanup path'}
    if(@(Get-ChildItem -LiteralPath $resolved -Recurse -Force | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}).Count){throw 'Reparse point in build directory'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
try {
$stage=Join-Path $PSScriptRoot ('target\release-build\'+$Version+'-'+[guid]::NewGuid().ToString('N'))
$runtime=Join-Path $stage 'windows\Mukhomor'
$source=Join-Path $stage 'source\Mukhomor'
foreach($path in @($runtime,$source)){[IO.Directory]::CreateDirectory($path)|Out-Null}
function Copy-Reviewed([string]$Relative,[string]$Destination){
    $input=Join-Path $PSScriptRoot $Relative
    $output=Join-Path $Destination $Relative
    if(!(Test-Path -LiteralPath $input -PathType Leaf)){throw ('Missing reviewed file: '+$Relative)}
    [IO.Directory]::CreateDirectory((Split-Path -Parent $output))|Out-Null
    [IO.File]::Copy($input,$output,$false)
}
$common=@('AppStartup.ps1','NativeBridge.ps1','SplitVpn.psm1','RulePresets.ps1','RuleNormalizer.cs','SplitWindows.psm1','TrafficGuard.psm1','ProfileImport.psm1','Install.ps1','Uninstall.ps1','README.md','Mihomo-LICENSE.txt','Wintun-LICENSE.txt','Rules-LICENSE.txt','ThirdParty-LICENSE.txt','Tiny5-LICENSE.txt','LICENSE','assets\settings.default.json','assets\README.ru.md','assets\preview.png','lists\sources.json','lists\seed-games.list','lists\seed-russia.list','lists\seed-steam.list','lists\seed-ozon.list')
foreach($file in $common){Copy-Reviewed $file $runtime;Copy-Reviewed $file $source}
Copy-Reviewed 'Slint-LICENSE.txt' $source
Copy-Reviewed 'native\src\app_update.rs' $source
Copy-Reviewed 'tests\linux-update.py' $source
Copy-Reviewed 'android\src\org\mukhomor\app\UpdateProvider.java' $source
Copy-Reviewed 'Slint-LICENSE.txt' $runtime
Copy-Reviewed 'ui\README.md' $runtime
$publication=@('ROADMAP.md','docs\performance.md','README.en.md','CHANGELOG.md','CONTRIBUTING.md','SECURITY.md','assets\github-banner.svg','assets\github-social-preview.png','assets\appearance-preview.png','docs\README.md','docs\getting-started.md','docs\gaming-and-routing.md','docs\protocols.md','docs\vps-and-vpn.md','docs\troubleshooting.md','docs\development.md','docs\architecture.md','docs\releasing.md','docs\credits.md')
foreach($file in $publication){Copy-Reviewed $file $runtime;Copy-Reviewed $file $source}
foreach($file in @('.gitattributes','.github\PULL_REQUEST_TEMPLATE.md','.github\ISSUE_TEMPLATE\bug_report.md','.github\ISSUE_TEMPLATE\feature_request.md','.github\workflows\ci.yml','.github\workflows\release.yml','tests\Test-PublicSource.ps1','tests\Run-Checks.ps1')){Copy-Reviewed $file $source}
foreach($file in @('.gitignore','Build-Release.ps1','ReleasePe.psm1','Fetch-Dependencies.ps1','native\Cargo.toml','native\Cargo.lock','native\build.rs','native\windows.manifest','native\README.md','assets\theme.json','assets\fonts\Tiny5-Regular.ttf','assets\fonts\README.md')){Copy-Reviewed $file $source}
foreach($file in @('ui\Cargo.toml','ui\Cargo.lock','ui\build.rs','ui\README.md','ui\src\lib.rs','ui\src\state.rs','ui\src\preview.rs','ui\src\visuals.rs','native\visual_build.rs','ui\views\app.slint','native\src\slint_host.rs')){Copy-Reviewed $file $source}
foreach($name in @('main.rs','desktop.rs','backend.rs','profile_store.rs','autostart.rs','platform.rs','service.rs','setup.rs','setup_recovery.rs','cleanup_request.rs','hero_motion.rs','presets.rs','i18n.rs','design.rs','appearance.rs','mushroom.rs','traffic_guard.rs','traffic_guard_cli.rs','traffic_guard_tun.rs')){Copy-Reviewed ('native\src\'+$name) $source}
foreach($name in @('Test-AppStartup.ps1','Test-SplitVpn.ps1','Test-RulePresets.ps1','Test-ProfileImport.ps1','Test-ProtocolTransport.ps1','Test-CoreRouting.ps1','Test-LargeRules.ps1','Measure-Routing.ps1','Measure-NativeControl.ps1','routing-benchmark.cjs','Test-UdpRouting.ps1','Test-NativeApp.ps1','Test-Disconnect.ps1','Test-Isolation.ps1','Test-ReleasePrivacy.ps1','Test-PixelFont.ps1','Test-Lifecycle.ps1','Test-Upgrade.ps1','Test-Transport.ps1','Test-StartupRecovery.ps1','Test-StartupCancellation.ps1','Test-DnsRecovery.ps1','udp-client.cjs','local-origin.cjs','proxy-client.cjs')){if(Test-Path -LiteralPath (Join-Path $PSScriptRoot ('tests\'+$name))){Copy-Reviewed ('tests\'+$name) $source}}
$nativeTarget=if($env:CARGO_TARGET_DIR){[IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)}else{Join-Path $native 'target'}
[IO.File]::Copy((Join-Path $nativeTarget 'release\mukhomor.exe'),(Join-Path $runtime 'mukhomor.exe'),$false)
# Inspect and hash the packaged bytes through one READ/shareREAD handle.
# Retain it through manifests and ZIP creation; dispose on every exit.
$releasePePin=Open-ReleasePe (Join-Path $runtime 'mukhomor.exe')
foreach($name in @('mihomo-windows-amd64-compatible.exe','wintun.dll')){Copy-Reviewed ('bin\'+$name) $runtime}
$pins=@{'bin\mihomo-windows-amd64-compatible.exe'='04f8d7fc2b314771e1ecbf15951d59f0e1cb7914503c88cfb6972ff6a824e314';'bin\wintun.dll'='e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce'}
foreach($entry in $pins.GetEnumerator()){if((Get-FileHash -LiteralPath (Join-Path $runtime $entry.Key)).Hash.ToLowerInvariant() -ne $entry.Value){throw ('Dependency checksum mismatch: '+$entry.Key)}}
$coreSource='Source\mihomo-v1.19.32-source.zip'
if(!(Test-Path -LiteralPath (Join-Path $PSScriptRoot $coreSource))){throw 'Run Fetch-Dependencies.ps1 to obtain the corresponding Mihomo source archive'}
if((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $coreSource)).Hash.ToLowerInvariant() -ne '135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1'){throw 'Mihomo source checksum mismatch'}
Copy-Reviewed $coreSource $runtime;Copy-Reviewed $coreSource $source
if(Test-Path -LiteralPath (Join-Path $PSScriptRoot 'bin\mycelium-build.json')){
    $mycelium=Read-JsonFile (Join-Path $PSScriptRoot 'bin\mycelium-build.json')
    if($mycelium.schema -ne 1 -or $mycelium.protocol_version -ne 1 -or $mycelium.base_version -ne '1.19.32'){throw 'Invalid Mycelium build provenance'}
    foreach($pair in @(@('bin\mihomo-mycelium.exe','core_sha256'),@('Source\mihomo-mycelium-source.zip','source_sha256'))){
        if($mycelium[$pair[1]] -cnotmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $pair[0])).Hash.ToLowerInvariant() -cne $mycelium[$pair[1]]){throw 'Mycelium build checksum mismatch'}
    }
    Copy-Reviewed 'bin\mihomo-mycelium.exe' $runtime
    Copy-Reviewed 'bin\mycelium-build.json' $runtime
    foreach($target in @($runtime,$source)){
        Copy-Reviewed 'Source\mihomo-mycelium-source.zip' $target
        [IO.File]::Copy((Join-Path $PSScriptRoot 'bin\mycelium-build.json'),(Join-Path $target 'Source\mycelium-build.json'),$false)
    }
}
Copy-Reviewed 'tests\Test-MyceliumImport.ps1' $source
Copy-Reviewed 'tests\Test-BundledRouting.ps1' $source
Copy-Reviewed 'tests\Test-MyceliumLifecycle.ps1' $source
Copy-Reviewed 'tests\Test-FullRouting.ps1' $source
Copy-Reviewed 'tests\Test-FullTrafficGuardLifecycle.ps1' $source
Copy-Reviewed 'tests\Test-PrivateAcl.ps1' $source
Copy-Reviewed 'tests\Test-NativeGuardRecovery.ps1' $source
Copy-Reviewed 'tests\Test-Uninstall.ps1' $source
Copy-Reviewed 'tests\Test-ControllerRecovery.ps1' $source
Copy-Reviewed 'tests\Test-ReleasePe.ps1' $source
Copy-Reviewed 'tests\Set-OwnedTestRoot.ps1' $source
Copy-Reviewed 'tests\Test-SlintStartup.ps1' $source
Copy-Reviewed 'tests\Test-Performance.ps1' $source
Copy-Reviewed 'Build-All.ps1' $source
Copy-Reviewed 'tests\Test-PortableReleasePrivacy.py' $source
foreach($file in @('Build-Android.ps1','Build-Linux.sh','Prepare-PortableCore.py','Package-PortableSource.py','go.mod','go.sum','portable_assets.go','docs\platforms.md','tests\linux-network.py','tests\android-probe.c','tests\android-fixture.py','native\src\lib.rs','native\src\portable.rs','native\src\android.rs','core\controller.go','core\controller_test.go','core\import.go','core\node_fields.go','core\openvpn.go','core\rules.go','core\storage.go','core\update.go','cmd\mukhomor-android\bridge.c','cmd\mukhomor-android\main.go','cmd\mukhomor-controller\main.go','android\AndroidManifest.xml','android\res\drawable\ic_mushroom.xml','android\src\org\mukhomor\app\MainActivity.java','android\src\org\mukhomor\app\MukhomorVpnService.java','android\src\org\mukhomor\app\NativeBridge.java','linux\install.sh','linux\mukhomor.desktop','linux\mukhomor.svg','linux\uninstall.sh')){Copy-Reviewed $file $source}
# Never scan or copy parent directories or user profiles. Only the explicit public allowlist above is packaged.
foreach($file in @(
    'android\Generate-LaunchArtwork.py',
    'android\res\drawable\ic_mushroom_monochrome.xml','android\res\drawable\ic_stat_mushroom.xml',
    'android\res\drawable\startup_background.xml','android\res\drawable\startup_wordmark.xml',
    'android\res\mipmap-anydpi-v26\ic_launcher.xml','android\res\mipmap-anydpi-v33\ic_launcher.xml',
    'android\res\values\colors.xml','android\res\values\styles.xml','android\res\values-v31\styles.xml'
)){Copy-Reviewed $file $source}
foreach($package in @($runtime,$source)){
    $manifest=@()
    foreach($file in @(Get-ChildItem -LiteralPath $package -Recurse -File)){
        $relative=$file.FullName.Substring($package.Length+1)
        if($relative -match '(?i)(^|\\)(private|runtime|data|target|\.git)(\\|$)|\.conf($|\.)|(^|\\)(settings\.json|api-token\.txt|profiles\.json|owner\.sid|installation\.json)$'){throw ('Private path in package: '+$relative)}
        $contents=[Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($file.FullName))
        if($contents.Contains($env:USERPROFILE) -or $contents.Contains([IO.Path]::GetFullPath($PSScriptRoot))){throw ('Local build path in package: '+$relative)}
        $fileSha=(Get-FileHash -LiteralPath $file.FullName).Hash.ToLowerInvariant()
        if($package -ceq $runtime -and $relative -ceq 'mukhomor.exe' -and ($fileSha -cne $releasePePin.Report.sha256 -or $file.Length -ne $releasePePin.Report.bytes)){throw 'RELEASE_PE_MANIFEST_MISMATCH'}
        $manifest+=@(@{path=$relative;sha256=$fileSha})
    }
    Write-AtomicJson (Join-Path $package 'release-manifest.json') @{version=$Version;frontend='slint';files=$manifest;source_url='https://github.com/MetaCubeX/mihomo/tree/v1.19.32';personal_data_included=$false}
}
$dist=Join-Path $PSScriptRoot ('release-staging\'+$Version+'\windows\x64');[IO.Directory]::CreateDirectory($dist)|Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.IO.Compression
foreach($kind in @('windows','source')){
    $suffix=if($kind -eq 'windows'){'windows-x64'}else{'source'}
    $zip=Join-Path $stage ('Mukhomor-'+$Version+'-'+$suffix+'.zip')
    if(Test-Path -LiteralPath $zip){[IO.File]::Delete($zip)}
    $zipRoot=Join-Path $stage $kind
    $stream=[IO.File]::Open($zip,[IO.FileMode]::CreateNew)
    $archive=New-Object IO.Compression.ZipArchive($stream,[IO.Compression.ZipArchiveMode]::Create,$false)
    try {
        foreach($file in @(Get-ChildItem -LiteralPath $zipRoot -Recurse -File)) {
            $entry=$file.FullName.Substring($zipRoot.Length+1).Replace('\','/')
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive,$file.FullName,$entry,[IO.Compression.CompressionLevel]::Optimal)|Out-Null
        }
    } finally {$archive.Dispose();$stream.Dispose()}
    Write-AtomicText ($zip+'.sha256') ((Get-FileHash -LiteralPath $zip).Hash.ToLowerInvariant()+'  '+[IO.Path]::GetFileName($zip)+"`n")
    Write-Host ('Release: '+(Join-Path $dist ([IO.Path]::GetFileName($zip))))
}
# Both archives were verified and hashed before replacing this version.
foreach($file in @(Get-ChildItem -LiteralPath $stage -File)){Copy-Item -LiteralPath $file.FullName -Destination $dist -Force}
$unpacked=Join-Path $dist 'package'
$releasePePin.Dispose();$releasePePin=$null
if(Test-Path -LiteralPath $unpacked){
    Remove-OwnedBuildDirectory $unpacked $dist
}
[IO.Directory]::Move((Join-Path $stage 'windows'),$unpacked)
Write-AtomicText (Join-Path $dist 'latest-package.txt') ((Join-Path $unpacked 'Mukhomor')+"`n")
Remove-OwnedBuildDirectory $stage (Join-Path $PSScriptRoot 'target\release-build')
Write-Host 'Explicit public allowlist verified. No personal profiles, machine state or parent workspace files packaged.'
}finally{if($null -ne $releasePePin){$releasePePin.Dispose()}}
