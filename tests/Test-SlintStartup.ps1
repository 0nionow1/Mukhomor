param([string]$Executable,[string]$Base)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=if($Base){[IO.Path]::GetFullPath($Base)}else{Split-Path -Parent $PSScriptRoot}
if(!$Executable){$Executable=Join-Path $package 'native\target\release\mukhomor.exe'}
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('slint-start-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
$app=$null; $second=$null
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class SlintStartupProbe {
    delegate bool EnumProc(IntPtr window, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    public static bool Retire(uint process) {
        IntPtr target=IntPtr.Zero;
        EnumWindows((window,param) => {
            uint owner; GetWindowThreadProcessId(window,out owner);
            if(owner!=process) return true;
            var text=new StringBuilder(256); GetClassName(window,text,text.Capacity);
            if(!text.ToString().StartsWith("Mukhomor.Native.",StringComparison.Ordinal)) return true;
            target=window; return false;
        },IntPtr.Zero);
        return target!=IntPtr.Zero && PostMessage(target,0x8004,IntPtr.Zero,IntPtr.Zero);
    }
}
'@
try {
    $arguments='--qa-startup --base "'+$package+'" --root "'+$fixture+'" --output "'+(Join-Path $fixture 'ui-error.txt')+'"'
    # Missing marker must fail before installer or live profile access.
    $second=Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $null=$second.Handle
    if(!$second.WaitForExit(15000) -or $second.ExitCode -ne 1){throw 'Unmarked QA root was not rejected'}
    if(!(Test-Path -LiteralPath (Join-Path $fixture 'ui-error.txt'))){throw 'Missing startup rejection'}
    $second.Dispose(); $second=$null
    [IO.File]::Delete((Join-Path $fixture 'ui-error.txt'))
    [IO.File]::WriteAllText((Join-Path $fixture 'qa.marker'),'Isolated Slint UI; no controller or Connect requests.')
    $app=Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $null=$app.Handle
    Start-Sleep -Seconds 5
    if($app.HasExited){throw ('Slint startup exited with '+$app.ExitCode)}
    if(Test-Path -LiteralPath (Join-Path $fixture 'ui-error.txt')){throw 'Slint reported a startup error'}
    $second=Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $null=$second.Handle
    if(!$second.WaitForExit(15000) -or $second.ExitCode -ne 0 -or $app.HasExited){throw 'Single-instance activation failed'}
    if(![SlintStartupProbe]::Retire([uint32]$app.Id)){throw 'Activation endpoint was not found'}
    if(!$app.WaitForExit(15000) -or $app.ExitCode -ne 0){throw 'Slint did not retire cleanly'}
    'PASS: Slint default window, isolated-root guard, second-instance activation and installer retirement. No VPN connection requested.'
} finally {
    foreach($child in @($app,$second)){
        if($null -ne $child){try {if(!$child.HasExited){$child.Kill();$null=$child.WaitForExit(5000)}}finally{$child.Dispose()}}
    }
    $resolved=(Resolve-Path -LiteralPath $fixture).ProviderPath
    if([IO.Path]::GetDirectoryName($resolved) -ne $PSScriptRoot -or [IO.Path]::GetFileName($resolved) -notmatch '^slint-start-[a-f0-9]{32}$'){throw 'Unexpected cleanup path'}
    if((Get-Item -LiteralPath $resolved).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse fixture root'}
    if(@(Get-ChildItem -LiteralPath $resolved -Recurse -Force | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}).Count){throw 'Reparse fixture descendant'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
