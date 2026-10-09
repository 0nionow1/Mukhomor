param([string]$Executable,[switch]$BridgeFramingOnly,[switch]$CancellationOnly)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
if(!$Executable){$Executable=Join-Path $package 'native\target\release\mukhomor.exe'}
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('disconnect-'+[guid]::NewGuid().ToString('N'))
$base=Join-Path $fixture 'assets';$root=Join-Path $fixture 'state'
[IO.Directory]::CreateDirectory($base)|Out-Null
[IO.Directory]::CreateDirectory((Join-Path $root 'runtime'))|Out-Null
[IO.File]::WriteAllText((Join-Path $root 'qa.marker'),'Isolated controller test. No TUN or system DNS.')
$passed=0;$worker=$null;$clients=New-Object Collections.Generic.List[object]
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
function Start-Request($Value){
    $id=[guid]::NewGuid().ToString('N');$input=Join-Path $fixture ($id+'-request.json');$output=Join-Path $fixture ($id+'-response.json')
    [IO.File]::WriteAllText($input,($Value|ConvertTo-Json -Depth 20 -Compress),(New-Object Text.UTF8Encoding($false)))
    $client=Start-Process -FilePath $Executable -ArgumentList ('--rpc --root "'+$root+'" --request "'+$input+'" --output "'+$output+'"') -PassThru -WindowStyle Hidden
    $null=$client.Handle
    $item=@{process=$client;output=$output;action=$Value.action};$clients.Add($item);return $item
}
# Generic RPC completion follows the 100 s client deadline; explicit cancellation/Exit bounds remain below.
function Finish-Request($Item,[int]$Timeout=115000){
    if(!$Item.process.WaitForExit($Timeout)){throw ('Test RPC '+$Item.action+' exceeded its deadline after '+$script:passed+' assertions')}
    if($Item.process.ExitCode -ne 0){throw ('Test RPC failed: '+[IO.File]::ReadAllText($Item.output))}
    return ([IO.File]::ReadAllText($Item.output)|ConvertFrom-Json)
}
function Request($Value){return Finish-Request (Start-Request $Value)}
function Start-Worker {
    $script:worker=Start-Process -FilePath $Executable -ArgumentList ('--worker --base "'+$base+'" --root "'+$root+'" --output "'+(Join-Path $fixture 'worker-error.json')+'"') -WindowStyle Hidden -PassThru
    $ready=$false
    $readiness=[Diagnostics.Stopwatch]::StartNew()
    while($readiness.Elapsed.TotalSeconds -lt 60){try{if((Request @{action='Status'}).ok){$ready=$true;break}}catch{if($worker.HasExited){$errorPath=Join-Path $fixture 'worker-error.json';$detail=if(Test-Path -LiteralPath $errorPath){[IO.File]::ReadAllText($errorPath)}else{'No worker error record'};throw ('Fixture worker exited: '+$detail)}};Start-Sleep -Milliseconds 100}
    Assert $ready 'isolated controller starts'
}
# The trusted bridge is replaced only for this isolated --worker fixture.
# These JSON booleans simulate a slow connection and DNS rollback; no adapter,
# real profile, real VPN engine or DNS setting is accessed by this test.
$fake=@'
param([string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$statePath=Join-Path $Root 'runtime\mock-state.json'
function Save($State){[IO.File]::WriteAllText($statePath,($State|ConvertTo-Json -Depth 20 -Compress))}
function Snapshot {
    $state=Get-Content -LiteralPath $statePath -Raw|ConvertFrom-Json
    return @{profiles=@(@{id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';name='Fixture A'},@{id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';name='Fixture B'});selected=$state.selected;autoconnect=$state.autoconnect;running=$state.running;starting=$state.starting;dns_recovery_pending=$state.recovery;settings=@{dns_update=@{enabled=$false}}}
}
function Stubborn-Work {
    $windows=[Environment]::GetFolderPath('Windows')
    $engine=Start-Process -FilePath (Join-Path $windows 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') -WindowStyle Hidden -PassThru
    [IO.File]::WriteAllText((Join-Path $Root 'runtime\child-process.json'),(@{pid=$engine.Id;started=$engine.StartTime.ToUniversalTime().ToString('o')}|ConvertTo-Json -Compress))
    Start-Sleep -Seconds 60 # Intentionally never observes stop.request.
}
function Hold-Output {
    # Match production redirects; an unused ancestor pipe writer may also
    # cross .NET Framework CreateProcess's inheritable-handle boundary.
    $windows=[Environment]::GetFolderPath('Windows')
    $logWork="[Console]::WriteLine('immediate-output');[Console]::Error.WriteLine('immediate-error');Start-Sleep -Seconds 2;[Console]::WriteLine('delayed-output');[Console]::Error.WriteLine('delayed-error');Start-Sleep -Seconds 60"
    $engine=Start-Process -FilePath (Join-Path $windows 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList @('-NoProfile','-NonInteractive','-Command',$logWork) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $Root 'runtime\holder-out.log') -RedirectStandardError (Join-Path $Root 'runtime\holder-error.log')
    [IO.File]::WriteAllText((Join-Path $Root 'runtime\output-holder.json'),(@{pid=$engine.Id;started=$engine.StartTime.ToUniversalTime().ToString('o')}|ConvertTo-Json -Compress))
}
function Stop-OutputHolder {
    $path=Join-Path $Root 'runtime\output-holder.json'
    if(!(Test-Path -LiteralPath $path)){return}
    $record=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json
    $child=Get-Process -Id $record.pid -ErrorAction SilentlyContinue
    if($child -and $child.StartTime.ToUniversalTime().ToString('o') -eq $record.started){$child.Kill();$child.WaitForExit(1000)|Out-Null}
    [IO.File]::Delete($path)
}
try {
    if(!(Test-Path -LiteralPath $statePath)){Save @{running=$false;starting=$false;recovery=$false;autoconnect=$false;connections=0;selected='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'}}
    if(Test-Path -LiteralPath (Join-Path $Root 'runtime\block-input.marker')){
        [IO.File]::WriteAllText((Join-Path $Root 'runtime\blocked-input-entered.marker'),'No stdin read by fixture helper')
        Start-Sleep -Seconds 60
    }
    $request=[Console]::In.ReadToEnd()|ConvertFrom-Json
    $state=Get-Content -LiteralPath $statePath -Raw|ConvertFrom-Json
    switch($request.action){
        'Init' { if(Test-Path -LiteralPath (Join-Path $Root 'runtime\fail-init.marker')){throw 'Synthetic Init failure'} }
        'Snapshot' { if(Test-Path -LiteralPath (Join-Path $Root 'runtime\fail-snapshot.marker')){throw 'Synthetic Snapshot failure'} }
        'Autoconnect' {$state.autoconnect=$request.enabled;Save $state}
        'Select' {
            if($null -ne $request.PSObject.Properties['slow'] -and $request.slow){
                [IO.File]::WriteAllText((Join-Path $Root 'runtime\select-entered.marker'),'selecting')
                Start-Sleep -Seconds 60
            }
            $state.selected=$request.id;Save $state
        }
        'Connect' {
            $state.connections++;$state.starting=$true;Save $state
            [IO.File]::WriteAllText((Join-Path $Root 'runtime\entered.marker'),'connecting')
            if($null -ne $request.PSObject.Properties['failWork'] -and $request.failWork){throw 'Synthetic Connect failure'}
            if($null -ne $request.PSObject.Properties['unicodeFailure'] -and $request.unicodeFailure){throw (-join [char[]](0x0422,0x0435,0x0441,0x0442,13,10,0x65e5,0x672c))}
            if($null -ne $request.PSObject.Properties['noncooperative'] -and $request.noncooperative){
                Stubborn-Work
            }
            $fast=($null -ne $request.PSObject.Properties['fast'] -and $request.fast) -or (Test-Path -LiteralPath (Join-Path $Root 'runtime\fast-startup.marker'))
            if(!$fast){
                for($n=0;$n -lt 750;$n++){
                    if(Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')){
                        $state.running=$false;$state.starting=$false;Save $state;throw 'Startup cancelled'
                    }
                    Start-Sleep -Milliseconds 40
                }
            }
            $state.starting=$false;$state.running=$true;Save $state
            if($null -ne $request.PSObject.Properties['holdOutput'] -and $request.holdOutput){Hold-Output}
            if($null -ne $request.PSObject.Properties['frameThenSleep'] -and $request.frameThenSleep){
                [Console]::WriteLine((@{ok=$true;data=(Snapshot)}|ConvertTo-Json -Depth 20 -Compress));[Console]::Out.Flush()
                [IO.File]::WriteAllText((Join-Path $Root 'runtime\frame-sent.marker'),'Frame emitted; operation deliberately not complete')
                Start-Sleep -Seconds 60
            }
            if($null -ne $request.PSObject.Properties['noNewline'] -and $request.noNewline){
                [Console]::Write((@{ok=$true;data=(Snapshot)}|ConvertTo-Json -Depth 20 -Compress));[Console]::Out.Flush();exit 0
            }
        }
        'Disconnect' {
            Stop-OutputHolder
            $state.running=$false;$state.starting=$false
            if(Test-Path -LiteralPath (Join-Path $Root 'runtime\hang-disconnect.marker')){
                $state.recovery=$true;Save $state;Stubborn-Work
            }
            if(Test-Path -LiteralPath (Join-Path $Root 'runtime\fail-dns.marker')){
                [IO.File]::Delete((Join-Path $Root 'runtime\fail-dns.marker'))
                $state.recovery=$true;Save $state;throw 'Simulated unavailable DNS adapter'
            }
            $state.recovery=$false;Save $state
        }
        'Exit' {
            $state.running=$false;$state.starting=$false
            if(Test-Path -LiteralPath (Join-Path $Root 'runtime\fail-dns.marker')){
                [IO.File]::Delete((Join-Path $Root 'runtime\fail-dns.marker'))
                $state.recovery=$true;Save $state;throw 'Simulated unavailable DNS adapter'
            }
            $state.recovery=$false;Save $state
        }
        'Update' {if($null -eq $request.PSObject.Properties['fast'] -or !$request.fast){Stubborn-Work}}
        default {throw 'Unsupported fixture operation'}
    }
    [Console]::WriteLine((@{ok=$true;data=(Snapshot)}|ConvertTo-Json -Depth 20 -Compress))
} catch {[Console]::WriteLine((@{ok=$false;error=$_.Exception.Message}|ConvertTo-Json -Compress));exit 1}
'@
[IO.File]::WriteAllText((Join-Path $base 'NativeBridge.ps1'),$fake,(New-Object Text.UTF8Encoding($false)))
try {
    Start-Worker
    if($CancellationOnly){
        Assert (Request @{action='Connect';fast=$true;request_id='cancellation-initial-connect'}).ok 'initial explicit Connect succeeds'
        Assert (Request @{action='Disconnect';request_id='cancellation-initial-stop'}).ok 'manual Disconnect completes before the next selection'
        $selected=Request @{action='Select';id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';request_id='select-after-disconnect'}
        Assert ($selected.ok -and !$selected.data.running -and !$selected.data.busy -and $selected.data.selected -eq 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb') ('new Select must not inherit the previous Disconnect cancellation: '+($selected|ConvertTo-Json -Depth 8 -Compress))
        Assert (Request @{action='Connect';fast=$true;request_id='connect-after-selection'}).ok 'Connect after selecting a different server succeeds without restarting the controller'
        Assert (Request @{action='Disconnect'}).ok 'second Disconnect reaches a terminal state'
        Assert (Request @{action='Update';fast=$true;request_id='update-after-disconnect'}).ok 'a new rule update must not inherit completed Disconnect cancellation'
        $slowSelect=Start-Request @{action='Select';id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';slow=$true;request_id='cancel-active-selection'}
        $selectEntered=Join-Path $root 'runtime\select-entered.marker'
        for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $selectEntered);$n++){Start-Sleep -Milliseconds 30}
        Assert (Test-Path -LiteralPath $selectEntered) 'fresh selection enters its helper after clearing old cancellation'
        $selectStop=Start-Request @{action='Disconnect';request_id='stop-active-selection'}
        $selectCancelled=Finish-Request $slowSelect;$selectOff=Finish-Request $selectStop
        Assert ($selectCancelled.cancelled -and $selectOff.ok -and !$selectOff.data.busy) 'a newer Disconnect still cancels an active selection'
        Assert (Request @{action='Select';id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'}).ok 'selection can be retried immediately after its cancellation'
        $slowUpdate=Start-Request @{action='Update';request_id='cancel-active-update'}
        $updateEntered=Join-Path $root 'runtime\child-process.json'
        for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $updateEntered);$n++){Start-Sleep -Milliseconds 30}
        Assert (Test-Path -LiteralPath $updateEntered) 'fresh rule update enters its blocking helper'
        $updateRecord=Get-Content -LiteralPath $updateEntered -Raw|ConvertFrom-Json
        $updateChild=Get-Process -Id $updateRecord.pid -ErrorAction Stop
        $updateStop=Start-Request @{action='Disconnect';request_id='stop-active-update'}
        $updateCancelled=Finish-Request $slowUpdate;$updateOff=Finish-Request $updateStop
        Assert ($updateCancelled.cancelled -and $updateOff.ok -and !$updateOff.data.busy -and $updateChild.WaitForExit(3000)) 'a newer Disconnect still cancels the active update and its exact child'
        Assert (Request @{action='Update';fast=$true}).ok 'a new rule update succeeds immediately after cancellation'
        for($cycle=0;$cycle -lt 5;$cycle++){
            $entered=Join-Path $root 'runtime\entered.marker';[IO.File]::Delete($entered)
            $slow=Start-Request @{action='Connect';request_id=('race-connect-'+$cycle)}
            for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $entered);$n++){Start-Sleep -Milliseconds 30}
            Assert (Test-Path -LiteralPath $entered) ('cycle '+$cycle+' reaches its blocking fixture startup')
            $stop=Start-Request @{action='Disconnect';request_id=('race-stop-'+$cycle)}
            $cancelled=Finish-Request $slow;$off=Finish-Request $stop
            Assert ($cancelled.cancelled -and $off.ok -and !$off.data.busy) ('cycle '+$cycle+' cancels only the old connection and completes cleanup')
            $target=if($cycle%2){'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'}else{'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'}
            $selection=Request @{action='Select';id=$target;request_id=('race-select-'+$cycle)}
            Assert ($selection.ok -and $selection.data.selected -eq $target -and !$selection.data.busy) ('cycle '+$cycle+' can select another profile after cancellation')
            $fresh=Request @{action='Connect';fast=$true;request_id=('race-reconnect-'+$cycle)}
            Assert ($fresh.ok -and $fresh.data.running -and !$fresh.data.busy -and !(Test-Path -LiteralPath (Join-Path $root 'runtime\stop.request'))) ('cycle '+$cycle+' reconnects with fresh cancellation state')
            Assert (Request @{action='Disconnect'}).ok ('cycle '+$cycle+' disconnects again without controller restart')
        }
        # ShutdownWorker acknowledges before cleanup; its Disconnect helper has a 20 s hard limit.
        # Give that bounded cleanup time to finish on a cold Windows runner.
        Request @{action='ShutdownWorker'}|Out-Null
        Assert ($worker.WaitForExit(30000)) 'cancellation fixture stops all its own descendants'
        Write-Host ('PASS: '+$passed+' isolated cancellation reset assertions; no TUN, DNS or service changes')
        return
    }
    if($BridgeFramingOnly){
        $clock=[Diagnostics.Stopwatch]::StartNew()
        $complete=Request @{action='Connect';fast=$true;holdOutput=$true;request_id='inherited-output-fixture'}
        Assert ($complete.ok -and $complete.data.running -and !$complete.data.busy) ('complete response must not wait for descendant stdout EOF: '+($complete|ConvertTo-Json -Depth 8 -Compress))
        Assert ($clock.ElapsedMilliseconds -lt 5000) 'complete framed response returns promptly after helper exit'
        $record=Get-Content -LiteralPath (Join-Path $root 'runtime\output-holder.json') -Raw|ConvertFrom-Json
        $holder=Get-Process -Id $record.pid -ErrorAction SilentlyContinue
        Assert ($holder -and !$holder.HasExited -and $holder.StartTime.ToUniversalTime().ToString('o') -eq $record.started) 'successful receipt leaves the isolated long-lived child running'
        Start-Sleep -Milliseconds 2300
        $logs=@{}
        foreach($name in @('holder-out.log','holder-error.log')){
            # Production redirects remain open by the engine; the test reader
            # must permit the writer to keep its existing read/write sharing.
            $stream=[IO.File]::Open((Join-Path $root ('runtime\'+$name)),[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
            $reader=New-Object IO.StreamReader($stream)
            try{$logs[$name]=$reader.ReadToEnd()}finally{$reader.Dispose()}
        }
        $outLog=$logs['holder-out.log'];$errorLog=$logs['holder-error.log']
        Assert ($outLog.Contains('immediate-output') -and $outLog.Contains('delayed-output') -and $errorLog.Contains('immediate-error') -and $errorLog.Contains('delayed-error')) 'redirected stdout and stderr still reach their logfiles after helper exit'
        Assert (Request @{action='Status'}).data.running 'subsequent Status remains available with inherited output handle open'
        Assert (Request @{action='Disconnect'}).ok 'disconnect remains available after a framed connection receipt'
        Assert ($holder.WaitForExit(3000)) 'disconnect stops only the exact fixture descendant'
        $early=Start-Request @{action='Connect';fast=$true;frameThenSleep=$true;request_id='frame-before-operation-complete'}
        $sent=Join-Path $root 'runtime\frame-sent.marker'
        for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $sent);$n++){Start-Sleep -Milliseconds 30}
        Assert (Test-Path -LiteralPath $sent) 'fixture emits its reply before deliberately blocking'
        Start-Sleep -Milliseconds 300
        Assert (!$early.process.HasExited -and (Request @{action='Status'}).data.busy) 'a frame alone cannot commit an operation before its helper finishes'
        $cancel=Request @{action='Disconnect'};$earlyReply=Finish-Request $early
        Assert ($cancel.ok -and $earlyReply.cancelled) 'Disconnect still cancels a helper after its frame was received'
        $truncated=Request @{action='Connect';fast=$true;noNewline=$true;request_id='truncated-frame-fixture'}
        Assert (!$truncated.ok -and !$truncated.data.busy -and $truncated.data.last_operation.request_id -eq 'truncated-frame-fixture') 'unterminated response fails explicitly with a terminal operation state'
        $unicode=Request @{action='Connect';fast=$true;unicodeFailure=$true;request_id='unicode-error-frame-fixture'}
        $expected=-join [char[]](0x0422,0x0435,0x0441,0x0442,13,10,0x65e5,0x672c)
        Assert (!$unicode.ok -and $unicode.error -ceq $expected) 'framed errors retain exact Unicode and embedded CRLF details'
        # ShutdownWorker acknowledges before cleanup; its Disconnect helper has a 20 s hard limit.
        # Give that bounded cleanup time to finish on a cold Windows runner.
        Request @{action='ShutdownWorker'}|Out-Null
        Assert ($worker.WaitForExit(30000)) 'framing fixture shuts down all its supervised descendants'
        Write-Host ('PASS: '+$passed+' isolated bridge framing assertions; no TUN, DNS or service changes')
        return
    }
    Assert (Request @{action='Autoconnect';enabled=$true}).ok 'boot preference can be enabled'
    $connect=Start-Request @{action='Connect';request_id='connecting-fixture-1'}
    $entered=Join-Path $root 'runtime\entered.marker'
    for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $entered);$n++){Start-Sleep -Milliseconds 40}
    Assert (Test-Path -LiteralPath $entered) 'slow connection is in progress'
    $timer=[Diagnostics.Stopwatch]::StartNew();$status=Request @{action='Status'};$timer.Stop()
    Assert ($timer.ElapsedMilliseconds -lt 1500 -and $status.data.phase -eq 'connecting' -and $status.data.can_disconnect) 'status and cancel remain available during connection'
    $repeated=Request @{action='Connect';fast=$true;request_id='repeated-connect-fixture'}
    Assert (!$repeated.ok -and $repeated.request_id -eq 'repeated-connect-fixture' -and $repeated.data.request_id -eq 'connecting-fixture-1') 'repeated Connect is rejected promptly without replacing the accepted operation'
    $disconnect=Start-Request @{action='Disconnect';request_id='disconnect-fixture-1'}
    $marker=Join-Path $root 'runtime\stop.request'
    for($n=0;$n -lt 50 -and !(Test-Path -LiteralPath $marker);$n++){Start-Sleep -Milliseconds 20}
    Assert (Test-Path -LiteralPath $marker) 'disconnect signals startup cancellation immediately'
    $cancelled=Finish-Request $connect;$off=Finish-Request $disconnect
    Assert (!$cancelled.ok -and $cancelled.cancelled -and $off.ok) ('pending connection is cancelled and disconnect completes: '+(@{connect=$cancelled;disconnect=$off}|ConvertTo-Json -Depth 8 -Compress))
    $status=Request @{action='Status'}
    Assert (!$status.data.running -and !$status.data.starting -and $status.data.phase -eq 'idle' -and $status.data.autoconnect) 'manual disconnect reaches idle while preserving next-boot preference'
    Assert (!$status.data.busy -and $status.data.last_operation.ok -and $status.data.last_operation.request_id -eq 'disconnect-fixture-1' -and $status.data.completed_operation_id -eq $status.data.operation_id) 'terminal Status identifies the latest request and cannot inherit cancelled Connect busy state'
    Start-Sleep -Seconds 6
    $mock=Get-Content -LiteralPath (Join-Path $root 'runtime\mock-state.json') -Raw|ConvertFrom-Json
    Assert ($mock.connections -eq 1 -and !(Request @{action='Status'}).data.running) 'manual disconnect suppresses retry despite enabled autoconnect'
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(30000)) 'disconnected controller stops before its restart test'
    Start-Worker
    $restart=Request @{action='Status'}
    $saved=Get-Content -LiteralPath (Join-Path $root 'runtime\connection-intent.json') -Raw|ConvertFrom-Json
    Assert (!$restart.data.running -and $restart.data.autoconnect -and !$saved.desired -and $restart.data.phase -eq 'idle') 'SCM-style controller restart preserves manual disconnect despite next-boot autoconnect'
    $on=Request @{action='Connect';fast=$true}
    Assert ($on.ok -and $on.data.phase -eq 'connected' -and !(Test-Path -LiteralPath $marker)) 'a new explicit connection clears only its own cancellation marker'
    [IO.File]::WriteAllText((Join-Path $root 'runtime\fail-dns.marker'),'simulate unavailable adapter')
    $failed=Request @{action='Disconnect'}
    Assert (!$failed.ok -and !$failed.data.running -and $failed.data.phase -eq 'recovery') 'DNS restore failure never reports the VPN as connected'
    $recovered=Request @{action='Disconnect'}
    Assert ($recovered.ok -and $recovered.data.phase -eq 'idle') 'network recovery can be retried through the same disconnect action'
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(30000)) 'controller exits after cancellation tests'
    [IO.File]::WriteAllText((Join-Path $root 'runtime\qa-system-boot.marker'),'Model SCM AUTO/DELAYEDAUTO reason, never actual system startup')
    [IO.File]::WriteAllText((Join-Path $root 'runtime\fast-startup.marker'),'No tunnel, immediate fixture success')
    Start-Worker
    $boot=$null
    for($n=0;$n -lt 40;$n++){$boot=Request @{action='Status'};if($boot.data.running){break};Start-Sleep -Milliseconds 50}
    Assert ($boot.data.running -and $boot.data.autoconnect) 'actual-boot mode honors autoconnect after a manual disconnect in the preceding boot'
    $bootSaved=Get-Content -LiteralPath (Join-Path $root 'runtime\connection-intent.json') -Raw|ConvertFrom-Json
    Assert $bootSaved.desired 'actual boot persists desired connection before its automatic attempt'
    Request @{action='Disconnect'}|Out-Null
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(30000)) 'boot-mode fixture shuts down without a real VPN'
    [IO.File]::Delete((Join-Path $root 'runtime\qa-system-boot.marker'))
    [IO.File]::WriteAllText((Join-Path $root 'runtime\connection-intent.json'),'Damaged synthetic intent record')
    Start-Worker
    $damaged=Request @{action='Status'}
    Assert (!$damaged.data.running -and $damaged.data.phase -eq 'idle' -and $damaged.data.last_error) 'damaged persisted intent defaults to disconnected while keeping management available'
    [IO.File]::Delete((Join-Path $root 'runtime\entered.marker'))
    $stubborn=Start-Request @{action='Connect';noncooperative=$true}
    $childRecord=Join-Path $root 'runtime\child-process.json'
    for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $childRecord);$n++){Start-Sleep -Milliseconds 30}
    Assert (Test-Path -LiteralPath $childRecord) 'noncooperative helper spawns a fixture child inside its supervised job'
    $childInfo=Get-Content -LiteralPath $childRecord -Raw|ConvertFrom-Json
    $child=Get-Process -Id $childInfo.pid -ErrorAction Stop
    Assert ($child.StartTime.ToUniversalTime().ToString('o') -eq $childInfo.started) 'fixture child identity is recorded before cancellation'
    $cancelTimer=[Diagnostics.Stopwatch]::StartNew();$stop=Start-Request @{action='Disconnect'}
    $stubbornReply=Finish-Request $stubborn;$stopped=Finish-Request $stop;$cancelTimer.Stop()
    Assert ($cancelTimer.Elapsed.TotalSeconds -lt 4 -and $stubbornReply.cancelled -and $stopped.ok) 'noncooperative helper cancels promptly without waiting for its 60-second sleep'
    Assert ($child.WaitForExit(2000)) 'job cancellation also removes its child before a session file could be written'
    $reuse=Request @{action='Connect';fast=$true}
    Assert ($reuse.ok -and $reuse.data.running) 'supervision job accepts a new connection after job termination'
    [IO.File]::Delete($childRecord)
    $update=Start-Request @{action='Update'}
    for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $childRecord);$n++){Start-Sleep -Milliseconds 30}
    Assert (Test-Path -LiteralPath $childRecord) 'a background update is allowed to block in its fixture helper'
    $updateInfo=Get-Content -LiteralPath $childRecord -Raw|ConvertFrom-Json
    $updateChild=Get-Process -Id $updateInfo.pid -ErrorAction Stop
    $updateTimer=[Diagnostics.Stopwatch]::StartNew();$stopUpdate=Start-Request @{action='Disconnect'}
    $updateReply=Finish-Request $update;$offUpdate=Finish-Request $stopUpdate;$updateTimer.Stop()
    Assert ($updateTimer.Elapsed.TotalSeconds -lt 4 -and $updateReply.cancelled -and $offUpdate.ok) 'disconnect interrupts a stalled update instead of waiting behind it'
    Assert ($updateChild.WaitForExit(2000)) 'update cancellation kills only its supervised fixture child'

    [IO.File]::WriteAllText((Join-Path $root 'runtime\qa-operation-limits.json'),'{"Disconnect":1000,"Exit":1000,"Snapshot":1000,"Init":1000}')
    [IO.File]::WriteAllText((Join-Path $root 'runtime\fail-snapshot.marker'),'Snapshot failure is synthetic')
    $snapshotFailure=Request @{action='Connect';failWork=$true;request_id='snapshot-failure-fixture'}
    Assert (!$snapshotFailure.ok -and !$snapshotFailure.data.busy -and !$snapshotFailure.data.starting -and $snapshotFailure.data.phase -eq 'idle' -and $snapshotFailure.data.status_uncertain) 'failed cleanup Snapshot reaches a finite disconnected terminal state'
    Assert ($snapshotFailure.data.last_operation.request_id -eq 'snapshot-failure-fixture' -and !$snapshotFailure.data.last_operation.ok -and $snapshotFailure.data.last_error) 'Snapshot failure preserves the action error and terminal request identity'
    [IO.File]::Delete((Join-Path $root 'runtime\fail-snapshot.marker'))

    [IO.File]::WriteAllText((Join-Path $root 'runtime\hang-disconnect.marker'),'No network adapter or DNS is accessed')
    [IO.File]::WriteAllText((Join-Path $root 'runtime\dns-backup.json'),'{}')
    [IO.File]::Delete($childRecord)
    $deadlineClock=[Diagnostics.Stopwatch]::StartNew()
    $boundedStop=Request @{action='Disconnect';request_id='bounded-disconnect-fixture'}
    $deadlineClock.Stop()
    Assert (!$boundedStop.ok -and !$boundedStop.data.busy -and $boundedStop.data.phase -eq 'recovery' -and $boundedStop.data.last_operation.request_id -eq 'bounded-disconnect-fixture' -and $deadlineClock.ElapsedMilliseconds -lt 4500) 'noncooperative Disconnect ends with a recoverable error within its hard deadline'
    $boundedInfo=Get-Content -LiteralPath $childRecord -Raw|ConvertFrom-Json
    $boundedChild=Get-Process -Id $boundedInfo.pid -ErrorAction SilentlyContinue
    Assert (!$boundedChild -or $boundedChild.WaitForExit(2000)) 'Disconnect hard timeout also removes its supervised child'
    [IO.File]::Delete((Join-Path $root 'runtime\hang-disconnect.marker'))
    [IO.File]::Delete((Join-Path $root 'runtime\dns-backup.json'))
    Assert (Request @{action='Disconnect'}).ok 'the same controller can recover after a timed out Disconnect'

    [IO.File]::WriteAllText((Join-Path $root 'runtime\block-input.marker'),'Helper deliberately ignores its stdin')
    $blocked=Start-Request @{action='Connect';padding=('x'*524288);request_id='blocked-stdin-fixture'}
    $blockedMarker=Join-Path $root 'runtime\blocked-input-entered.marker'
    for($n=0;$n -lt 100 -and !(Test-Path -LiteralPath $blockedMarker);$n++){Start-Sleep -Milliseconds 30}
    Assert (Test-Path -LiteralPath $blockedMarker) 'large request can reach a helper that deliberately never reads stdin'
    [IO.File]::Delete((Join-Path $root 'runtime\block-input.marker'))
    $inputClock=[Diagnostics.Stopwatch]::StartNew();$inputOff=Start-Request @{action='Disconnect';request_id='cancel-blocked-stdin-fixture'}
    $blockedReply=Finish-Request $blocked;$inputOffReply=Finish-Request $inputOff;$inputClock.Stop()
    Assert ($blockedReply.cancelled -and $inputOffReply.ok -and $inputClock.ElapsedMilliseconds -lt 4500) 'Disconnect cancels a blocked helper stdin write without waiting for the helper sleep'

    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(30000)) 'damaged-state fixture shuts down'
    [IO.Directory]::CreateDirectory((Join-Path $root 'runtime\connection-intent.json.pending'))|Out-Null
    Start-Worker
    $persistFailed=Request @{action='Status'}
    Assert ($persistFailed.ok -and !$persistFailed.data.running -and $persistFailed.data.last_error) 'intent persistence failure keeps management responsive instead of deadlocking startup'
    $unsaved=Request @{action='Connect';fast=$true;request_id='unsaved-intent-fixture'}
    Assert (!$unsaved.ok -and !$unsaved.data.busy -and $unsaved.data.last_operation.request_id -eq 'unsaved-intent-fixture') 'an unsaved Connect fails explicitly and reaches a terminal state'
    [IO.Directory]::Delete((Join-Path $root 'runtime\connection-intent.json.pending'))
    Add-Type -TypeDefinition @'
