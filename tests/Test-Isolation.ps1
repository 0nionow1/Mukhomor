param([string]$Executable)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
if(!$Executable){$Executable=Join-Path $package 'native\target\release\mukhomor.exe'}
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('isolation-'+[guid]::NewGuid().ToString('N'))
$roots=@((Join-Path $fixture 'Mukhomor-A'),(Join-Path $fixture 'Mukhomor-B'))
$legacy=Join-Path $fixture 'VPN-Split';[IO.Directory]::CreateDirectory((Join-Path $legacy 'private'))|Out-Null
[IO.File]::WriteAllText((Join-Path $legacy 'private\awg.conf'),'PRIVATE-LEGACY-CANARY: never import this file')
[IO.File]::WriteAllText((Join-Path $legacy 'settings.json'),'{"device":"VPN-Split","proxy":19790,"controller":19793,"dns":19753}')
$before=@{}
foreach($file in @(Get-ChildItem -LiteralPath $legacy -File -Recurse)){$before[$file.FullName]=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash}
$workers=New-Object Collections.Generic.List[object];$passed=0
function Assert($Condition,[string]$Message){if(!$Condition){throw ('FAIL: '+$Message)};$script:passed++}
function Request([string]$Root,$Value){
    $id=[guid]::NewGuid().ToString('N');$input=Join-Path $fixture ($id+'-request.json');$output=Join-Path $fixture ($id+'-response.json')
    [IO.File]::WriteAllText($input,($Value|ConvertTo-Json -Depth 30 -Compress),(New-Object Text.UTF8Encoding($false)))
    $client=Start-Process -FilePath $Executable -ArgumentList ('--rpc --root "'+$Root+'" --request "'+$input+'" --output "'+$output+'"') -WindowStyle Hidden -PassThru
    $null=$client.Handle
    if(!$client.WaitForExit(20000)){$client.Kill();throw 'Isolation RPC timed out'}
    if($client.ExitCode -ne 0){throw ('Isolation RPC failed: '+[IO.File]::ReadAllText($output))}
    return ([IO.File]::ReadAllText($output)|ConvertFrom-Json)
}
$profile=@'
[Interface]
PrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=
Address = 10.2.0.2/32
[Peer]
PublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=
Endpoint = 192.0.2.1:51820
AllowedIPs = 0.0.0.0/0
'@
try {
    foreach($root in $roots){
        [IO.Directory]::CreateDirectory($root)|Out-Null
        [IO.File]::WriteAllText((Join-Path $root 'qa.marker'),'Two isolated disconnected controllers; no system DNS or TUN')
        $worker=Start-Process -FilePath $Executable -ArgumentList ('--worker --base "'+$package+'" --root "'+$root+'" --output "'+(Join-Path $root 'worker-error.json')+'"') -PassThru -WindowStyle Hidden
        $workers.Add($worker);$ready=$false
        $readiness=[Diagnostics.Stopwatch]::StartNew()
        while($readiness.Elapsed.TotalSeconds -lt 60){try{if((Request $root @{action='Status'}).ok){$ready=$true;break}}catch{if($worker.HasExited){throw 'Isolation worker exited'}};Start-Sleep -Milliseconds 100}
        Assert $ready 'independent controller starts beside the other instance'
    }
    $a=Request $roots[0] @{action='Import';name='Fixture A';content=$profile}
    Assert ($a.ok -and $a.data.profiles.Count -eq 1) 'profile imports into instance A'
    $b=Request $roots[1] @{action='Status'}
    Assert ($b.data.profiles.Count -eq 0 -and !$b.data.selected) 'instance B does not discover or inherit A profiles'
    $b=Request $roots[1] @{action='Import';name='Fixture B';content=$profile.Replace('192.0.2.1','192.0.2.2')}
    Assert ($b.ok -and $b.data.selected -ne $a.data.selected) 'profile identities remain separate across controllers'
    $s=$a.data.settings;$s.direct.process_names=@('fixture-a.exe');$s.direct.domains=@('a.example.com')
    $a=Request $roots[0] @{action='ApplySettings';settings=$s}
    Assert $a.ok 'A changes its own rules'
    $b=Request $roots[1] @{action='Status'}
    Assert ($b.data.settings.direct.process_names -contains 'qbittorrent.exe' -and $b.data.settings.direct.domains -notcontains 'a.example.com') 'B retains independent public preset and rule settings'
    foreach($root in $roots){
        $config=Get-Content -LiteralPath (Join-Path $root 'private\config.json') -Raw|ConvertFrom-Json
        Assert ($config.tun.device -eq 'Mukhomor' -and $config.'mixed-port' -eq 19890 -and $config.'external-controller' -eq '127.0.0.1:19893' -and $config.dns.listen -eq '127.0.0.1:19853') 'generated TUN identity and ports do not share private VPN-Split defaults'
        Assert ($config.proxies[0].server -notlike '*CANARY*') 'profile generation ignores the neighbouring legacy private directory'
    }
    $removed=Request $roots[0] @{action='Remove';id=$a.data.selected}
    Assert ($removed.ok -and (Request $roots[1] @{action='Status'}).data.profiles.Count -eq 1) 'removing A profile leaves B profile intact'
    # ShutdownWorker acknowledges before cleanup; its Disconnect helper has a 20 s hard limit.
    # Give that bounded cleanup time to finish on a cold Windows runner.
    Request $roots[0] @{action='ShutdownWorker'}|Out-Null
    Assert ($workers[0].WaitForExit(30000) -and (Request $roots[1] @{action='Status'}).ok) 'stopping controller A leaves B IPC available'
    foreach($file in $before.Keys){Assert ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -eq $before[$file]) 'neighbouring legacy private file is byte-for-byte unchanged'}
    Request $roots[1] @{action='ShutdownWorker'}|Out-Null
    Assert ($workers[1].WaitForExit(30000)) 'controller B exits separately'
    Write-Host ('PASS: '+$passed+' functional root / IPC / profile / rule isolation assertions; no TUN or system DNS')
} finally {
    foreach($worker in $workers){if(!$worker.HasExited){$worker.Kill();$worker.WaitForExit()}}
    $resolved=[IO.Path]::GetFullPath($fixture);$expected=([IO.Path]::GetFullPath($PSScriptRoot)+'\isolation-')
    if($resolved.StartsWith($expected,[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^isolation-[a-f0-9]{32}$'){Remove-Item -LiteralPath $resolved -Recurse -Force}
}
