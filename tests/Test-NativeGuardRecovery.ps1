param([string]$Executable)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
if(!$Executable){$Executable=Join-Path $package 'native\target\release\mukhomor.exe'}
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('guard-worker-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$worker=$null
$utf8=New-Object Text.UTF8Encoding($false)
# Run the real native worker/RPC/cleanup decisions. Only its privileged bridge
# is replaced with a fixture that has no adapter, WFP, service or DNS functions.
$bridge=@'
param([string]$Root)
$ErrorActionPreference='Stop'
$reader=[IO.StreamReader]::new([Console]::OpenStandardInput(),[Text.UTF8Encoding]::new($false,$true))
try{$request=$reader.ReadToEnd()|ConvertFrom-Json}finally{$reader.Dispose()}
[IO.File]::AppendAllText((Join-Path $Root 'actions.txt'),$request.action+"`n")
$statePath=Join-Path $Root 'state.json'
$state=Get-Content -LiteralPath $statePath -Raw|ConvertFrom-Json
$ok=$true;$message=''
switch($request.action){
 'Init' {}
 'Snapshot' {if($state.fail_snapshot){$ok=$false;$message='FIXTURE_SNAPSHOT_FAILURE'}}
 'Connect' {$ok=$false;$message='FIXTURE_CONNECT_FAILURE'}
 'Recover' {$state.running=$false;$state.protected=$false}
 'Disconnect' {$state.running=$false;$state.armed=$false;$state.protected=$false}
 'Exit' {$state.running=$false;$state.armed=$false;$state.protected=$false}
 default {$ok=$false;$message='Unexpected fixture action'}
}
[IO.File]::WriteAllText($statePath,($state|ConvertTo-Json -Compress))
$data=@{selected='fixture';profiles=@();autoconnect=$false;settings=@{dns_update=@{enabled=$false}};running=[bool]$state.running;starting=$false;dns_recovery_pending=$false;traffic_guard=@{configured=[bool]$state.armed;armed=[bool]$state.armed;verified=$true;protected=[bool]$state.protected}}
$reply=@{ok=$ok;error=$message;data=$data}|ConvertTo-Json -Depth 12 -Compress
$bytes=[Text.Encoding]::UTF8.GetBytes($reply+"`n")
$out=[Console]::OpenStandardOutput();$out.Write($bytes,0,$bytes.Length);$out.Flush()
'@
function Request($Value){
    $inputPath=Join-Path $script:root 'request.json';$outputPath=Join-Path $script:root 'reply.json'
    [IO.File]::WriteAllText($inputPath,($Value|ConvertTo-Json -Compress),$utf8)
    [IO.File]::Delete($outputPath)
    $p=Start-Process -FilePath $Executable -ArgumentList ('--rpc --root "'+$script:root+'" --request "'+$inputPath+'" --output "'+$outputPath+'"') -WindowStyle Hidden -PassThru
    try{if(!$p.WaitForExit(110000)){$p.Kill();throw 'Fixture RPC deadline exceeded'};if(!(Test-Path -LiteralPath $outputPath)){throw 'Missing fixture RPC response'};Get-Content -LiteralPath $outputPath -Raw|ConvertFrom-Json}finally{$p.Dispose()}
}
function Start-Worker([string]$Name,[bool]$Running,[bool]$FailSnapshot){
    $script:root=Join-Path $fixture $Name
    foreach($dir in @('runtime','private')){[IO.Directory]::CreateDirectory((Join-Path $script:root $dir))|Out-Null}
    [IO.File]::WriteAllText((Join-Path $script:root 'qa.marker'),'isolated native worker test')
    [IO.File]::WriteAllText((Join-Path $script:root 'private\traffic-guard-plan.json'),'{}')
    [IO.File]::WriteAllText((Join-Path $script:root 'state.json'),(@{running=$Running;armed=$true;protected=$true;fail_snapshot=$FailSnapshot}|ConvertTo-Json -Compress))
    $script:worker=Start-Process -FilePath $Executable -ArgumentList ('--worker --base "'+$fixture+'" --root "'+$script:root+'" --output "'+(Join-Path $script:root 'worker-error.txt')+'"') -WindowStyle Hidden -PassThru
    $watch=[Diagnostics.Stopwatch]::StartNew()
    while($watch.Elapsed.TotalSeconds -lt 35){
        $reply=Request @{action='Status'}
        if($reply.ok){return $reply}
        if($script:worker.HasExited){throw 'Fixture worker exited during startup'}
        Start-Sleep -Milliseconds 100
    }
    throw 'Fixture worker never became ready'
}
function Stop-Worker {
    if($worker -and !$worker.HasExited){$null=Request @{action='ShutdownWorker'};if(!$worker.WaitForExit(30000)){$worker.Kill();throw 'Fixture worker shutdown deadline exceeded'}}
    if($worker){$worker.Dispose();$script:worker=$null}
}
function Assert([bool]$Condition,[string]$Message){if(!$Condition){throw $Message}}
try{
    [IO.File]::WriteAllText((Join-Path $fixture 'NativeBridge.ps1'),$bridge,$utf8)
    $reply=Start-Worker 'startup' $true $false
    $actions=@(Get-Content -LiteralPath (Join-Path $root 'actions.txt'))
    Assert ($actions -contains 'Recover' -and $actions -notcontains 'Disconnect') 'Automatic startup cleanup released protection'
    Assert ($reply.data.traffic_guard.armed -eq $true -and $reply.data.traffic_guard.protected -eq $false -and $reply.data.phase -eq 'recovery') 'Startup cleanup misreported a protected tunnel'
    $reply=Request @{action='Disconnect'}
    Assert ($reply.ok -and $reply.data.traffic_guard.armed -eq $false) 'Explicit user disconnect did not release protection'
    Stop-Worker
    Write-Host 'PASS: real worker startup recovery retains guard; explicit user disconnect releases it'

    $reply=Start-Worker 'startup-snapshot-error' $true $true
    $actions=@(Get-Content -LiteralPath (Join-Path $root 'actions.txt'))
    Assert ($actions -contains 'Recover' -and $actions -contains 'Snapshot' -and $actions -notcontains 'Disconnect') 'Startup snapshot-error scenario did not recover without release'
    Assert ($reply.data.status_uncertain -eq $true -and $reply.data.traffic_guard.configured -eq $true -and $reply.data.traffic_guard.verified -eq $false -and $reply.data.traffic_guard.protected -eq $false -and $reply.data.phase -eq 'recovery') 'Failed startup snapshot retained a false protection claim'
    Stop-Worker
    $state=Get-Content -LiteralPath (Join-Path $root 'state.json') -Raw|ConvertFrom-Json
    Assert ($state.armed -eq $true) 'Startup snapshot-error shutdown released persistent protection'
    Write-Host 'PASS: real worker startup snapshot failure clears verified/protected and retains guard'

    $null=Start-Worker 'failure' $false $true
    $reply=Request @{action='Connect'}
    Assert (!$reply.ok) 'Failed fixture Connect unexpectedly succeeded'
    $reply=Request @{action='Status'}
    Assert ($reply.ok -and $reply.data.traffic_guard.configured -eq $true -and $reply.data.traffic_guard.verified -eq $false -and $reply.data.traffic_guard.protected -eq $false -and $reply.data.phase -eq 'recovery') 'Cached snapshot retained a false protection claim'
    Stop-Worker
    $actions=@(Get-Content -LiteralPath (Join-Path $root 'actions.txt'))
    $state=Get-Content -LiteralPath (Join-Path $root 'state.json') -Raw|ConvertFrom-Json
    Assert (@($actions|Where-Object {$_ -eq 'Recover'}).Count -ge 2 -and $actions -notcontains 'Disconnect' -and $actions -notcontains 'Exit' -and $state.armed -eq $true) 'Connect failure or automatic worker stop released persistent protection'
    Write-Host 'PASS: real worker failure and automatic shutdown retain guard; failed snapshot clears verified/protected'
}finally{
    if($worker){try{Stop-Worker}catch{if(!$worker.HasExited){$worker.Kill()};$worker.Dispose()}}
    $absolute=[IO.Path]::GetFullPath($fixture)
    if([IO.Path]::GetDirectoryName($absolute) -cne [IO.Path]::GetFullPath($PSScriptRoot) -or [IO.Path]::GetFileName($absolute) -cnotmatch '^guard-worker-[a-f0-9]{32}$'){throw 'Unexpected fixture cleanup path'}
    foreach($item in @((Get-Item -LiteralPath $absolute))+@(Get-ChildItem -LiteralPath $absolute -Recurse -Force)){if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Fixture cleanup refuses reparse points'}}
    Remove-Item -LiteralPath $absolute -Recurse -Force
}
