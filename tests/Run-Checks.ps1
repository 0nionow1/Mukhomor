param([switch]$SkipNative)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
$checks=@('Test-AppStartup.ps1','Test-PublicSource.ps1','Test-ReleasePe.ps1','Test-PrivateAcl.ps1','Test-Uninstall.ps1','Test-ControllerRecovery.ps1','Test-SplitVpn.ps1','Test-RulePresets.ps1','Test-FullRouting.ps1','Test-FullTrafficGuardLifecycle.ps1','Test-ProfileImport.ps1','Test-MyceliumImport.ps1','Test-MyceliumLifecycle.ps1','Test-ProtocolTransport.ps1','Test-CoreRouting.ps1','Test-UdpRouting.ps1','Test-LargeRules.ps1','Test-Transport.ps1')
if(!$SkipNative){$checks+=@('Test-NativeApp.ps1','Test-NativeGuardRecovery.ps1','Test-Isolation.ps1','Test-Lifecycle.ps1','Test-Upgrade.ps1','Test-StartupRecovery.ps1','Test-StartupCancellation.ps1','Test-DnsRecovery.ps1','Test-Disconnect.ps1','Test-PixelFont.ps1')}
if(!$SkipNative){
    # Reject invalid generated routing before the long controller fixture,
    # then check controller startup before the slower network integrations.
    $checks=@('Test-FullRouting.ps1','Test-NativeApp.ps1')+@($checks|Where-Object {$_ -notin @('Test-FullRouting.ps1','Test-NativeApp.ps1')})
}
$checks+=@('Test-BundledRouting.ps1')
foreach($name in $checks){
    Write-Host ('Checking '+$name)
    & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot $name)
    if($LASTEXITCODE -ne 0){throw ('Check failed: '+$name)}
}
Write-Host ('PASS: '+$checks.Count+' isolated check scripts')
