Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'AppStartup.ps1')
$passed=0
function Assert($Condition,[string]$Message) {if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
$program='C:\Program Files\Mukhomor'
$old=Join-Path $program ('app-0.5.0-'+('a'*32)+'\mukhomor.exe')
$new=Join-Path $program ('app-0.6.0-'+('b'*32)+'\mukhomor.exe')
$command=Get-AppStartupCommand $old
Assert ($command -ceq ('"'+$old+'" --startup')) 'installed path is quoted and starts only the tray interface'
Assert (Test-AppStartupCommand $command $program) 'previous installed versions are owned'
foreach($foreign in @('foreign.exe','"C:\Downloads\mukhomor.exe" --startup',($command+' --smoke'),'"C:\bad|path\app-0.6.0-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\mukhomor.exe" --startup','"C:\bad<path\app-0.6.0-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\mukhomor.exe" --startup','"C:app-0.6.0-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\mukhomor.exe" --startup')) {
    Assert (!(Test-AppStartupCommand $foreign $program)) 'foreign or malformed commands are preserved'
}
$key=[pscustomobject]@{Value=$command;Writes=0;Deletes=0;Approved=[byte[]](3,0,0,0)}
$key|Add-Member ScriptMethod GetValue {param($Name,$Default,$Options) if($Name -cne 'Mukhomor'){throw 'Unexpected registry value'};return $this.Value}
$key|Add-Member ScriptMethod SetValue {param($Name,$Value,$Kind) if($Name -cne 'Mukhomor'){throw 'Unexpected registry write'};$this.Value=$Value;$this.Writes++}
$key|Add-Member ScriptMethod DeleteValue {param($Name,$Throw) if($Name -cne 'Mukhomor'){throw 'Unexpected registry deletion'};$this.Value=$null;$this.Deletes++}
Update-AppStartupKey $key $program $new
Assert ($key.Value -ceq (Get-AppStartupCommand $new) -and $key.Writes -eq 1) 'upgrade follows the new version in the same owner hive'
Assert ($key.Approved[0] -eq 3) 'upgrade does not override Windows startup approval'
Update-AppStartupKey $key $program '' -Remove
Assert ($null -eq $key.Value -and $key.Deletes -eq 1) 'removal deletes only the owned entry'
Update-AppStartupKey $key $program $new
Assert ($null -eq $key.Value -and $key.Writes -eq 1) 'install does not enable an absent registration'
foreach($foreign in @('foreign.exe','"C:\bad|path\mukhomor.exe" --startup')) {
    $key.Value=$foreign
    Update-AppStartupKey $key $program $new
    Update-AppStartupKey $key $program '' -Remove
    Assert ($key.Value -ceq $foreign -and $key.Writes -eq 1 -and $key.Deletes -eq 1) 'update and uninstall leave foreign values intact'
}
$failed=$false;try {Get-AppStartupCommand ('C:\'+('x'*260)+'\mukhomor.exe')|Out-Null}catch{$failed=$true}
Assert $failed 'Windows startup length limit is enforced'
$failed=$false;try {Get-AppStartupCommand 'mukhomor.exe'|Out-Null}catch{$failed=$true}
Assert $failed 'relative startup paths are rejected'
$failed=$false;try {Get-AppStartupCommand 'D:mukhomor.exe'|Out-Null}catch{$failed=$true}
Assert $failed 'drive-relative registration paths are rejected'
Assert (!(Test-AppStartupCommand '"D:app-0.6.0-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\mukhomor.exe" --startup' ([Environment]::CurrentDirectory))) 'drive-relative foreign command cannot inherit current-directory ownership'
Write-Host ('PASS: '+$passed+' startup registration assertions; all registry actions mocked')
