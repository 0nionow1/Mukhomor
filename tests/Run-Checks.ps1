param([switch]$SkipNative)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
$checks=@('Test-PublicSource.ps1','Test-SplitVpn.ps1','Test-ProfileImport.ps1','Test-ProtocolTransport.ps1','Test-CoreRouting.ps1','Test-UdpRouting.ps1','Test-LargeRules.ps1','Test-Transport.ps1')
if(!$SkipNative){$checks+=@('Test-NativeApp.ps1','Test-Isolation.ps1','Test-Lifecycle.ps1','Test-Upgrade.ps1','Test-StartupRecovery.ps1','Test-StartupCancellation.ps1','Test-DnsRecovery.ps1','Test-Disconnect.ps1','Test-PixelFont.ps1')}
if(!$SkipNative){
    # Check controller startup before the slower network fixtures on CI.
    $checks=@('Test-NativeApp.ps1')+@($checks|Where-Object {$_ -ne 'Test-NativeApp.ps1'})
}
foreach($name in $checks){
    Write-Host ('Checking '+$name)
    & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot $name)
    if($LASTEXITCODE -ne 0){throw ('Check failed: '+$name)}
}
Write-Host ('PASS: '+$checks.Count+' isolated check scripts')