using System;
using System.Text;
public static class MukhomorFixturePipe {
    public static string Name(string root) {
        ulong hash = 0xcbf29ce484222325UL;
        foreach (byte b in Encoding.UTF8.GetBytes(root.ToLowerInvariant()))
            hash = unchecked((hash ^ b) * 0x100000001b3UL);
        return "Mukhomor-" + hash.ToString("x16");
    }
}
'@
    $partial=New-Object Collections.Generic.List[object]
    try {
        for($n=0;$n -lt 4;$n++){
            $stream=New-Object IO.Pipes.NamedPipeClientStream('.',([MukhomorFixturePipe]::Name($root)),[IO.Pipes.PipeDirection]::InOut,[IO.Pipes.PipeOptions]::Asynchronous)
            $stream.Connect(2000);$partial.Add($stream)
            $header=[BitConverter]::GetBytes([uint32]128);$stream.Write($header,0,$header.Length);$stream.Flush()
        }
        Start-Sleep -Milliseconds 5500
        $pipeClock=[Diagnostics.Stopwatch]::StartNew();$afterPartial=Request @{action='Status'};$pipeClock.Stop()
        Assert ($afterPartial.ok -and $pipeClock.ElapsedMilliseconds -lt 1500 -and !$worker.HasExited) 'all four abandoned partial IPC frames expire and management becomes responsive again'
    } finally {foreach($stream in $partial){$stream.Dispose()}}

    # A same-user process can publish valid framed JSON under the expected
    # fixture pipe name. The client must reject its foreign process identity.
    $foreignRoot=Join-Path $fixture 'foreign-server-state'
    [IO.Directory]::CreateDirectory($foreignRoot)|Out-Null
    [IO.File]::WriteAllText((Join-Path $foreignRoot 'qa.marker'),'Foreign endpoint fixture only')
    $serverScript=Join-Path $fixture 'foreign-server.ps1';$serverReady=Join-Path $foreignRoot 'ready.marker'
    [IO.File]::WriteAllText($serverScript,@'
param([string]$PipeName,[string]$Ready)
$pipe=New-Object IO.Pipes.NamedPipeServerStream($PipeName,[IO.Pipes.PipeDirection]::InOut)
try {
    [IO.File]::WriteAllText($Ready,'Fixture server ready')
    $pipe.WaitForConnection()
    $bytes=[Text.Encoding]::UTF8.GetBytes('{"ok":true,"data":{"running":true}}')
    $header=[BitConverter]::GetBytes([uint32]$bytes.Length)
    $pipe.Write($header,0,$header.Length);$pipe.Write($bytes,0,$bytes.Length);$pipe.Flush()
    Start-Sleep -Seconds 2
} finally {$pipe.Dispose()}
'@,(New-Object Text.UTF8Encoding($false)))
    $foreign=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File "'+$serverScript+'" -PipeName '+[MukhomorFixturePipe]::Name($foreignRoot)+' -Ready "'+$serverReady+'"') -PassThru -WindowStyle Hidden
    $clients.Add(@{process=$foreign;output='';action='Foreign fixture server'})
    $readiness=[Diagnostics.Stopwatch]::StartNew()
    while($readiness.Elapsed.TotalSeconds -lt 60 -and !(Test-Path -LiteralPath $serverReady)){if($foreign.HasExited){throw 'Foreign fixture server exited before readiness'};Start-Sleep -Milliseconds 100}
    if(!(Test-Path -LiteralPath $serverReady)){throw 'Foreign fixture server did not start'}
    $foreignRequest=Join-Path $foreignRoot 'request.json';$foreignReply=Join-Path $foreignRoot 'reply.json'
    [IO.File]::WriteAllText($foreignRequest,'{"action":"Status"}',(New-Object Text.UTF8Encoding($false)))
    $probe=Start-Process -FilePath $Executable -ArgumentList ('--rpc --root "'+$foreignRoot+'" --request "'+$foreignRequest+'" --output "'+$foreignReply+'"') -PassThru -WindowStyle Hidden
    $null=$probe.Handle
    $clients.Add(@{process=$probe;output=$foreignReply;action='Foreign endpoint client'})
    if(!$probe.WaitForExit(5000)){throw 'Foreign endpoint client exceeded its deadline'}
    $foreignResult=[IO.File]::ReadAllText($foreignReply)|ConvertFrom-Json
    Assert ($probe.ExitCode -ne 0 -and !$foreignResult.ok -and $foreignResult.error -eq 'The IPC endpoint does not belong to the installed Mukhomor service') 'valid framed responses from a foreign pipe server are rejected before accepting state'
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(30000)) 'persistence-error fixture shuts down'

    [IO.File]::WriteAllText((Join-Path $root 'runtime\fail-init.marker'),'Synthetic failed startup helper')
    Start-Worker
    $failedInit=Request @{action='Status'}
    Assert ($failedInit.ok -and !$failedInit.data.busy -and !$failedInit.data.running -and $failedInit.data.status_uncertain -and $failedInit.data.last_error) 'Init failure leaves the controller reachable for repairs with a finite idle error'
    [IO.File]::Delete((Join-Path $root 'runtime\fail-init.marker'))
    Assert (Request @{action='Connect';fast=$true}).ok 'explicit Connect succeeds after repairing a failed Init'
    [IO.File]::WriteAllText((Join-Path $root 'runtime\fail-dns.marker'),'Synthetic failed Exit cleanup')
    $failedExit=Request @{action='Exit';request_id='failed-exit-fixture'}
    Assert (!$failedExit.ok -and !$failedExit.data.busy -and $failedExit.data.phase -eq 'recovery' -and !$worker.HasExited) 'failed Exit stays open for DNS recovery instead of claiming success'
    $exit=Request @{action='Exit';request_id='exit-fixture'}
    Assert ($exit.ok -and !$exit.data.running -and !$exit.data.busy -and $exit.data.shutting_down -and $exit.data.last_operation.request_id -eq 'exit-fixture') 'successful Exit returns a clean terminal receipt before stopping its controller'
    Assert ($worker.WaitForExit(5000) -and $worker.ExitCode -eq 0) 'authenticated Exit terminates the fixture controller gracefully without recovery restart'
    Start-Worker
    $lostReceipt=New-Object IO.Pipes.NamedPipeClientStream('.',([MukhomorFixturePipe]::Name($root)),[IO.Pipes.PipeDirection]::InOut,[IO.Pipes.PipeOptions]::Asynchronous)
    try {
        $lostReceipt.Connect(2000)
        $bytes=[Text.Encoding]::UTF8.GetBytes('{"action":"Exit","request_id":"unread-exit-receipt-fixture"}')
        $header=[BitConverter]::GetBytes([uint32]$bytes.Length)
        $lostReceipt.Write($header,0,$header.Length);$lostReceipt.Write($bytes,0,$bytes.Length);$lostReceipt.Flush()
        # Disappear before reading any response or acknowledgement.
    } finally {$lostReceipt.Dispose()}
    Assert ($worker.WaitForExit(5000) -and $worker.ExitCode -eq 0) 'lost Exit receipt still stops the owned controller gracefully within the reply grace period'
    $lostSaved=Get-Content -LiteralPath (Join-Path $root 'runtime\connection-intent.json') -Raw|ConvertFrom-Json
    $lostState=Get-Content -LiteralPath (Join-Path $root 'runtime\mock-state.json') -Raw|ConvertFrom-Json
    Assert (!$lostSaved.desired -and !$lostState.running -and !$lostState.recovery -and !(Test-Path -LiteralPath (Join-Path $root 'runtime\dns-backup.json')) -and !(Test-Path -LiteralPath (Join-Path $root 'runtime\session.json'))) 'lost receipt preserves disconnected intent and recovered runtime proof'
    Write-Host ('PASS: '+$passed+' disconnect / cancellation / recovery assertions; no TUN or system DNS touched')
} finally {
    foreach($client in $clients){if(!$client.process.HasExited){$client.process.Kill();$client.process.WaitForExit()}}
    if($worker -and !$worker.HasExited){$worker.Kill();$worker.WaitForExit()}
    $childRecord=Join-Path $root 'runtime\child-process.json'
    if(Test-Path -LiteralPath $childRecord){
        $info=Get-Content -LiteralPath $childRecord -Raw|ConvertFrom-Json
        $remaining=Get-Process -Id $info.pid -ErrorAction SilentlyContinue
        if($remaining -and $remaining.StartTime.ToUniversalTime().ToString('o') -eq $info.started){$remaining.Kill();$remaining.WaitForExit()}
    }
    $resolved=[IO.Path]::GetFullPath($fixture);$expected=([IO.Path]::GetFullPath($PSScriptRoot)+'\disconnect-')
    if($resolved.StartsWith($expected,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^disconnect-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
