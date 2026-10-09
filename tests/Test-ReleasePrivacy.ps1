param([string]$Archive)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
if(!$Archive){$match=[regex]::Match([IO.File]::ReadAllText((Join-Path $package 'native\Cargo.toml')),'(?m)^version = "(\d+\.\d+\.\d+)"$');if(!$match.Success){throw 'Missing Cargo version'};$Archive=Join-Path $package ('dist\Mukhomor-'+$match.Groups[1].Value+'-windows-x64.zip')}
$fixture=Join-Path $PSScriptRoot ('release-'+[guid]::NewGuid().ToString('N'))
$gitRoot=Join-Path $fixture 'ignore-test'
$unpack=Join-Path $fixture 'unpacked'
[IO.Directory]::CreateDirectory((Join-Path $gitRoot 'Mukhomor'))|Out-Null
$checks=0
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:checks++}
function Test-PackageManifest([string]$Root){
    $manifest=[IO.File]::ReadAllText((Join-Path $Root 'release-manifest.json'))|ConvertFrom-Json
    $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach($file in $manifest.files){
        $path=[IO.Path]::GetFullPath((Join-Path $Root $file.path))
        if(!$path.StartsWith(([IO.Path]::GetFullPath($Root)+'\'),[StringComparison]::OrdinalIgnoreCase) -or !$seen.Add($path)){return $false}
        if(!(Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256){return $false}
    }
    return ($seen.Count -gt 0)
}
try {
    $workspaceIgnore=Join-Path (Split-Path -Parent $package) '.gitignore'
    if(Test-Path -LiteralPath $workspaceIgnore){Copy-Item -LiteralPath $workspaceIgnore -Destination (Join-Path $gitRoot '.gitignore')}
    else{[IO.File]::WriteAllText((Join-Path $gitRoot '.gitignore'),"/*`n!/Mukhomor/`n")}
    Copy-Item -LiteralPath (Join-Path $package '.gitignore') -Destination (Join-Path $gitRoot 'Mukhomor\.gitignore')
    & git.exe -C $gitRoot init --quiet
    if($LASTEXITCODE -ne 0){throw 'Cannot initialize isolated Git test'}
    $private=@('personal.conf','notes.md','Mukhomor/private/awg.conf','Mukhomor/private/api-token.txt','Mukhomor/private/selected-profile.json','Mukhomor/native/src/secret.OVPN','Mukhomor/native/src/secret.yaml','Mukhomor/private/subscription.txt','Mukhomor/settings.json','Mukhomor/ui-preferences.json','Mukhomor/runtime/session.json','Mukhomor/native/target/release/mukhomor.exe','Mukhomor/native/src/secret.CONF','Mukhomor/lists/imported-domains.txt','Mukhomor/lists/original-ip-cidrs.txt','Mukhomor/owner.sid','Mukhomor/installation.json','Mukhomor/dist/public.zip','Mukhomor/tests/native-0123456789abcdef0123456789abcdef/private/profiles.json')
    foreach($relative in $private){
        $path=Join-Path $gitRoot $relative
        [IO.Directory]::CreateDirectory((Split-Path -Parent $path))|Out-Null
        [IO.File]::WriteAllText($path,'synthetic private marker')
        & git.exe -C $gitRoot check-ignore --quiet -- $relative
        Assert ($LASTEXITCODE -eq 0) ('ignored synthetic private path '+$relative)
    }
    foreach($relative in @('Mukhomor/native/src/main.rs','Mukhomor/native/src/font.rs','Mukhomor/native/src/i18n.rs','Mukhomor/native/src/texture.rs','Mukhomor/native/Cargo.lock','Mukhomor/native/windows.manifest','Mukhomor/NativeBridge.ps1','Mukhomor/ProfileImport.psm1','Mukhomor/tests/Test-ProfileImport.ps1','Mukhomor/tests/Test-ProtocolTransport.ps1','Mukhomor/tests/Test-LargeRules.ps1','Mukhomor/tests/Measure-Routing.ps1','Mukhomor/tests/routing-benchmark.cjs','Mukhomor/assets/settings.default.json','Mukhomor/assets/fonts/Tiny5-Regular.ttf','Mukhomor/assets/fonts/README.md','Mukhomor/Tiny5-LICENSE.txt','Mukhomor/lists/seed-russia.list','Mukhomor/README.md')){
        $path=Join-Path $gitRoot $relative;[IO.Directory]::CreateDirectory((Split-Path -Parent $path))|Out-Null;[IO.File]::WriteAllText($path,'synthetic public marker')
        & git.exe -C $gitRoot check-ignore --quiet -- $relative
        Assert ($LASTEXITCODE -eq 1) ('public source remains trackable '+$relative)
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip=[IO.Compression.ZipFile]::OpenRead($Archive)
    try{
        foreach($entry in $zip.Entries){
            $relative=$entry.FullName.Replace('\','/')
            Assert (!$relative.Contains('..') -and !$relative.StartsWith('/') -and $relative.StartsWith('Mukhomor/')) 'safe ZIP path'
            Assert ($relative -notmatch '(?i)(^|/)(private|runtime|target|data|release-staging)/|\.conf($|\.)|(^|/)(settings\.json|profiles\.json|api-token\.txt|owner\.sid|installation\.json)$') 'no private state in ZIP'
        }
    }finally{$zip.Dispose()}
    [IO.Compression.ZipFile]::ExtractToDirectory($Archive,$unpack)
    $release=Join-Path $unpack 'Mukhomor'
    Assert (Test-PackageManifest $release) 'all packaged file hashes and manifest paths verified'
    if(Test-Path -LiteralPath (Join-Path $release 'native\Cargo.toml')){
        foreach($required in @('native\Cargo.lock','native\src\main.rs','Build-Release.ps1','Fetch-Dependencies.ps1','Source\mihomo-v1.19.32-source.zip','docs\credits.md','.github\workflows\release.yml')){Assert (Test-Path -LiteralPath (Join-Path $release $required) -PathType Leaf) ('source package contains '+$required)}
        & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $release 'tests\Test-PublicSource.ps1')
        Assert ($LASTEXITCODE -eq 0) 'reviewed source manifest passes the publication scan without Git metadata'
        [IO.File]::AppendAllText((Join-Path $release 'assets\settings.default.json'),'corruption')
        Assert (!(Test-PackageManifest $release)) 'source file corruption rejected'
        Write-Host ('PASS: '+$checks+' Git ignore, source privacy and integrity checks')
        $global:LASTEXITCODE=0
        return
    }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $release 'Install.ps1') -VerifyOnly
    Assert ($LASTEXITCODE -eq 0) 'installer verifies entire extracted release without administrator privileges'
    $defaults=Join-Path $release 'assets\settings.default.json'
    [IO.File]::AppendAllText($defaults,'corruption')
    $errorLog=Join-Path $fixture 'verification-error.txt'
    $previousAction=$ErrorActionPreference
    try{$ErrorActionPreference='Continue'; & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $release 'Install.ps1') -VerifyOnly *> $errorLog}
    finally{$ErrorActionPreference=$previousAction}
    Assert ($LASTEXITCODE -ne 0) 'modified file rejected by installer'
    Write-Host ('PASS: '+$checks+' Git ignore, ZIP privacy and installer integrity checks')
    # The tampered-package rejection above deliberately returns a nonzero
    # native exit code. A successful test must not leak it to the CI shell.
    $global:LASTEXITCODE=0
}finally{
    $resolved=[IO.Path]::GetFullPath($fixture)
    if($resolved.StartsWith(([IO.Path]::GetFullPath($PSScriptRoot)+'\release-'),[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^release-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
