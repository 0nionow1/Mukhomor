Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
$parseErrors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $package 'Uninstall.ps1'),[ref]$null,[ref]$parseErrors)
if($parseErrors.Count){throw 'Uninstaller could not be parsed'}
$functions=@{}
foreach($node in $ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)) {
    if($functions.ContainsKey($node.Name)){throw 'Duplicate uninstall helper'}
    $functions[$node.Name]=[scriptblock]::Create($node.Extent.Text)
}
$gates=@($ast.EndBlock.Statements | Where-Object {$_.Extent.Text -ceq 'Remove-VerifiedController $program $data'})
if($gates.Count -ne 1){throw 'Actual uninstall transaction gate was not found'}
$gate=[scriptblock]::Create($gates[0].Extent.Text)
# Extract actual helper code only. Never dot-source the privileged main script.
# All operating-system mutation commands in the extracted code are mocked.
$allowed=@('Get-RemovalField','Assert-RemovalBoolean','Assert-RemovalString','Assert-RemovalReceipt','Assert-RemovalPath','Read-RemovalJson','Get-RemovalVersionFiles','Get-RemovalVersionTree','Test-RemovalOwnedFile','Get-RemovalIdentity','Assert-RemovalService','Set-RemovalStartMode','Write-RemovalIntent','Invoke-RemovalExit','Remove-VerifiedController','Get-Item','Get-Acl','Get-Content','ConvertFrom-Json','ConvertTo-Json','Test-Path','Join-Path','Get-FileHash','New-Object','Get-Process','Get-CimInstance','Invoke-CimMethod','Get-Service','Start-Service','Start-Process','sc.exe','Out-Null')
foreach($node in $ast.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.FunctionDefinitionAst]}) {
    foreach($command in $node.FindAll({param($item) $item -is [Management.Automation.Language.CommandAst]},$true)) {
        if($command.GetCommandName() -notin $allowed){throw ('Unmocked uninstall command: '+$command.GetCommandName())}
    }
}
$fixture=Join-Path $PSScriptRoot ('uninstall-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$passed=0
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
$harness=New-Module -ArgumentList $functions -ScriptBlock {
    param($functions)
    foreach($name in $functions.Keys){. $functions[$name]}
    function script:Get-Item {
        param($LiteralPath,[switch]$Force)
        $item=Microsoft.PowerShell.Management\Get-Item -LiteralPath $LiteralPath -Force:$Force
        if($script:Model.scenario -eq 'reparse' -and $LiteralPath -eq $script:Model.directory){return [pscustomobject]@{Attributes=[IO.FileAttributes]::ReparsePoint;PSIsContainer=$true;Length=0}}
        if($script:Model.scenario -eq 'child_reparse' -and $LiteralPath -eq (Join-Path $script:Model.directory 'bin')){return [pscustomobject]@{Attributes=[IO.FileAttributes]::ReparsePoint;PSIsContainer=$true;Length=0}}
        if($script:Model.scenario -eq 'child_reparse' -and $LiteralPath.StartsWith((Join-Path $script:Model.directory 'bin')+'\',[StringComparison]::OrdinalIgnoreCase)){$script:Model.unsafeTraversal=$true;throw 'Ancestor-first verification was bypassed'}
        $item
    }
    function script:Get-Acl {
        param($LiteralPath)
        $acl=[pscustomobject]@{}
        $acl|Add-Member ScriptMethod GetOwner {param($type) [pscustomobject]@{Value=$(if($script:Model.scenario -eq 'untrusted_owner'){'S-1-5-32-545'}else{'S-1-5-32-544'})}}
        $acl|Add-Member ScriptMethod GetAccessRules {
            param($explicit,$inherited,$type)
            [pscustomobject]@{AccessControlType='Allow';FileSystemRights=[Security.AccessControl.FileSystemRights]::FullControl;IdentityReference=[pscustomobject]@{Value=$(if($script:Model.scenario -eq 'untrusted_write'){'S-1-5-32-545'}else{'S-1-5-18'})}}
        }
        $acl
    }
    function script:Get-CimInstance {
        param($ClassName,$Filter)
        if($ClassName -ne 'Win32_Service' -or $Filter -cne "Name='Mukhomor'"){throw 'Unexpected CIM query'}
        if($script:Model.scenario -eq 'missing_service'){return $null}
        [pscustomobject]@{Name='Mukhomor';PathName=$script:Model.command;StartName=$script:Model.account;ServiceType=$script:Model.type;StartMode=$script:Model.mode;State=$script:Model.service.Status;ProcessId=$(if($script:Model.service.Status -eq 'Stopped' -and $script:Model.scenario -ne 'pid_after_exit'){0}else{101})}
    }
    function script:Get-Process {
        param($Id,$ErrorAction)
        if($Id -ne 101){throw 'Unexpected process query'}
        [pscustomobject]@{Path=$(if($script:Model.scenario -eq 'foreign_pid'){Join-Path $script:Model.data 'foreign.exe'}else{$script:Model.executable})}
    }
    function script:Invoke-CimMethod {
        param($InputObject,$MethodName,$Arguments)
        if($MethodName -ne 'Change' -or $Arguments.Keys.Count -ne 1 -or !$Arguments.ContainsKey('StartMode')){throw 'Unexpected CIM mutation'}
        $script:Model.events.Add('mode:'+ $Arguments.StartMode)
        if($script:Model.scenario -eq 'disable_failure' -and $Arguments.StartMode -eq 'Disabled'){return @{ReturnValue=5}}
        $script:Model.mode=if($Arguments.StartMode -eq 'Automatic'){'Auto'}else{$Arguments.StartMode}
        @{ReturnValue=0}
    }
    function script:Get-Service {param($Name,$ErrorAction) if($Name -cne 'Mukhomor'){throw 'Unexpected service'};$script:Model.service}
    function script:Start-Service {
        param($Name)
        if($Name -cne 'Mukhomor' -or $script:Model.mode -eq 'Disabled'){throw 'Unexpected start'}
        $intent=Read-RemovalJson (Join-Path $script:Model.data 'runtime\connection-intent.json')
        if($intent.desired -isnot [bool] -or $intent.desired -ne $false){throw 'Stopped controller was started with reconnect intent'}
        $script:Model.events.Add('start')
        $script:Model.service.Status='Running'
    }
    function script:sc.exe {
        param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Arguments)
        if($Arguments.Count -ne 2 -or $Arguments[0] -cne 'delete' -or $Arguments[1] -cne 'Mukhomor'){throw 'Unexpected SCM command'}
        if(!$script:Model.receiptVerified -or $script:Model.service.Status -ne 'Stopped' -or $script:Model.mode -ne 'Disabled'){throw 'Deletion bypassed verified Exit/stop'}
        $script:Model.events.Add('delete')
        $script:LASTEXITCODE=if($script:Model.scenario -eq 'delete_failure'){5}else{0}
        if($script:LASTEXITCODE -eq 0){$script:Model.deleted=$true}
    }
    function New-FixtureReceipt([string]$Id) {
        @{ok=$true;request_id=$Id;operation_id=7;data=@{running=$false;starting=$false;busy=$false;shutting_down=$true;last_operation=@{id=7;ok=$true;action='Exit';request_id=$Id};traffic_guard=@{armed=$false;verified=$true;configured=$false;protected=$false}}}
    }
    function script:Start-Process {
        param($FilePath,$ArgumentList,$WindowStyle,[switch]$PassThru)
        if($FilePath -cne $script:Model.executable -or $WindowStyle -cne 'Hidden' -or !$PassThru -or $ArgumentList.Count -ne 7 -or $ArgumentList[0] -cne '--rpc'){throw 'Unexpected executable launch'}
        if($script:Model.mode -ne 'Disabled'){throw 'RPC executed before restart was suspended'}
        $requestPath=$ArgumentList[4].Trim('"');$outputPath=$ArgumentList[6].Trim('"')
        if([IO.Path]::GetDirectoryName($requestPath) -ne (Join-Path $script:Model.data 'private') -or [IO.Path]::GetDirectoryName($outputPath) -ne (Join-Path $script:Model.data 'private')){throw 'RPC files escaped private fixture'}
        $request=Read-RemovalJson $requestPath
        if($request.action -cne 'Exit' -or $request.request_id -notmatch '^uninstall-[a-f0-9]{32}$'){throw 'Unexpected RPC request'}
        $script:Model.events.Add('rpc')
        $reply=New-FixtureReceipt $request.request_id
        switch($script:Model.scenario) {
            'armed' {$reply.data.traffic_guard.armed=$true}
            'unverified' {$reply.data.traffic_guard.verified=$false}
            'verified_string' {$reply.data.traffic_guard.verified='true'}
            'string_boolean' {$reply.data.traffic_guard.armed='false'}
            'legacy' {$reply.data.Remove('traffic_guard')}
            'wrong_request' {$reply.request_id='another-request'}
            'array_request' {$reply.request_id=@($request.request_id,$request.request_id)}
            'wrong_terminal' {$reply.data.last_operation.action='Disconnect'}
            'wrong_operation' {$reply.data.last_operation.id=6}
            'string_operation' {$reply.data.last_operation.id='7'}
            'active' {$reply.data.running=$true}
            'not_shutdown' {$reply.data.shutting_down=$false}
            'failed_operation' {$reply.ok=$false}
        }
        if($script:Model.scenario -ne 'lost_receipt'){[IO.File]::WriteAllText($outputPath,($reply|ConvertTo-Json -Depth 10))}
        try {Assert-RemovalReceipt $reply $request.request_id;$script:Model.receiptVerified=$true} catch {}
        if($script:Model.scenario -notin @('server_not_stopped','rpc_timeout')){$script:Model.service.Status='Stopped'}
        if($script:Model.scenario -eq 'foreign_after_exit'){$script:Model.command='"foreign.exe"'}
        if($script:Model.scenario -eq 'metadata_after_exit'){[IO.File]::AppendAllText($script:Model.recordPath,' ')}
        $process=[pscustomobject]@{Handle=1;ExitCode=$(if($script:Model.scenario -eq 'rpc_error'){1}elseif($script:Model.scenario -eq 'null_exitcode'){$null}else{0});HasExited=($script:Model.scenario -ne 'rpc_timeout')}
        $process|Add-Member ScriptMethod WaitForExit {param($milliseconds) $this.HasExited}
        $process|Add-Member ScriptMethod Refresh {}
        $process|Add-Member ScriptMethod Kill {$script:Model.events.Add('kill-client');$this.HasExited=$true}
        $process|Add-Member ScriptMethod Dispose {$script:Model.events.Add('dispose-client')}
        $process
    }
    function Invoke-UninstallScenario($Gate,[string]$Fixture,[string]$Scenario,[string]$InitialState='Running',[string]$InitialMode='Auto') {
        $root=Join-Path $Fixture $Scenario
        $program=Join-Path $root 'program';$data=Join-Path $root 'data'
        $directory=Join-Path $program ('app-1.2.3-'+[guid]::NewGuid().ToString('N'))
        foreach($path in @($directory,(Join-Path $directory 'bin'),(Join-Path $directory 'assets'),(Join-Path $data 'private'),(Join-Path $data 'runtime'))){[IO.Directory]::CreateDirectory($path)|Out-Null}
        $files=@()
        foreach($relative in @('mukhomor.exe','NativeBridge.ps1','SplitVpn.psm1','SplitWindows.psm1','TrafficGuard.psm1','ProfileImport.psm1','bin\mihomo-windows-amd64-compatible.exe','bin\wintun.dll','assets\settings.default.json')) {
            $path=Join-Path $directory $relative
            [IO.File]::WriteAllText($path,'Inert uninstall fixture, never executed: '+$relative)
            $files+=@{path=$relative;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
        }
        $executable=Join-Path $directory 'mukhomor.exe'
        $recordPath=Join-Path $data 'installation.json'
        [IO.File]::WriteAllText($recordPath,(@{version='1.2.3';executable=$executable}|ConvertTo-Json))
        if($Scenario -eq 'manifest_escape'){$files+=@{path='..\..\outside.exe';sha256=('0'*64)}}
        if($Scenario -eq 'manifest_duplicate'){$files+=@{path='BIN\..\mukhomor.exe';sha256=$files[0].sha256}}
        [IO.File]::WriteAllText((Join-Path $directory 'release-manifest.json'),(@{version='1.2.3';files=$files}|ConvertTo-Json -Depth 8))
        [IO.File]::WriteAllText((Join-Path $data 'runtime\connection-intent.json'),(@{schema=1;generation=41;desired=$true}|ConvertTo-Json))
        if($Scenario -eq 'hash_mismatch'){[IO.File]::AppendAllText($executable,'changed')}
        if($Scenario -eq 'qa_worker'){[IO.File]::WriteAllText((Join-Path $data 'qa.marker'),'fixture')}
        $script:Model=[pscustomobject]@{scenario=$Scenario;data=$data;directory=$directory;executable=$executable;recordPath=$recordPath;command='"'+$executable+'" --service --root "'+$data+'"';account='LocalSystem';type='Own Process';mode=$InitialMode;service=[pscustomobject]@{Status=$InitialState};events=[Collections.Generic.List[string]]::new();deleted=$false;receiptVerified=$false;unsafeTraversal=$false}
        if($Scenario -eq 'foreign_path'){$script:Model.command='"foreign.exe"'}
        if($Scenario -eq 'foreign_account'){$script:Model.account='fixture-user'}
        if($Scenario -eq 'shared_service'){$script:Model.type='Share Process'}
        $script:Model.service|Add-Member ScriptMethod WaitForStatus {param($state,$timeout) if($this.Status -ne $state){throw 'Fixture service wait expired'}}
        $script:LASTEXITCODE=0
        $failure='';$downstream=$false
        try {
            # Actual source-level gate; downstream deletion is represented only
            # by a counter so these tests cannot remove a real installation.
            . $Gate
            $downstream=$true
        } catch {$failure=$_.Exception.Message}
        [pscustomobject]@{model=$script:Model;failure=$failure;downstream=$downstream;filesPreserved=[IO.File]::Exists($executable);privateFiles=@([IO.Directory]::GetFiles((Join-Path $data 'private'))).Count;intent=(Read-RemovalJson (Join-Path $data 'runtime\connection-intent.json'))}
    }
    function Invoke-VersionTreeScenario($Gate,[string]$Fixture,[string]$Scenario) {
        $fixtureResult=Invoke-UninstallScenario $Gate $Fixture $Scenario
        $program=[IO.Path]::GetDirectoryName($script:Model.directory)
        switch($Scenario) {
            'unknown_file' {[IO.File]::WriteAllText((Join-Path $script:Model.directory 'unlisted.txt'),'Preserve this unknown file')}
            'unknown_directory' {[IO.Directory]::CreateDirectory((Join-Path $script:Model.directory 'unlisted'))|Out-Null}
            'changed_old_file' {[IO.File]::AppendAllText($script:Model.executable,'changed')}
            'child_reparse' {}
        }
        $failed=$false;$items=$null
        try {$tree=Get-RemovalVersionTree $program $script:Model.directory;$items=$tree.items} catch {$failed=$true}
        [pscustomobject]@{failed=$failed;count=$(if($items){$items.Count}else{0});filesPreserved=[IO.File]::Exists($script:Model.executable);unsafeTraversal=$script:Model.unsafeTraversal;ownedExe=(Test-RemovalOwnedFile $script:Model.executable $program);ownedCore=(Test-RemovalOwnedFile (Join-Path $script:Model.directory 'bin\mihomo-windows-amd64-compatible.exe') $program);foreign=(Test-RemovalOwnedFile (Join-Path $script:Model.data 'foreign.exe') $program);unlisted=(Test-RemovalOwnedFile (Join-Path $script:Model.directory 'unlisted.txt') $program)}
    }
}
try {
    # Compile the actual scheduling wrapper with only its kernel import replaced
    # by a pure string/flag inspector. Never call a Windows deletion API.
    $interop=@($ast.FindAll({param($node) $node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value -match 'public static class MukhomorRemoval'},$true))
    if($interop.Count -ne 1){throw 'Removal scheduling wrapper was not found'}
    $import='(?s)\[DllImport\("kernel32\.dll", CharSet=CharSet\.Unicode, SetLastError=true\)\]\s*public static extern bool MoveFileEx\(string existing, string target, uint flags\);'
    if([regex]::Matches($interop[0].Value,$import).Count -ne 1){throw 'Removal scheduling import cannot be safely mocked'}
    $testClass='MukhomorRemovalFixture'+[guid]::NewGuid().ToString('N')
    $stub=[regex]::Replace($interop[0].Value,$import,'public static bool MoveFileEx(string existing, string target, uint flags) { return existing == "inert" && target == null && flags == 4; }').Replace('MukhomorRemoval',$testClass)
    Add-Type -TypeDefinition $stub
    $fixtureType=$testClass -as [type]
    Assert (!$fixtureType::MoveFileEx('inert',$null,4)) 'negative control confirms PowerShell string null coercion'
    Assert ($fixtureType::ScheduleDelete('inert')) 'actual C# wrapper preserves true native null for delayed deletion'
    Assert ($ast.Extent.Text -notmatch '\[MukhomorRemoval\]::MoveFileEx\(') 'main deletion never sends PowerShell null to the native import'
    # The gate precedes every top-level stop/removal command. Function bodies
    # are excluded because their privileged calls are isolated above.
    $gateOffset=$gates[0].Extent.StartOffset
    foreach($statement in $ast.EndBlock.Statements | Where-Object {$_ -isnot [Management.Automation.Language.FunctionDefinitionAst]}) {
        foreach($command in $statement.FindAll({param($node) $node -is [Management.Automation.Language.CommandAst]},$true)) {
            if($command.GetCommandName() -in @('Stop-Service','Stop-Process','Remove-Item','Remove-NetFirewallRule','sc.exe')){Assert ($command.Extent.StartOffset -gt $gateOffset) 'no destructive main command can precede the gate'}
        }
    }
    foreach($case in @(@{name='running'},@{name='stopped';state='Stopped'},@{name='disabled';state='Stopped';mode='Disabled'})) {
        $state=if($case.ContainsKey('state')){$case.state}else{'Running'};$mode=if($case.ContainsKey('mode')){$case.mode}else{'Auto'}
        $result=& $harness {param($g,$f,$n,$s,$m) Invoke-UninstallScenario $g $f $n $s $m} $gate $fixture $case.name $state $mode
        Assert (!$result.failure -and $result.downstream -and $result.model.deleted) ($case.name+' verifies Exit before deletion: '+$result.failure)
        Assert ($result.model.events.IndexOf('mode:Disabled') -lt $result.model.events.IndexOf('rpc') -and $result.model.events.IndexOf('rpc') -lt $result.model.events.IndexOf('delete')) 'restart is suspended before Exit and delete'
        Assert ($result.filesPreserved -and $result.privateFiles -eq 0) 'inert fixture files are preserved and RPC files cleaned'
        if($state -eq 'Stopped'){Assert ($result.intent.desired -is [bool] -and !$result.intent.desired -and $result.intent.generation -eq 42 -and $result.model.events.Contains('start')) 'stopped own service starts only with monotonic disconnect intent'}
    }
    foreach($scenario in @('foreign_path','foreign_account','shared_service','foreign_pid','missing_service','hash_mismatch','manifest_escape','manifest_duplicate','untrusted_owner','untrusted_write','reparse','qa_worker')) {
        $result=& $harness {param($g,$f,$n) Invoke-UninstallScenario $g $f $n} $gate $fixture $scenario
        Assert ($result.failure -and !$result.downstream -and !$result.model.deleted -and $result.filesPreserved) ($scenario+' refuses deletion')
        Assert ($result.model.events.Count -eq 0) ($scenario+' refuses before any service mutation or RPC')
    }
    foreach($scenario in @('armed','unverified','verified_string','string_boolean','legacy','wrong_request','array_request','wrong_terminal','wrong_operation','string_operation','active','not_shutdown','failed_operation','lost_receipt','rpc_error','null_exitcode','rpc_timeout','server_not_stopped','pid_after_exit','foreign_after_exit','metadata_after_exit','disable_failure','delete_failure')) {
        $result=& $harness {param($g,$f,$n) Invoke-UninstallScenario $g $f $n} $gate $fixture $scenario
        Assert ($result.failure -and !$result.downstream -and !$result.model.deleted -and $result.filesPreserved) ($scenario+' preserves application on failed proof')
        Assert ($result.privateFiles -eq 0) ($scenario+' cleans only fresh owned RPC files')
        if($scenario -ne 'foreign_after_exit'){Assert ($result.model.mode -eq 'Auto') ($scenario+' restores exact owned initial startup mode')}
        else{Assert ($result.model.mode -eq 'Disabled') 'foreign changed service is never mutated during restoration'}
        if($scenario -eq 'rpc_timeout'){Assert ($result.model.events.Contains('kill-client') -and !$result.model.events.Contains('delete')) 'timeout kills only the owned RPC client'}
    }
    # Repeat the failed receipt in a separate cold-start fixture to prove that
    # reconnect intent is never restored after a failure.
    $failedFixture=Join-Path $fixture 'repeat';[IO.Directory]::CreateDirectory($failedFixture)|Out-Null
    $result=& $harness {param($g,$f) Invoke-UninstallScenario $g $f 'lost_receipt' 'Stopped'} $gate $failedFixture
    Assert ($result.failure -and !$result.downstream -and !$result.intent.desired -and $result.intent.generation -eq 42) 'lost receipt after cold start preserves explicit disconnect intent'
    foreach($scenario in @('tree_valid','unknown_file','unknown_directory','changed_old_file','child_reparse')) {
        $result=& $harness {param($g,$f,$n) Invoke-VersionTreeScenario $g $f $n} $gate $fixture $scenario
        Assert ($result.filesPreserved) ($scenario+' preflight never deletes files')
        if($scenario -eq 'tree_valid'){
            Assert (!$result.failed -and $result.count -eq 12) 'BFS validates exact manifest files and their two directories'
            Assert ($result.ownedExe -and $result.ownedCore -and !$result.foreign -and !$result.unlisted) 'cleanup ownership requires manifest membership for GUI, shortcuts, registry and firewall'
        }
        else{Assert ($result.failed) ($scenario+' refuses version deletion before traversal or mutation')}
        Assert (!$result.unsafeTraversal) ($scenario+' never looks through an unverified ancestor')
    }
    Write-Host ('Uninstall isolation checks passed: '+$passed+'; no real service, WFP, DNS, route, or installed application was changed.')
} finally {
    if($harness){Remove-Module $harness -Force}
    $resolved=[IO.Path]::GetFullPath($fixture)
    if([IO.Path]::GetDirectoryName($resolved) -ne [IO.Path]::GetFullPath($PSScriptRoot) -or [IO.Path]::GetFileName($resolved) -notmatch '^uninstall-[a-f0-9]{32}$'){throw 'Uninstall fixture cleanup escaped the test directory'}
    foreach($item in @(Get-ChildItem -LiteralPath $resolved -Recurse -Force)) {if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Uninstall fixture cleanup found a reparse point'}}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
