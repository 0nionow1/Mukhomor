param(
    [string]$Sdk = $env:ANDROID_HOME,
    [string]$Ndk = $env:ANDROID_NDK_HOME,
    [string]$Java = $env:JAVA_HOME,
    [string]$Go = 'go',
    [string]$Python = 'python',
    [switch]$Mycelia,
    [ValidateSet('arm64-v8a','x86_64')][string[]]$Abis = @('arm64-v8a','x86_64'),
    [string]$Keystore,
    [string]$KeyAlias = 'mukhomor',
    [switch]$DebugBuild
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if(!$Sdk -or !$Ndk -or !$Java){throw 'Set ANDROID_HOME, ANDROID_NDK_HOME and JAVA_HOME, or pass -Sdk -Ndk -Java.'}
$root=$PSScriptRoot
$stage=Join-Path $root 'target\android-package'
foreach($dir in @($stage,(Join-Path $stage 'classes'),(Join-Path $stage 'dex'))){[IO.Directory]::CreateDirectory($dir)|Out-Null}
$buildTools=Join-Path $Sdk 'build-tools\35.0.0'
$platform=Join-Path $Sdk 'platforms\android-35\android.jar'
$llvm=Join-Path $Ndk 'toolchains\llvm\prebuilt\windows-x86_64\bin'
function Run([string]$Program,[string[]]$Arguments){
    # Windows PowerShell wraps redirected native stderr as an ErrorRecord even
    # for Cargo progress and javac warnings. The process exit code is decisive.
    $previous=$ErrorActionPreference
    try{$ErrorActionPreference='Continue'; & $Program @Arguments; $code=$LASTEXITCODE}
    finally{$ErrorActionPreference=$previous}
    if($code -ne 0){throw "$Program failed ($code)"}
}
$versionMatch=[regex]::Match([IO.File]::ReadAllText((Join-Path $root 'native\Cargo.toml')),'(?m)^version = "([0-9.]+)"\r?$')
$version=$versionMatch.Groups[1].Value
if(!$version -or [IO.File]::ReadAllText((Join-Path $root 'android\AndroidManifest.xml')) -notmatch ('versionName="'+[regex]::Escape($version)+'"')){throw 'Android and Rust versions differ'}
$architecture=if(@($Abis | Select-Object -Unique).Count -eq 2){'universal'}else{$Abis[0]}
$variant=if($DebugBuild){'debug'}else{'release'}
$dist=Join-Path $root ('release-staging\'+$version+'\android\'+$architecture+'\'+$variant)
[IO.Directory]::CreateDirectory($dist)|Out-Null
$env:ANDROID_NDK_ROOT=$Ndk
$env:ANDROID_NDK=$Ndk
$env:JAVA_HOME=$Java
$env:CGO_ENABLED='1'
$env:GOOS='android'
$env:CARGO_NET_OFFLINE='false'
$profile=if($DebugBuild){'debug'}else{'release'}
$cargoTarget=if($env:CARGO_TARGET_DIR){[IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)}else{Join-Path $root 'native\target'}
$previousRustFlags=$env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS=@('-C','link-arg=-Wl,-z,max-page-size=16384',('--remap-path-prefix='+[IO.Path]::GetFullPath($root)+'=/project'),('--remap-path-prefix='+$env:USERPROFILE+'=/build-user')) -join [char]31
Push-Location $root
try {
    $prepare=@('Prepare-PortableCore.py','--go',$Go)
    if($Mycelia){$prepare+='--mycelia'}
    Run $Python $prepare
    Run $Python @('pt/prepare.py','--go',$Go)
    Run $Python @('tor/prepare.py')
    $modfile=Join-Path $root 'target\portable-core\windows\portable.mod'
    foreach($abi in $Abis){
        & (Join-Path $root 'i2p\windows-build.ps1') -Platform ('android-'+$abi)
        $target=if($abi -eq 'arm64-v8a'){'aarch64-linux-android'}else{'x86_64-linux-android'}
        $env:GOARCH=if($abi -eq 'arm64-v8a'){'arm64'}else{'amd64'}
        $clangTarget=$target+'26'
        $env:CC=(Join-Path $llvm 'clang.exe')+' --target='+$clangTarget
        $env:CGO_LDFLAGS='-Wl,-z,max-page-size=16384'
        $libdir=Join-Path $stage ('lib\'+$abi);[IO.Directory]::CreateDirectory($libdir)|Out-Null
        Copy-Item -LiteralPath (Join-Path $root ('target\i2p-sources\android-'+$abi+'\native\mukhomor-i2pd')) -Destination (Join-Path $libdir 'libmukhomor_i2pd.so') -Force
        Run (Join-Path $llvm 'llvm-strip.exe') @('--strip-debug',(Join-Path $libdir 'libmukhomor_i2pd.so'))
        Push-Location (Join-Path $root 'pt')
        try {Run $Go @('build','-modfile=../target/pt-sources/windows/pt.mod','-mod=readonly','-trimpath','-buildmode=pie','-ldflags=-s -w -extldflags=-Wl,-z,max-page-size=16384','-o',(Join-Path $libdir 'libmukhomor_pt.so'),'.')}finally{Pop-Location}
        Run $Go @('build',('-modfile='+$modfile),'-mod=readonly','-tags=with_gvisor','-trimpath','-buildmode=c-shared','-ldflags=-s -w','-o',(Join-Path $libdir 'libmukhomor_core.so'),'./cmd/mukhomor-android')
        $linker=Join-Path $llvm ($clangTarget+'-clang.cmd')
        [Environment]::SetEnvironmentVariable(('CARGO_TARGET_'+$target.Replace('-','_').ToUpperInvariant()+'_LINKER'),$linker,'Process')
        [Environment]::SetEnvironmentVariable(('CC_'+$target.Replace('-','_')),$linker,'Process')
        [Environment]::SetEnvironmentVariable(('AR_'+$target.Replace('-','_')),(Join-Path $llvm 'llvm-ar.exe'),'Process')
        $env:RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384'
        $cargoArgs=@('build','--manifest-path','native/Cargo.toml','--locked','--lib','--target',$target)
        if(!$DebugBuild){$cargoArgs+='--release'}
        Run 'cargo' $cargoArgs
        $torArgs=@('build','--manifest-path','tor/Cargo.toml','--locked','--release','--target',$target)
        Run 'cargo' $torArgs
        $torTarget=if($env:CARGO_TARGET_DIR){$cargoTarget}else{Join-Path $root 'tor\target'}
        Copy-Item -LiteralPath (Join-Path $torTarget ($target+'\release\mukhomor-tor')) -Destination (Join-Path $libdir 'libmukhomor_tor.so') -Force
        Run (Join-Path $llvm 'llvm-strip.exe') @('--strip-debug',(Join-Path $libdir 'libmukhomor_tor.so'))
        Copy-Item -LiteralPath (Join-Path $cargoTarget ($target+'\'+$profile+'\libmukhomor_app.so')) -Destination $libdir -Force
        # Keep debugging symbols in Cargo's output, not in the installed APK.
        Run (Join-Path $llvm 'llvm-strip.exe') @('--strip-debug',(Join-Path $libdir 'libmukhomor_app.so'))
    }
    Run $Python @('i2p/sources.py')
    $launchResources=Join-Path $stage 'launch-res'
    Run $Python @('android/Generate-LaunchArtwork.py','--output',$launchResources)
    Run (Join-Path $buildTools 'aapt2.exe') @('compile','--dir',(Join-Path $root 'android\res'),'-o',(Join-Path $stage 'resources.zip'))
    Run (Join-Path $buildTools 'aapt2.exe') @('compile','--dir',$launchResources,'-o',(Join-Path $stage 'launch-resources.zip'))
    Run (Join-Path $buildTools 'aapt2.exe') @('link','-o',(Join-Path $stage 'base.apk'),'--manifest',(Join-Path $root 'android\AndroidManifest.xml'),'-I',$platform,'--java',(Join-Path $stage 'generated'),(Join-Path $stage 'resources.zip'),(Join-Path $stage 'launch-resources.zip'))
    $sources=@(Get-ChildItem -LiteralPath (Join-Path $root 'android\src'),(Join-Path $stage 'generated') -Recurse -Filter '*.java' -File | ForEach-Object {'"'+$_.FullName.Replace('\','/')+'"'})
    [IO.File]::WriteAllLines((Join-Path $stage 'sources.txt'),$sources,(New-Object Text.UTF8Encoding($false)))
    Run (Join-Path $Java 'bin\javac.exe') @('-encoding','UTF-8','-source','8','-target','8','-classpath',$platform,'-d',(Join-Path $stage 'classes'),('@'+(Join-Path $stage 'sources.txt')))
    Run (Join-Path $Java 'bin\jar.exe') @('cf',(Join-Path $stage 'classes.jar'),'-C',(Join-Path $stage 'classes'),'.')
    Run (Join-Path $buildTools 'd8.bat') @('--min-api','26','--lib',$platform,'--output',(Join-Path $stage 'dex'),(Join-Path $stage 'classes.jar'))
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip=[IO.Compression.ZipFile]::Open((Join-Path $stage 'base.apk'),[IO.Compression.ZipArchiveMode]::Update)
    try{
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $stage 'dex\classes.dex'),'classes.dex')|Out-Null
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $root 'tor\THIRD_PARTY_NOTICES.txt'),'assets/Tor-THIRD_PARTY_NOTICES.txt')|Out-Null
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $root 'pt\THIRD_PARTY_NOTICES.txt'),'assets/Bridges-THIRD_PARTY_NOTICES.txt')|Out-Null
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $root 'Tor-LICENSE.txt'),'assets/Tor-LICENSE.txt')|Out-Null
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $root 'i2p\THIRD_PARTY_NOTICES.txt'),'assets/I2P-THIRD_PARTY_NOTICES.txt')|Out-Null
        foreach($abi in $Abis){foreach($name in @('libmukhomor_core.so','libmukhomor_app.so','libmukhomor_tor.so','libmukhomor_pt.so','libmukhomor_i2pd.so')){[IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $stage ('lib\'+$abi+'\'+$name)),('lib/'+$abi+'/'+$name))|Out-Null}}
    }finally{$zip.Dispose()}
    Run (Join-Path $buildTools 'zipalign.exe') @('-f','-P','16','4',(Join-Path $stage 'base.apk'),(Join-Path $stage 'aligned.apk'))
    if(!$Keystore){
        if(!$DebugBuild){throw 'Release signing requires -Keystore and MUKHOMOR_KEYSTORE_PASSWORD. Use -DebugBuild for local device tests.'}
        $Keystore=Join-Path $stage 'debug.keystore';$KeyAlias='androiddebugkey';$env:MUKHOMOR_KEYSTORE_PASSWORD='android'
        if(!(Test-Path -LiteralPath $Keystore)){Run (Join-Path $Java 'bin\keytool.exe') @('-genkeypair','-keystore',$Keystore,'-storepass','android','-keypass','android','-alias',$KeyAlias,'-keyalg','RSA','-keysize','2048','-validity','10000','-dname','CN=Android Debug,O=Android,C=US')}
    }
    if(!$env:MUKHOMOR_KEYSTORE_PASSWORD){throw 'Set MUKHOMOR_KEYSTORE_PASSWORD for signing'}
    $suffix=if($DebugBuild){'-debug'}else{''}
    $apk=Join-Path $stage ('Mukhomor-'+$version+'-android'+$suffix+'.apk')
    Run (Join-Path $buildTools 'apksigner.bat') @('sign','--ks',$Keystore,'--ks-key-alias',$KeyAlias,'--ks-pass','env:MUKHOMOR_KEYSTORE_PASSWORD','--out',$apk,(Join-Path $stage 'aligned.apk'))
    Run (Join-Path $buildTools 'apksigner.bat') @('verify','--verbose',$apk)
    Run $Python @('tests/Test-PortableReleasePrivacy.py','--archive',$apk)
    [IO.File]::WriteAllText(($apk+'.sha256'),((Get-FileHash -LiteralPath $apk -Algorithm SHA256).Hash.ToLowerInvariant()+'  '+[IO.Path]::GetFileName($apk)+"`n"))
    $source=Join-Path $stage ('Mukhomor-'+$version+'-android-source.zip')
    Run $Python @('Package-PortableSource.py','--platform','windows','--output',$source,'--go',$Go)
    # Only verified outputs replace the previous build of this exact variant.
    foreach($file in @($apk,($apk+'.sha256'),$source,($source+'.sha256'))){Copy-Item -LiteralPath $file -Destination $dist -Force}
    if(Test-Path -LiteralPath ($apk+'.idsig')){Copy-Item -LiteralPath ($apk+'.idsig') -Destination $dist -Force}
    Write-Host ('Android package: '+(Join-Path $dist ([IO.Path]::GetFileName($apk))))
}finally{Pop-Location;$env:CARGO_ENCODED_RUSTFLAGS=$previousRustFlags}
