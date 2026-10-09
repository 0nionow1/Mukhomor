Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $package 'Install.ps1'),[ref]$null,[ref]$errors)
if($errors.Count){throw 'Installer could not be parsed'}
$functions=@{}
foreach($name in @('Write-ManagedBytes','Set-AppEntries','Give-OwnerStartAccess')) {
    $found=@($ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true) | Where-Object Name -eq $name)
    if($found.Count -ne 1){throw ('Missing installer helper: '+$name)}
    $functions[$name]=[scriptblock]::Create($found[0].Extent.Text)
}
# Execute the actual transaction, rather than a second implementation. Exclude
# package installation/admin preflight and GUI handoff: every privileged action
# in this block is intercepted by a mock inside an isolated module.
$transactions=@($ast.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.TryStatementAst] -and $_.Extent.Text -match '\$upgradeCommitted=\$true'})
if($transactions.Count -ne 1){throw 'Installer transaction was not found'}
$transaction=[scriptblock]::Create($transactions[0].Extent.Text)
$allowed=@('Get-CimInstance','Get-Content','Invoke-CimMethod','sc.exe','Get-Process','Stop-Process','Join-Path','New-Service','Write-ManagedBytes','ConvertTo-Json','ConvertFrom-Json','Set-AppEntries','Give-OwnerStartAccess','Start-Service','Get-Service','Out-Null')
foreach($command in $transactions[0].FindAll({param($node) $node -is [Management.Automation.Language.CommandAst]},$true)) {
    if($command.GetCommandName() -notin $allowed){throw ('Unmocked installer command: '+$command.GetCommandName())}
}
$fixture=Join-Path $PSScriptRoot ('upgrade-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$passed=0
function Assert($Condition,[string]$Message) {if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
$harness=New-Module -ArgumentList $functions -ScriptBlock {
    param($functions)
    foreach($name in $functions.Keys){. $functions[$name]}
    function script:Get-CimInstance {
        param($ClassName,$Filter)
        if($ClassName -ne 'Win32_Service' -or $Filter -ne "Name='Mukhomor'"){throw 'Unexpected CIM query'}
        [pscustomobject]@{PathName=$script:Model.path;ProcessId=101;StartMode=$script:Model.mode}
    }
    function script:Invoke-CimMethod {
        param($InputObject,$MethodName,$Arguments)
        if($MethodName -ne 'Change'){throw 'Unexpected CIM method'}
        if($Arguments.ContainsKey('PathName')) {
            if($script:Model.mode -ne 'Disabled'){throw 'Controller path changed while recovery was enabled'}
            if($script:Model.scenario -eq 'rollback_failure' -and $Arguments.PathName -eq $script:Model.oldBinary){return @{ReturnValue=5}}
            $script:Model.path=$Arguments.PathName;$script:Model.events.Add('path')
        }
        if($Arguments.ContainsKey('StartMode')){$script:Model.mode=$Arguments.StartMode;$script:Model.events.Add('mode:'+ $Arguments.StartMode)}
        @{ReturnValue=0}
    }
    function script:Get-Service {param($Name,$ErrorAction) if($Name -ne 'Mukhomor'){throw 'Unexpected service'};$script:Model.service}
    function script:sc.exe {
        param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Arguments)
        if($Arguments.Count -lt 2 -or $Arguments[1] -ne 'Mukhomor'){throw 'Unexpected SCM command'}
        $script:LASTEXITCODE=0
        switch($Arguments[0]) {
            'stop' {
                if($script:Model.mode -ne 'Disabled'){throw 'Recovery was not disabled before stop'}
                $script:Model.events.Add('stop')
                $script:Model.service.Status=if($script:Model.scenario -in @('forced','foreign')){'StopPending'}else{'Stopped'}
            }
            'config' {
                $record=Get-Content -LiteralPath $script:Model.recordPath -Raw -Encoding UTF8|ConvertFrom-Json
                $expected='"'+$record.executable+'" --service --root "'+$script:Model.data+'"'
                if($expected -ne $script:Model.path){throw 'Recovery enabled before records matched the controller'}
                $script:Model.mode='Automatic';$script:Model.events.Add('enable')
                if($script:Model.scenario -eq 'queued_restart'){$script:Model.service.Status='Running'}
            }
            'failure' {$script:Model.events.Add('failure')}
            'failureflag' {$script:Model.events.Add('failureflag')}
            'delete' {$script:Model.deleted=$true;$script:Model.events.Add('delete')}
            'sdshow' {$script:Model.sddl}
            'sdset' {$script:Model.sddl=$Arguments[2];$script:Model.events.Add('sdset')}
            default {throw 'Unexpected SCM operation'}
        }
    }
    function script:Get-Process {param($Id,$ErrorAction) if($Id -ne 101){throw 'Unexpected process query'};[pscustomobject]@{Path=$(if($script:Model.scenario -eq 'foreign'){Join-Path $script:Model.data 'foreign.exe'}else{$script:Model.oldExecutable})}}
    function script:Stop-Process {param($Id,[switch]$Force) if($Id -ne 101 -or !$Force){throw 'Unexpected process stop'};$script:Model.events.Add('force');$script:Model.service.Status='Stopped'}
    function script:New-Service {
        param($Name,$DisplayName,$BinaryPathName,$StartupType,$Description)
        if($Name -ne 'Mukhomor' -or $StartupType -ne 'Disabled'){throw 'Fresh controller must be created disabled'}
        $script:Model.path=$BinaryPathName;$script:Model.mode='Disabled';$script:Model.events.Add('create')
    }
    function script:Start-Service {
        param($Name)
        if($Name -ne 'Mukhomor' -or $script:Model.mode -ne 'Automatic'){throw 'Unexpected controller start'}
        $script:Model.events.Add('start')
        if($script:Model.scenario -eq 'start_failure'){throw 'Fixture start failed'}
        if($script:Model.scenario -eq 'queued_restart'){throw 'Fixture service already running'}
        $script:Model.service.Status='Running'
    }
    function script:New-Object {
        param($TypeName,$ArgumentList,$ComObject)
        if(!$ComObject) {
            if($TypeName -notin @('Security.AccessControl.RawSecurityDescriptor','Security.Principal.SecurityIdentifier','Security.AccessControl.CommonAce')){throw 'Unexpected constructor'}
            return Microsoft.PowerShell.Utility\New-Object -TypeName $TypeName -ArgumentList $ArgumentList
        }
        if($ComObject -ne 'WScript.Shell'){throw 'Unexpected COM object'}
        $shell=[pscustomobject]@{}
        $shell|Add-Member ScriptMethod CreateShortcut {
            param($path)
            $link=[pscustomobject]@{Path=$path;TargetPath='';WorkingDirectory='';Description='';IconLocation=''}
            $link|Add-Member ScriptMethod Save {$script:Model.shortcuts[$this.Path]=$this.TargetPath;$script:Model.events.Add('shortcut')}
            return $link
        }
        return $shell
    }
    function script:New-Item {param($Path,[switch]$Force) if($Path -ne 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Mukhomor'){throw 'Unexpected registry path'}}
    function script:New-ItemProperty {
        param($LiteralPath,$Name,$Value,$PropertyType,[switch]$Force)
        if($LiteralPath -ne 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Mukhomor'){throw 'Unexpected registry write'}
        $script:Model.properties[$Name]=$Value
        if($Name -eq 'DisplayVersion' -and $Value -eq '1.2.3' -and !$script:Model.faultUsed -and $script:Model.scenario -in @('entry_failure','rollback_failure','fresh_failure')){
            $script:Model.faultUsed=$true;throw 'Fixture application entry failed'
        }
    }
    function Invoke-UpgradeScenario {
        param($transaction,$fixture,$scenario)
        $program=Join-Path $fixture ('program-'+$scenario)
        $data=Join-Path $fixture ('data-'+$scenario)
        $version=Join-Path $program 'app-1.2.3-new'
        $oldDirectory=Join-Path $program 'app-1.1.0-old'
        foreach($path in @($version,$oldDirectory,$data)){[IO.Directory]::CreateDirectory($path)|Out-Null}
        $oldExecutable=Join-Path $oldDirectory 'mukhomor.exe'
        foreach($path in @($oldExecutable,(Join-Path $version 'mukhomor.exe'))){[IO.File]::WriteAllText($path,'Fixture, never executed')}
        $oldBinary='"'+$oldExecutable+'" --service --root "'+$data+'"'
        $recordPath=Join-Path $data 'installation.json'
        [IO.File]::WriteAllText($recordPath,(@{version='1.1.0';executable=$oldExecutable}|ConvertTo-Json))
        $script:Model=[pscustomobject]@{scenario=$scenario;path=$oldBinary;oldBinary=$oldBinary;oldExecutable=$oldExecutable;mode='Automatic';data=$data;recordPath=$recordPath;events=[Collections.Generic.List[string]]::new();shortcuts=@{};properties=@{};faultUsed=$false;deleted=$false;sddl='D:P(A;;GA;;;SY)(A;;GA;;;BA)';service=[pscustomobject]@{Status='Running'}}
        $script:Model.service|Add-Member ScriptMethod WaitForStatus {param($status,$timeout);if($this.Status -ne $status){throw 'Fixture stop timed out'}}
        $manifest=@{version='1.2.3'};$OwnerSid='S-1-5-21-111-222-333-1001'
        $service=if($scenario -in @('fresh','fresh_failure')){$null}else{$script:Model.service}
        $oldExecutable=$null;$oldBinary=$null;$upgradeCommitted=$false;$serviceDisabled=$false;$createdService=$false;$previousRecord=$null;$oldRunning=$false
        $script:LASTEXITCODE=0;$failure=''
        try {. $transaction} catch {$failure=$_.Exception.Message}
        $record=Get-Content -LiteralPath $recordPath -Raw -Encoding UTF8|ConvertFrom-Json
        return [pscustomobject]@{model=$script:Model;failure=$failure;record=$record;committed=$upgradeCommitted;newExecutable=(Join-Path $version 'mukhomor.exe')}
    }
}
try {
    $atomic=Join-Path $fixture 'atomic.json'
    & $harness {param($path) Write-ManagedBytes $path ([Text.Encoding]::UTF8.GetBytes('old'))} $atomic
    Assert ([IO.File]::ReadAllText($atomic) -eq 'old') 'atomic writer creates a new record'
    & $harness {param($path) Write-ManagedBytes $path ([Text.Encoding]::UTF8.GetBytes('new'))} $atomic
    Assert ([IO.File]::ReadAllText($atomic) -eq 'new') 'atomic writer replaces an existing record'
    $lock=[IO.File]::Open($atomic,'Open','ReadWrite','None');$failed=$false
    try {& $harness {param($path) Write-ManagedBytes $path ([Text.Encoding]::UTF8.GetBytes('broken'))} $atomic} catch {$failed=$true} finally {$lock.Dispose()}
    $leftovers=@(Get-ChildItem -LiteralPath $fixture|Where-Object {$_.Name.StartsWith('atomic.json.',[StringComparison]::Ordinal)})
    Assert ($failed -and [IO.File]::ReadAllText($atomic) -eq 'new' -and $leftovers.Count -eq 0) 'failed atomic replacement preserves the record and removes temporary files and backups'
    foreach($scenario in @('healthy','forced','foreign','entry_failure','rollback_failure','fresh','fresh_failure','queued_restart','start_failure')) {
        $result=& $harness {param($transaction,$fixture,$scenario) Invoke-UpgradeScenario $transaction $fixture $scenario} $transaction $fixture $scenario
        $model=$result.model
        if($scenario -in @('healthy','forced','fresh','queued_restart')) {
            Assert (!$result.failure -and $result.committed -and $result.record.executable -eq $result.newExecutable -and $model.service.Status -eq 'Running') ($scenario+': complete verified upgrade starts the new controller')
            Assert ($model.shortcuts.Count -eq 2 -and @($model.shortcuts.Values|Where-Object {$_ -ne $result.newExecutable}).Count -eq 0 -and $model.properties.DisplayVersion -eq '1.2.3') ($scenario+': shortcuts and uninstall entries target the new release')
        }
        if($scenario -eq 'healthy'){Assert ($model.events.IndexOf('mode:Disabled') -lt $model.events.IndexOf('stop') -and $model.events.IndexOf('path') -lt $model.events.IndexOf('enable')) 'recovery is disabled throughout stop, path change and record commit'}
        if($scenario -eq 'forced'){Assert ($model.events -contains 'force') 'stalled verified controller is force-stopped only after graceful stop timed out'}
        if($scenario -eq 'foreign'){Assert ($result.failure -match 'safely identified' -and $model.events -notcontains 'force' -and $result.record.version -eq '1.1.0' -and $model.path -eq $model.oldBinary) 'foreign process image is never killed or replaced'}
        if($scenario -eq 'entry_failure') {
            Assert ($result.failure -match 'application entry' -and !$result.committed -and $result.record.version -eq '1.1.0' -and $model.path -eq $model.oldBinary -and $model.service.Status -eq 'Running') 'failed pre-commit upgrade restores the old controller and installation record'
            Assert ($model.properties.DisplayVersion -eq '1.1.0' -and @($model.shortcuts.Values|Where-Object {$_ -ne $model.oldExecutable}).Count -eq 0) 'rollback restores shortcuts and uninstall metadata together'
        }
        if($scenario -eq 'rollback_failure'){Assert ($result.failure -match 'remains disabled' -and $model.mode -eq 'Disabled' -and $model.events -notcontains 'enable') 'failed rollback leaves recovery disabled rather than launching mismatched state'}
        if($scenario -eq 'fresh_failure'){Assert ($result.failure -match 'application entry' -and $model.deleted -and $model.events -notcontains 'enable') 'failed first installation removes its disabled new controller'}
        if($scenario -eq 'start_failure'){Assert ($result.failure -match 'start failed' -and $result.committed -and $result.record.version -eq '1.2.3' -and $model.path -ne $model.oldBinary) 'post-commit startup failure retains the complete new installation for repair'}
    }
    $aclResult=& $harness {
        $ownerSid='S-1-5-21-111-222-333-1001';$otherSid='S-1-5-21-111-222-333-1002'
        $script:Model.sddl='D:P(D;;WP;;;'+$otherSid+')(A;;GA;;;SY)(A;;GA;;;BA)'
        $before=[Security.AccessControl.RawSecurityDescriptor]::new($script:Model.sddl)
        Give-OwnerStartAccess $ownerSid
        $after=[Security.AccessControl.RawSecurityDescriptor]::new($script:Model.sddl)
        $owner=@($after.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier.Value -eq $ownerSid})
        $other=@($after.DiscretionaryAcl|Where-Object {$_.SecurityIdentifier.Value -ne $ownerSid})
        $original=@($before.DiscretionaryAcl)
        $preserved=$original.Count -eq $other.Count
        for($i=0;$i -lt $original.Count;$i++) {
            $preserved=$preserved -and $original[$i].SecurityIdentifier -eq $other[$i].SecurityIdentifier -and $original[$i].AccessMask -eq $other[$i].AccessMask -and $original[$i].AceFlags -eq $other[$i].AceFlags -and $original[$i].AceQualifier -eq $other[$i].AceQualifier
        }
        $beforeSets=@($script:Model.events|Where-Object {$_ -eq 'sdset'}).Count
        Give-OwnerStartAccess $ownerSid
        $afterSets=@($script:Model.events|Where-Object {$_ -eq 'sdset'}).Count
        [pscustomobject]@{owner=$owner;preserved=($preserved -and $after.ControlFlags -eq $before.ControlFlags);denyFirst=($after.DiscretionaryAcl[0].AceQualifier -eq [Security.AccessControl.AceQualifier]::AccessDenied);idempotent=($beforeSets -eq $afterSets)}
    }
    Assert ($aclResult.owner.Count -eq 1 -and $aclResult.owner[0].AccessMask -eq 21 -and $aclResult.owner[0].AceQualifier -eq [Security.AccessControl.AceQualifier]::AccessAllowed) 'owner receives only query-config, query-status and start rights'
    Assert ($aclResult.preserved -and $aclResult.denyFirst) 'service ACL retains existing deny, SYSTEM/admin permissions and protection flags'
    Assert $aclResult.idempotent 'repeated install does not append duplicate owner permissions'
    # Exercise the real retirement helper against two hidden windows owned by
    # this test process. Every other application's PID is excluded explicitly.
    $handoff=@($ast.FindAll({param($node) $node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value -match 'public static class MukhomorUpgrade'},$true))
    if($handoff.Count -ne 1){throw 'Installer retirement helper was not found'}
    Add-Type -TypeDefinition $handoff[0].Value
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class MukhomorUpgradeFixture {
 [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate IntPtr Procedure(IntPtr window,uint message,UIntPtr wParam,IntPtr lParam);
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Class {
  public uint style; public IntPtr procedure; public int classExtra,windowExtra; public IntPtr instance,icon,cursor,brush; public string menu,name;
 }
 [StructLayout(LayoutKind.Sequential)] struct Message {
  public IntPtr window; public uint message; public UIntPtr wParam; public IntPtr lParam; public uint time; public int x,y; public uint reserved;
 }
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] static extern IntPtr GetModuleHandle(string name);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern ushort RegisterClass(ref Class value);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateWindowEx(uint ex,string cls,string title,uint style,int x,int y,int w,int h,IntPtr parent,IntPtr menu,IntPtr instance,IntPtr parameter);
 [DllImport("user32.dll")] static extern IntPtr DefWindowProc(IntPtr window,uint message,UIntPtr wParam,IntPtr lParam);
 [DllImport("user32.dll")] static extern bool PeekMessage(out Message message,IntPtr window,uint minimum,uint maximum,uint remove);
 [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref Message message);
 [DllImport("user32.dll")] static extern bool DestroyWindow(IntPtr window);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool UnregisterClass(string cls,IntPtr instance);
 static Procedure procedure=Receive;
 static Dictionary<IntPtr,string> classes=new Dictionary<IntPtr,string>();
 static Dictionary<IntPtr,List<uint>> commands=new Dictionary<IntPtr,List<uint>>();
 static IntPtr Receive(IntPtr window,uint message,UIntPtr wParam,IntPtr lParam) {
  if((message==0x0010 || message==0x8004) && commands.ContainsKey(window)) { commands[window].Add(message); return IntPtr.Zero; }
  return DefWindowProc(window,message,wParam,lParam);
 }
 public static IntPtr Create(bool target) {
  string cls=(target?"Mukhomor.Native.Test.":"Mukhomor.Unrelated.Test.")+Guid.NewGuid().ToString("N");
  var instance=GetModuleHandle(null); var value=new Class{name=cls,instance=instance,procedure=Marshal.GetFunctionPointerForDelegate(procedure)};
  if(RegisterClass(ref value)==0) throw new Exception("Fixture class registration failed");
  var window=CreateWindowEx(0,cls,"Hidden upgrade fixture",0,0,0,8,8,IntPtr.Zero,IntPtr.Zero,instance,IntPtr.Zero);
  if(window==IntPtr.Zero){UnregisterClass(cls,instance);throw new Exception("Fixture window creation failed");}
  classes.Add(window,cls); commands.Add(window,new List<uint>()); return window;
 }
 public static void Pump() { Message message; while(PeekMessage(out message,IntPtr.Zero,0,0,1)) DispatchMessage(ref message); }
 public static int Count(IntPtr window,uint message) { return commands[window].FindAll(value=>value==message).Count; }
 public static void Dispose() { foreach(var item in classes){DestroyWindow(item.Key);UnregisterClass(item.Value,GetModuleHandle(null));} classes.Clear();commands.Clear(); }
}
'@
    try {
        $target=[MukhomorUpgradeFixture]::Create($true);$unrelated=[MukhomorUpgradeFixture]::Create($false)
        Assert (![MukhomorUpgrade]::CloseInterface([uint32]::MaxValue,$true)) 'retirement helper ignores every window outside the supplied process'
        Assert ([MukhomorUpgrade]::CloseInterface([uint32]$PID,$false)) 'legacy handoff finds the owned Mukhomor window'
        [MukhomorUpgradeFixture]::Pump()
        Assert ([MukhomorUpgradeFixture]::Count($target,0x0010) -eq 1 -and [MukhomorUpgradeFixture]::Count($unrelated,0x0010) -eq 0) 'legacy handoff sends WM_CLOSE only to the Mukhomor window class'
        Assert ([MukhomorUpgrade]::CloseInterface([uint32]$PID,$true)) 'current handoff finds the owned Mukhomor window'
        [MukhomorUpgradeFixture]::Pump()
        Assert ([MukhomorUpgradeFixture]::Count($target,0x8004) -eq 1 -and [MukhomorUpgradeFixture]::Count($unrelated,0x8004) -eq 0) 'current handoff sends explicit retirement rather than close-to-tray'
    } finally {[MukhomorUpgradeFixture]::Dispose()}
    Write-Host ('PASS: '+$passed+' upgrade / rollback / atomic record assertions; every SCM, CIM, process, shortcut and registry action mocked')
} finally {
    Remove-Module $harness -Force -ErrorAction SilentlyContinue
    $resolved=[IO.Path]::GetFullPath($fixture);$allowed=[IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')+'\'
    if(!$resolved.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notmatch '^upgrade-[a-f0-9]{32}$'){throw 'Fixture cleanup left the test directory'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
