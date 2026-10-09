param([string]$Version='',[switch]$Offline)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -Force -DisableNameChecking
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
    try {if($Offline){& cargo.exe build --release --locked --offline}else{& cargo.exe build --release --locked};if($LASTEXITCODE -ne 0){throw 'Native build failed'}}finally{Pop-Location}
}finally{$env:CARGO_ENCODED_RUSTFLAGS=$oldFlags}
$stage=Join-Path $PSScriptRoot ('release-staging\'+$Version+'-'+[guid]::NewGuid().ToString('N'))
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
$common=@('NativeBridge.ps1','SplitVpn.psm1','SplitWindows.psm1','ProfileImport.psm1','Install.ps1','Uninstall.ps1','README.md','Mihomo-LICENSE.txt','Wintun-LICENSE.txt','Rules-LICENSE.txt','ThirdParty-LICENSE.txt','Tiny5-LICENSE.txt','LICENSE','assets\settings.default.json','assets\README.ru.md','assets\preview.png','lists\sources.json','lists\seed-russia.list','lists\seed-steam.list','lists\seed-ozon.list')
foreach($file in $common){Copy-Reviewed $file $runtime;Copy-Reviewed $file $source}
$publication=@('README.en.md','CHANGELOG.md','CONTRIBUTING.md','SECURITY.md','assets\github-banner.svg','docs\README.md','docs\getting-started.md','docs\gaming-and-routing.md','docs\protocols.md','docs\vps-and-vpn.md','docs\troubleshooting.md','docs\development.md','docs\architecture.md','docs\releasing.md','docs\credits.md')
foreach($file in $publication){Copy-Reviewed $file $runtime;Copy-Reviewed $file $source}
foreach($file in @('.gitattributes','.github\PULL_REQUEST_TEMPLATE.md','.github\ISSUE_TEMPLATE\bug_report.md','.github\ISSUE_TEMPLATE\feature_request.md','.github\workflows\ci.yml','.github\workflows\release.yml','tests\Test-PublicSource.ps1','tests\Run-Checks.ps1')){Copy-Reviewed $file $source}
foreach($file in @('.gitignore','Build-Release.ps1','Fetch-Dependencies.ps1','native\Cargo.toml','native\Cargo.lock','native\build.rs','native\windows.manifest','native\README.md','assets\theme.json','assets\fonts\Tiny5-Regular.ttf','assets\fonts\README.md')){Copy-Reviewed $file $source}
foreach($name in @('main.rs','backend.rs','platform.rs','service.rs','setup.rs','ui.rs','font.rs','i18n.rs','texture.rs')){Copy-Reviewed ('native\src\'+$name) $source}
foreach($name in @('Test-SplitVpn.ps1','Test-ProfileImport.ps1','Test-ProtocolTransport.ps1','Test-CoreRouting.ps1','Test-LargeRules.ps1','Measure-Routing.ps1','routing-benchmark.cjs','Test-UdpRouting.ps1','Test-NativeApp.ps1','Test-Disconnect.ps1','Test-Isolation.ps1','Test-ReleasePrivacy.ps1','Test-PixelFont.ps1','Test-Lifecycle.ps1','Test-Upgrade.ps1','Test-Transport.ps1','Test-StartupRecovery.ps1','Test-StartupCancellation.ps1','Test-DnsRecovery.ps1','udp-client.cjs','local-origin.cjs','proxy-client.cjs')){if(Test-Path -LiteralPath (Join-Path $PSScriptRoot ('tests\'+$name))){Copy-Reviewed ('tests\'+$name) $source}}
[IO.File]::Copy((Join-Path $native 'target\release\mukhomor.exe'),(Join-Path $runtime 'mukhomor.exe'),$false)
foreach($name in @('mihomo-windows-amd64-compatible.exe','wintun.dll')){Copy-Reviewed ('bin\'+$name) $runtime}
$pins=@{'bin\mihomo-windows-amd64-compatible.exe'='04f8d7fc2b314771e1ecbf15951d59f0e1cb7914503c88cfb6972ff6a824e314';'bin\wintun.dll'='e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce'}
foreach($entry in $pins.GetEnumerator()){if((Get-FileHash -LiteralPath (Join-Path $runtime $entry.Key)).Hash.ToLowerInvariant() -ne $entry.Value){throw ('Dependency checksum mismatch: '+$entry.Key)}}
$coreSource='Source\mihomo-v1.19.32-source.zip'
if(!(Test-Path -LiteralPath (Join-Path $PSScriptRoot $coreSource))){throw 'Run Fetch-Dependencies.ps1 to obtain the corresponding Mihomo source archive'}
if((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $coreSource)).Hash.ToLowerInvariant() -ne '135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1'){throw 'Mihomo source checksum mismatch'}
Copy-Reviewed $coreSource $runtime;Copy-Reviewed $coreSource $source
# Never scan or copy parent directories or user profiles. Only the explicit public allowlist above is packaged.
foreach($package in @($runtime,$source)){
    $manifest=@()
    foreach($file in @(Get-ChildItem -LiteralPath $package -Recurse -File)){
        $relative=$file.FullName.Substring($package.Length+1)
        if($relative -match '(?i)(^|\\)(private|runtime|data|target|\.git)(\\|$)|\.conf($|\.)|(^|\\)(settings\.json|api-token\.txt|profiles\.json|owner\.sid|installation\.json)$'){throw ('Private path in package: '+$relative)}
        $contents=[Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($file.FullName))
        if($contents.Contains($env:USERPROFILE) -or $contents.Contains([IO.Path]::GetFullPath($PSScriptRoot))){throw ('Local build path in package: '+$relative)}
        $manifest+=@(@{path=$relative;sha256=(Get-FileHash -LiteralPath $file.FullName).Hash.ToLowerInvariant()})
    }
    Write-AtomicJson (Join-Path $package 'release-manifest.json') @{version=$Version;files=$manifest;source_url='https://github.com/MetaCubeX/mihomo/tree/v1.19.32';personal_data_included=$false}
}
$dist=Join-Path $PSScriptRoot 'dist';[IO.Directory]::CreateDirectory($dist)|Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.IO.Compression
foreach($kind in @('windows','source')){
    $suffix=if($kind -eq 'windows'){'windows-x64'}else{'source'}
    $zip=Join-Path $dist ('Mukhomor-'+$Version+'-'+$suffix+'.zip')
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
    Write-Host ('Release: '+$zip)
}
Write-AtomicText (Join-Path $dist 'latest-package.txt') ($runtime+"`n")
Write-Host 'Explicit public allowlist verified. No personal profiles, machine state or parent workspace files packaged.'
