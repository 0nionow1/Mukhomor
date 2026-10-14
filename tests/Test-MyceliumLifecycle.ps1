param([string]$CorePath)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitWindows.psm1') -Force -DisableNameChecking
if(!$CorePath){
    $CorePath=Join-Path $package 'bin\mihomo-mycelium.exe'
    # Ownership/manifest regression does not require the native adapter. This
    # keeps vanilla checkouts and CI covered after Fetch-Dependencies.ps1.
    if(!(Test-Path -LiteralPath $CorePath -PathType Leaf)){$CorePath=Join-Path $package 'bin\mihomo-windows-amd64-compatible.exe'}
}
$source=[IO.Path]::GetFullPath($CorePath)
if(!(Test-Path -LiteralPath $source -PathType Leaf)){throw 'FAIL: lifecycle fixture requires a built core executable'}
$fixture=Join-Path $PSScriptRoot ('mycelium-'+[guid]::NewGuid().ToString('N'))
$root=Join-Path $fixture 'state'
$bin=Join-Path $fixture 'bin'
$stock=Join-Path $bin 'mihomo-windows-amd64-compatible.exe'
$mycelium=Join-Path $bin 'mihomo-mycelium.exe'
$manifestPath=Join-Path $bin 'mycelium-build.json'
$sessionPath=Join-Path $root 'runtime\session.json'
$savedAssets=$env:MUKHOMOR_ASSETS;$savedCore=$env:MUKHOMOR_CORE_PATH
$spawned=New-Object Collections.Generic.List[object]
$passed=0;$sequence=0
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
function Reject([scriptblock]$Action,[string]$Message){$failed=$false;try{& $Action|Out-Null}catch{$failed=$true};Assert $failed $Message}
function Write-Session($Child,[string]$Program){
    Write-AtomicJson $sessionPath @{pid=$Child.Id;started=$Child.StartTime.ToUniversalTime().ToString('o');core_path=$Program;mode='proxy';phase='running'}
}
function Write-Manifest($Value=$script:validManifest){Write-AtomicJson $manifestPath $Value}
function New-LoopbackPort {
    $listener=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
    try{$listener.Start();return ([Net.IPEndPoint]$listener.LocalEndpoint).Port}finally{$listener.Stop()}
}
function Start-FixtureCore([string]$Program){
    $script:sequence++
    $port=New-LoopbackPort
    $token='mycelium-lifecycle-'+[guid]::NewGuid().ToString('N')
    Write-AtomicText (Join-Path $root 'private\api-token.txt') $token
    Write-AtomicJson (Join-Path $root 'settings.json') @{schema=1;ports=@{controller=$port}}
    $config=Join-Path $root ('core-'+$script:sequence+'.json')
    Write-AtomicJson $config @{
        'mixed-port'=0;'port'=0;'socks-port'=0;'redir-port'=0;'tproxy-port'=0
        'external-controller'=('127.0.0.1:'+$port);secret=$token;'allow-lan'=$false;'bind-address'='127.0.0.1'
        'log-level'='silent';'geo-auto-update'=$false;mode='rule';dns=@{enable=$false};tun=@{enable=$false}
        proxies=@();'proxy-groups'=@();'rule-providers'=@{};rules=@('MATCH,REJECT');sniffer=@{enable=$false}
    }
    $process=Start-Process -FilePath $Program -ArgumentList ('-d "'+$root+'" -f "'+$config+'"') -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root ('core-'+$script:sequence+'-out.log')) -RedirectStandardError (Join-Path $root ('core-'+$script:sequence+'-error.log'))
    # Hold the actual child process handle; cleanup cannot resolve a reused PID.
    $null=$process.Handle;$script:spawned.Add($process)
    $ready=$false;$timer=[Diagnostics.Stopwatch]::StartNew()
    while($timer.ElapsedMilliseconds -lt 12000){
        if($process.HasExited){throw 'FAIL: isolated core exited before loopback readiness'}
        try{Invoke-CoreApi $root '/version' -TimeoutSeconds 1|Out-Null;$ready=$true;break}catch{}
        Start-Sleep -Milliseconds 40
    }
    Assert $ready 'the fixture core starts with a loopback controller and no TUN, DNS or traffic listener'
    Write-Session $process $Program
    return $process
}
function Assert-Owned($Child,[string]$Message){$owned=Get-OwnedCore $root;Assert ($null -ne $owned -and $owned.Id -eq $Child.Id) $Message}
function Assert-Stopped($Child,[string]$Message){
    Stop-OwnedCore $root
    Assert ($Child.WaitForExit(1500) -and !(Test-Path -LiteralPath $sessionPath)) $Message
}

