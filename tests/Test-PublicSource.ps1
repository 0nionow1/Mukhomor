Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
# Inspect only files Git would publish. Do not read ignored profiles or parent folders.
$packaged=!(Test-Path -LiteralPath (Join-Path $package '.git'))
if($packaged){
    $manifestPath=Join-Path $package 'release-manifest.json'
    if(!(Test-Path -LiteralPath $manifestPath)){throw 'Expected a Git checkout or reviewed source package manifest'}
    $manifest=[IO.File]::ReadAllText($manifestPath)|ConvertFrom-Json
    $files=@($manifest.files | ForEach-Object {$_.path.Replace('\','/')})
}else{
    $files=@(& git.exe -C $package -c core.quotepath=false ls-files --cached --others --exclude-standard | Sort-Object -Unique)
    if($LASTEXITCODE -ne 0){throw 'Cannot enumerate public Git files'}
    $deleted=@(& git.exe -C $package -c core.quotepath=false ls-files --deleted)
    if($LASTEXITCODE -ne 0){throw 'Cannot enumerate deleted Git files'}
    $files=@($files | Where-Object {$_ -notin $deleted})
}
if(!$files.Count){throw 'No public source files'}
$issues=New-Object 'Collections.Generic.List[string]'
$checked=0
# A privacy allowlist must also retain every required source dependency. These
# files were previously present locally but excluded from a clean Git checkout.
$required=@('TrafficGuard.psm1','native/src/cleanup_request.rs','native/src/setup_recovery.rs','native/src/traffic_guard.rs','native/src/traffic_guard_cli.rs','native/src/traffic_guard_tun.rs','tests/Set-OwnedTestRoot.ps1','tests/Test-PrivateAcl.ps1','tests/Test-Uninstall.ps1','tests/Test-ControllerRecovery.ps1','tests/Test-FullRouting.ps1','tests/Test-FullTrafficGuardLifecycle.ps1','tests/Test-NativeGuardRecovery.ps1')
$required+=@('Slint-LICENSE.txt','native/src/desktop.rs','native/src/slint_host.rs','ui/Cargo.toml','ui/Cargo.lock','ui/build.rs','ui/src/lib.rs','ui/src/state.rs','ui/src/preview.rs','ui/src/visuals.rs','native/visual_build.rs','native/src/hero_motion.rs','ui/views/app.slint','ui/README.md')
$required+=@('tests/Test-SlintStartup.ps1')
$required+=@('native/src/app_update.rs','android/src/org/mukhomor/app/UpdateProvider.java')
$required+=@('RuleNormalizer.cs','tests/Test-Performance.ps1')
$required+=@('Build-All.ps1')
$required+=@('tests/Test-PortableReleasePrivacy.py')
$required+=@('Build-Android.ps1','Build-Linux.sh','Prepare-PortableCore.py','Package-PortableSource.py','go.mod','go.sum','portable_assets.go','docs/platforms.md','tests/linux-network.py','native/src/lib.rs','native/src/portable.rs','native/src/android.rs','core/controller.go','core/controller_test.go','core/import.go','core/node_fields.go','core/openvpn.go','core/rules.go','core/storage.go','core/update.go','cmd/mukhomor-android/bridge.c','cmd/mukhomor-android/main.go','cmd/mukhomor-controller/main.go','android/AndroidManifest.xml','android/res/drawable/ic_mushroom.xml','android/src/org/mukhomor/app/MainActivity.java','android/src/org/mukhomor/app/MukhomorVpnService.java','android/src/org/mukhomor/app/NativeBridge.java','linux/install.sh','linux/mukhomor.desktop','linux/mukhomor.svg','linux/uninstall.sh')
$required+=@('android/Generate-LaunchArtwork.py','android/res/drawable/ic_mushroom_monochrome.xml','android/res/drawable/ic_stat_mushroom.xml','android/res/drawable/startup_background.xml','android/res/mipmap-anydpi-v26/ic_launcher.xml','android/res/mipmap-anydpi-v33/ic_launcher.xml','android/res/values/colors.xml','android/res/values/styles.xml','android/res/values-v31/styles.xml')
$required+=@('android/res/drawable/startup_wordmark.xml')
foreach($relative in $required){
    if($relative -notin $files){$issues.Add($relative+': required source excluded from publication')}
}
foreach($relative in $files){
    if($packaged -and $relative -in @('Source/mycelium-build.json','Source/mihomo-mycelium-source.zip')){
        $build=[IO.File]::ReadAllText((Join-Path $package 'Source\mycelium-build.json'))|ConvertFrom-Json
        if($build.schema -ne 1 -or $build.protocol_version -ne 1 -or $build.base_version -ne '1.19.32' -or $build.source_sha256 -cnotmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath (Join-Path $package 'Source\mihomo-mycelium-source.zip')).Hash.ToLowerInvariant() -cne $build.source_sha256){$issues.Add($relative+': Mycelium source provenance mismatch')}
        continue
    }
    if($packaged -and $relative -eq 'Source/mihomo-v1.19.32-source.zip'){
        if((Get-FileHash -LiteralPath (Join-Path $package $relative) -Algorithm SHA256).Hash.ToLowerInvariant() -ne '135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1'){$issues.Add($relative+': source checksum mismatch')}
        continue
    }
    if($relative -match '(?i)(^|/)(private|runtime|dist|release-staging|target|graphify-out|bin|Source)/|\.(conf|ovpn|pem|key|pfx|p12|uri|sub|log|db|zip)($|\.)|(^|/)(\.env[^/]*|settings\.json|profiles\.json|api-token\.txt|owner\.sid|installation\.json)$'){$issues.Add($relative+': private/generated path');continue}
    if($relative -match '(?i)\.(yaml|yml)$' -and $relative -notin @('.github/workflows/ci.yml','.github/workflows/release.yml','.github/workflows/pages.yml')){$issues.Add($relative+': unreviewed YAML');continue}
    $path=[IO.Path]::GetFullPath((Join-Path $package $relative))
    if(!$path.StartsWith($package+'\',[StringComparison]::OrdinalIgnoreCase) -or !(Test-Path -LiteralPath $path -PathType Leaf)){$issues.Add($relative+': invalid path');continue}
    if($relative -match '(?i)\.(png|ttf)$'){continue}
    $text=[IO.File]::ReadAllText($path)
    # Patterns identify credentials, not their values, so failures never print secrets.
    $checks=@{
        'GitHub token'='\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})\b'
        'private key block'='-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=\r\n]{64,}'
        'AWS access key'='\b(?:AKIA|ASIA)[A-Z0-9]{16}\b'
        'local build path'='(?i)(?:D:[/\\]VPS[/\\]|C:[/\\]Users[/\\](?!Public\b|Default\b|<)[^\s"''<>/\\]+)'
        'WireGuard credential'='(?im)^\s*(?:PrivateKey|PresharedKey)\s*=\s*[A-Za-z0-9+/]{43}='
    }
    foreach($entry in $checks.GetEnumerator()){
        if($text -match $entry.Value){
            # Only the canonical 32-byte repeated-A test key is public.
            if($entry.Key -eq 'WireGuard credential' -and $relative.StartsWith('tests/')){
                $matchesFound=[regex]::Matches($text,$entry.Value)
                $synthetic=[Convert]::ToBase64String([byte[]](1..32|ForEach-Object {65}))
                if(@($matchesFound | Where-Object {!$_.Value.TrimEnd().EndsWith($synthetic,[StringComparison]::Ordinal)}).Count -eq 0){continue}
            }
            $issues.Add($relative+': '+$entry.Key)
        }
    }
    $checked++
}
if($issues.Count){throw ('Publication scan failed (paths and categories only): '+($issues -join '; '))}
Write-Host ('PASS: '+$files.Count+' public paths and '+$checked+' text files checked; no ignored data read')
