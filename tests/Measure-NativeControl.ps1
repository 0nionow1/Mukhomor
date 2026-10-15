param([Parameter(Mandatory=$true)][string]$Executable,[string]$Output,[int]$Repeats=5,[switch]$VerifyNative)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if($Repeats -lt 1 -or $Repeats -gt 100){throw 'Repeats must be between 1 and 100'}
. (Join-Path $PSScriptRoot 'Set-OwnedTestRoot.ps1')
$package=Split-Path -Parent $PSScriptRoot
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('control-perf-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
Set-OwnedTestRoot $fixture $fixture
[IO.Directory]::CreateDirectory((Join-Path $fixture 'private'))|Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'qa.marker'),'Isolated control benchmark; no tunnel or network access')
Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
$id='11111111111111111111111111111111'
$index=@{schema=1;selected='';last_connected='';autoconnect=$false;profiles=@(@{id=$id;name='Public fixture';format='node';protocol='ss'})}
[IO.File]::WriteAllText((Join-Path $fixture 'private\profiles.json'),($index|ConvertTo-Json -Depth 8))
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.IO.Pipes;
using System.Text;
public static class MukhomorControlProbe {
    public static string Name(string root) {
        ulong hash=0xcbf29ce484222325UL;
        foreach(byte b in Encoding.UTF8.GetBytes(root.ToLowerInvariant())) hash=unchecked((hash^b)*0x100000001b3UL);
        return "Mukhomor-"+hash.ToString("x16");
    }
    static byte[] Read(Stream stream, int count) {
        byte[] bytes=new byte[count]; int at=0;
        while(at<count) {
            var task=stream.ReadAsync(bytes,at,count-at);
            if(!task.Wait(100000)) throw new TimeoutException("IPC read timed out");
            int n=task.Result; if(n==0) throw new EndOfStreamException(); at+=n;
        }
        return bytes;
    }
    public static byte[] Request(string root, byte[] bytes) {
        using(var pipe=new NamedPipeClientStream(".",Name(root),PipeDirection.InOut,PipeOptions.Asynchronous)) {
            pipe.Connect(500);
            byte[] header=BitConverter.GetBytes((uint)bytes.Length);
            pipe.Write(header,0,4); pipe.Write(bytes,0,bytes.Length); pipe.Flush();
            int length=BitConverter.ToInt32(Read(pipe,4),0);
            if(length<0 || length>4194304) throw new InvalidDataException("IPC limit");
            byte[] result=Read(pipe,length); pipe.WriteByte(0); pipe.Flush(); return result;
        }
    }
}
'@
$samples=New-Object 'Collections.Generic.List[object]'
function Request($Value,[string]$Case='') {
    $bytes=[Text.Encoding]::UTF8.GetBytes(($Value|ConvertTo-Json -Depth 40 -Compress))
    $watch=[Diagnostics.Stopwatch]::StartNew()
    $reply=[MukhomorControlProbe]::Request($fixture,$bytes)
    $elapsed=$watch.Elapsed.TotalMilliseconds
    if($Case){$samples.Add([ordered]@{case=$Case;elapsed_ms=$elapsed;reply_bytes=$reply.Length})}
    return ([Text.Encoding]::UTF8.GetString($reply)|ConvertFrom-Json)
}
function Assert($Condition,[string]$Message){if(!$Condition){throw $Message}}
$worker=$null
try {
    $watch=[Diagnostics.Stopwatch]::StartNew()
    $worker=Start-Process -FilePath $Executable -ArgumentList ('--worker --base "'+$package+'" --root "'+$fixture+'"') -WindowStyle Hidden -PassThru
    $ready=$false
    $profilesMs=$null
    while($watch.Elapsed.TotalSeconds -lt 60){
        try {
            $initial=Request @{action='Status'}
            if($null -eq $profilesMs -and $initial.PSObject.Properties.Name -contains 'data' -and @($initial.data.profiles).Count -gt 0){$profilesMs=$watch.Elapsed.TotalMilliseconds}
            if($initial.ok){$ready=$true;break}
        }catch{if($worker.HasExited){throw 'Worker exited before readiness'}}
        Start-Sleep -Milliseconds 50
    }
    Assert $ready 'Controller did not become ready'
    $readyMs=$watch.Elapsed.TotalMilliseconds
    for($i=0;$i -lt $Repeats;$i++) {
        $full=Request @{action='Status'} 'status_full'
        $revision=if($full.PSObject.Properties.Name -contains 'revision'){$full.revision}else{'unsupported'}
        $conditional=Request @{action='Status';if_revision=$revision} 'status_conditional'
        if($VerifyNative){Assert ($conditional.not_modified -and !($conditional.PSObject.Properties.Name -contains 'data')) 'Unchanged status must be compact'}
        $rename=Request @{action='Rename';id=$id;name="Fixture $i";request_id="rename-$i"} 'rename'
        Assert ($rename.ok -and $rename.data.profiles[0].name -eq "Fixture $i") 'Rename failed'
        $auto=Request @{action='Autoconnect';enabled=($i % 2 -eq 0);request_id="auto-$i"} 'autoconnect'
        Assert ($auto.ok -and $auto.data.autoconnect -eq ($i % 2 -eq 0)) 'Autoconnect failed'
        if($VerifyNative){
            $changed=Request @{action='Status';if_revision=$revision}
            Assert ($changed.PSObject.Properties.Name -contains 'data') 'Mutation must invalidate old revision'
            Assert ($changed.data.last_operation.request_id -eq "auto-$i") 'Polling must recover operation receipts'
        }
    }
    $before=[IO.File]::ReadAllText((Join-Path $fixture 'private\profiles.json'))
    $invalid=Request @{action='Rename';id=$id;name="bad`nname";request_id='invalid-name'} 'invalid_rename'
    Assert (!$invalid.ok -and [IO.File]::ReadAllText((Join-Path $fixture 'private\profiles.json')) -ceq $before) 'Invalid rename must preserve disk state'
    $saved=$before|ConvertFrom-Json
    Assert ($saved.profiles[0].name -eq "Fixture $($Repeats-1)" -and !$saved.selected -and !$saved.last_connected) 'Profile metadata and connection history must remain separate'
    $report=[ordered]@{schema=1;executable_sha256=(Get-FileHash -LiteralPath $Executable).Hash;first_profiles_ms=$profilesMs;controller_ready_ms=$readyMs;scope='Isolated real controller; no TUN, DNS changes or VPN connection. RPC timings exclude JSON parsing in this probe.';samples=@($samples.ToArray())}
    if(!$Output){$Output=Join-Path $fixture 'performance.json'}
    $report|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $Output -Encoding UTF8
    Write-Host ('PASS: control operations and status; measurements '+$Output)
} finally {
    if($worker){
        try {if(!$worker.HasExited){$null=Request @{action='ShutdownWorker'};if(!$worker.WaitForExit(30000)){$worker.Kill();$worker.WaitForExit()}}}catch{if(!$worker.HasExited){$worker.Kill();$worker.WaitForExit()}}
        $worker.Dispose()
    }
}