try{
    foreach($directory in @($bin,(Join-Path $root 'private'),(Join-Path $root 'runtime'))){[IO.Directory]::CreateDirectory($directory)|Out-Null}
    Set-OwnedTestRoot $root $fixture
    Copy-Item -LiteralPath $source -Destination $mycelium
    $stockSource=Join-Path $package 'bin\mihomo-windows-amd64-compatible.exe'
    if(!(Test-Path -LiteralPath $stockSource -PathType Leaf)){$stockSource=$source}
    Copy-Item -LiteralPath $stockSource -Destination $stock
    $env:MUKHOMOR_ASSETS=$fixture;$env:MUKHOMOR_CORE_PATH=$null
    $script:validManifest=@{schema=1;protocol_version=1;base_version='1.19.32';core_sha256=(Get-FileHash -LiteralPath $mycelium -Algorithm SHA256).Hash.ToLowerInvariant()}

    Assert ((Get-CorePath $fixture) -eq $stock) 'without a Mycelium manifest the fixture selects its stock bundled executable'
    $env:MUKHOMOR_CORE_PATH=$mycelium
    Reject {Get-CorePath $fixture} 'an explicitly selected Mycelium core requires a build manifest'
    $env:MUKHOMOR_CORE_PATH=$null
    Write-Manifest
    Assert ((Get-CorePath $fixture) -eq $mycelium) 'a valid manifest selects the verified native Mycelium core'
    foreach($mutation in @('schema','protocol','missing-hash','malformed-hash','wrong-hash')){
        $bad=$script:validManifest.Clone()
        switch($mutation){
            'schema' {$bad.schema=2}
            'protocol' {$bad.protocol_version=2}
            'missing-hash' {$bad.Remove('core_sha256')}
            'malformed-hash' {$bad.core_sha256='invalid'}
            'wrong-hash' {$bad.core_sha256='0'*64}
        }
        Write-Manifest $bad
        Reject {Get-CorePath $fixture} 'unsupported manifest versions and missing or corrupt SHA256 values fail before launch'
    }
    Write-AtomicText $manifestPath '{not valid JSON'
    Reject {Get-CorePath $fixture} 'a malformed manifest fails before launch'
    Write-AtomicText $manifestPath ('x'*16385)
    Reject {Get-CorePath $fixture} 'an oversized manifest fails before launch'
    Write-Manifest
    $stream=[IO.File]::Open($mycelium,'Open','ReadWrite','None')
    try{$first=$stream.ReadByte();$stream.Position=0;$stream.WriteByte([byte]($first -bxor 1));$stream.Flush()}finally{$stream.Dispose()}
    Reject {Get-CorePath $fixture} 'a byte changed in the executable causes SHA256 verification to fail'
    Set-OwnedTestRoot $root $fixture
    Copy-Item -LiteralPath $source -Destination $mycelium -Force
    Assert ((Get-CorePath $fixture) -eq $mycelium) 'restoring the original executable restores manifest integrity'

    [IO.File]::Delete($manifestPath)
    $stockChild=Start-FixtureCore (Get-CorePath $fixture)
    Assert-Owned $stockChild 'the running stock core is owned by its actual path and recorded start time'
    $stale=Read-JsonFile $sessionPath;$stale.started=$stockChild.StartTime.ToUniversalTime().AddSeconds(-1).ToString('o')
    Write-AtomicJson $sessionPath $stale
    Assert ($null -eq (Get-OwnedCore $root)) 'a live PID with a stale recorded start time is rejected as a possible reused PID'
    Stop-OwnedCore $root
    Assert (!$stockChild.HasExited) 'a stale PID/start-time record never terminates the live process'
    Write-Session $stockChild $stock
    $wrongPath=Read-JsonFile $sessionPath;$wrongPath.core_path=$mycelium
    Write-AtomicJson $sessionPath $wrongPath
    Assert ($null -eq (Get-OwnedCore $root)) 'a different allowed filename cannot substitute for the actual running executable path'
    Stop-OwnedCore $root
    Assert (!$stockChild.HasExited) 'an executable-path mismatch cannot terminate the running core'

    $shell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $outsider=Start-Process -FilePath $shell -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root 'outsider-out.log') -RedirectStandardError (Join-Path $root 'outsider-error.log')
    $null=$outsider.Handle;$spawned.Add($outsider)
    Write-Session $outsider $stock
    Assert ($null -eq (Get-OwnedCore $root)) 'a real outsider PowerShell PID cannot impersonate the bundled core path'
    Stop-OwnedCore $root
    Assert (!$outsider.HasExited -and !$stockChild.HasExited) 'rejecting an outsider session leaves both independently spawned processes alive'
    Write-Session $outsider $shell
    Assert ($null -eq (Get-OwnedCore $root)) 'an accurately recorded outsider executable is still outside the bundled-core allowlist'
    Stop-OwnedCore $root
    Assert (!$outsider.HasExited) 'disconnect never terminates an outsider executable'

    Write-Session $stockChild $stock
    Write-Manifest
    Assert ((Get-CorePath $fixture) -eq $mycelium) 'adding the manifest changes future launch selection from stock to Mycelium'
    Assert-Owned $stockChild 'launch selection changes do not change ownership of the already running stock core'
    Assert-Stopped $stockChild 'the stock core disconnects after selection has changed to Mycelium'
    Assert (!$outsider.HasExited) 'stopping the recorded stock core leaves the outsider child untouched'

    $nativeChild=Start-FixtureCore (Get-CorePath $fixture)
    Write-AtomicText $manifestPath '{damaged after launch'
    Reject {Get-CorePath $fixture} 'a damaged manifest still prevents the next launch'
    Assert-Owned $nativeChild 'manifest damage cannot hide the already running native core'
    Assert-Stopped $nativeChild 'the native core disconnects with a damaged manifest'

    Write-Manifest
    $nativeChild=Start-FixtureCore (Get-CorePath $fixture)
    [IO.File]::Delete($manifestPath)
    Assert ((Get-CorePath $fixture) -eq $stock) 'removing the manifest restores stock selection for future launches'
    Assert-Owned $nativeChild 'removing the manifest cannot hide the recorded native core'
    Assert-Stopped $nativeChild 'the native core disconnects after its manifest has been removed'
    Assert (!$outsider.HasExited) 'all owned-core shutdowns preserve the outsider process'
    Write-Host ('PASS: '+$passed+' Mycelium lifecycle assertions; only isolated fixture processes and loopback controllers; no service, TUN or system DNS changes')
}finally{
    $env:MUKHOMOR_ASSETS=$savedAssets;$env:MUKHOMOR_CORE_PATH=$savedCore
    # These objects hold handles obtained directly from Start-Process in this
    # fixture. Never look up another process by name or kill an arbitrary PID.
    $cleanupFailed=$false
    foreach($child in $spawned){
        try{if(!$child.HasExited){$child.Kill();if(!$child.WaitForExit(2000)){$cleanupFailed=$true}}}
        catch{$cleanupFailed=$true}
        finally{$child.Dispose()}
    }
    if($cleanupFailed){throw 'One or more fixture children could not be stopped during cleanup'}
    $resolved=[IO.Path]::GetFullPath($fixture);$prefix=[IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')+'\'
    if(!$resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notmatch '^mycelium-[a-f0-9]{32}$'){throw 'Fixture cleanup left the intended test directory'}
    if(Test-Path -LiteralPath $resolved){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
