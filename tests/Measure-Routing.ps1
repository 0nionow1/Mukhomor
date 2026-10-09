# Reproducible local comparison. Never a WAN latency or Wintun benchmark.
param([ValidateRange(32,5000)][int]$Entries=5000,[ValidateRange(100,10000)][int]$Iterations=800,[ValidateRange(1,8)][int]$Rounds=3)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$fixture=Join-Path $PSScriptRoot ('benchmark-'+[guid]::NewGuid().ToString('N'))
$core=$null;$origin=$null;$allocated=New-Object 'Collections.Generic.HashSet[int]'
function Free-Port {
    do {$l=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0);try {$l.Start();$p=$l.LocalEndpoint.Port} finally {$l.Stop()}} while(!$allocated.Add($p))
    return $p
}
function Median($Values) {$sorted=@($Values|Sort-Object);return $sorted[[int][Math]::Floor($sorted.Count/2)]}
try {
    [IO.Directory]::CreateDirectory($fixture)|Out-Null
    Copy-Item -LiteralPath (Join-Path $package 'assets\settings.default.json') -Destination (Join-Path $fixture 'settings.json')
    Initialize-SplitRoot $fixture
    $s=Get-SplitSettings $fixture
    $s.ports.proxy=Free-Port;$s.ports.controller=Free-Port;$s.ports.dns=Free-Port
    $s.direct.process_names=@();$s.direct.process_paths=@();$s.direct.domain_rule_sets=@();$s.direct.rule_set_exclusions=@()
    $s.direct.domains=@(1..($Entries-1)|ForEach-Object {"exact$_.benchmark.test"})+@('domain-hit.benchmark.test')
    $s.direct.domain_suffixes=@(1..$Entries|ForEach-Object {"suffix$_.benchmark.test"})
    $s.direct.ip_cidrs=@(1..($Entries-1)|ForEach-Object {'203.0.'+[int][Math]::Floor($_/256)+'.'+($_%256)+'/32'})+@('127.0.0.1/32')
    $s.dns_update.enabled=$false;$s.dns_update.observe_subdomains=$false;$s.dns_update.seed_hosts=@()
    Write-AtomicJson (Join-Path $fixture 'settings.json') $s
    Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = eCtXsJZ27+4PbhDkHnB923tkUn2Gj59wZw5wFA75MnU=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
    $c=New-CoreConfig $fixture $s $false
    if($c['find-process-mode'] -ne 'off'){throw 'Empty EXE lists must disable socket attribution'}
    $c.rules=@($c.rules|Where-Object {$_ -notmatch '^IP-CIDR'}|ForEach-Object {$_ -replace ',AWG$',',REJECT'})
    # Hosts provide deterministic DNS. Both configurations route the same data
    # through the same core, native sockets and HTTP origin. Only rule storage differs.
    $c.hosts=@{'domain-hit.benchmark.test'='127.0.0.1';'ip-hit.benchmark.test'='127.0.0.1'}
    $c.proxies=@();$c.dns=@{enable=$false};$c.sniffer=@{enable=$false};$c.ntp=@{enable=$false}
    $optimizedPath=Join-Path $fixture 'private\optimized.json';Write-AtomicJson $optimizedPath $c
    $baseline=Convert-ToMap ($c|ConvertTo-Json -Depth 40|ConvertFrom-Json)
    $baseline['rule-providers']=@{}
    $baseline.rules=@($s.direct.domains|ForEach-Object {"DOMAIN,$_,DIRECT"})+@($s.direct.domain_suffixes|ForEach-Object {"DOMAIN-SUFFIX,$_,DIRECT"})+@($s.direct.ip_cidrs|ForEach-Object {"IP-CIDR,$_,DIRECT"})+@('DOMAIN-REGEX,.+,REJECT','MATCH,REJECT')
    $baselinePath=Join-Path $fixture 'private\linear.json';Write-AtomicJson $baselinePath $baseline
    Test-CoreConfig $package $baselinePath;Test-CoreConfig $package $optimizedPath
    $originPortPath=Join-Path $fixture 'origin-port.txt'
    $origin=Start-Process (Get-Command node.exe).Source -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'local-origin.cjs')+'"'),('"'+$originPortPath+'"')) -WindowStyle Hidden -PassThru
    for($n=0;$n -lt 40 -and !(Test-Path -LiteralPath $originPortPath);$n++){if($origin.HasExited){throw 'Origin exited'};Start-Sleep -Milliseconds 100}
    $originPort=[int][IO.File]::ReadAllText($originPortPath)
    $core=Start-Process (Get-CorePath $package) -ArgumentList @('-d',('"'+$fixture+'"'),'-f',('"'+$optimizedPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core-error.log')
    $ready=$false
    for($n=0;$n -lt 40;$n++){if($core.HasExited){throw 'Core exited'};try {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null;$ready=$true;break} catch {Start-Sleep -Milliseconds 100}}
    if(!$ready){throw 'Core readiness timed out'}
    $samples=@()
    for($round=1;$round -le $Rounds;$round++) {
        # Alternate order to reduce warm-cache and background-load bias.
        $variants=if($round%2){@('linear','optimized')}else{@('optimized','linear')}
        foreach($variant in $variants) {
            $path=if($variant -eq 'linear'){$baselinePath}else{$optimizedPath}
            Invoke-CoreApi $fixture '/configs?force=true' 'PUT' @{path=$path}|Out-Null
            $core.Refresh();$cpuBefore=$core.TotalProcessorTime.TotalMilliseconds
            $out=Join-Path $fixture ($variant+'-'+$round+'.json')
            & node.exe (Join-Path $PSScriptRoot 'routing-benchmark.cjs') $s.ports.proxy $originPort $Iterations $out
            if($LASTEXITCODE -ne 0){throw 'Benchmark request failed'}
            $core.Refresh();$sample=Read-JsonFile $out
            $sample.variant=$variant;$sample.round=$round
            # Includes fixed warmup and relay CPU; never label this pure lookup CPU.
            $sample.core_cpu_ms=$core.TotalProcessorTime.TotalMilliseconds-$cpuBefore
            $samples+=@($sample)
            Write-Host ("$variant round $round : p50="+[Math]::Round($sample.p50_ms,3)+'ms; p95='+[Math]::Round($sample.p95_ms,3)+'ms; core CPU='+[Math]::Round($sample.core_cpu_ms,2)+'ms')
        }
    }
    # Compare the former authenticated API preparation with the new port-only
    # preparation, using the same running core, token, request and settings file.
    $apiSamples=@()
    foreach($variant in @('full-settings','port-only')) {
        for($n=0;$n -lt 5;$n++) {
            $timer=[Diagnostics.Stopwatch]::StartNew()
            if($variant -eq 'full-settings') {
                $old=Get-SplitSettings $fixture
                $token=[IO.File]::ReadAllText((Join-Path $fixture 'private\api-token.txt')).Trim()
                Invoke-RestMethod -Uri ("http://127.0.0.1:$($old.ports.controller)/version") -Headers @{Authorization="Bearer $token"} -TimeoutSec 1 -UseBasicParsing|Out-Null
            } else {Invoke-CoreApi $fixture '/version' -TimeoutSeconds 1|Out-Null}
            $apiSamples+=@(@{variant=$variant;elapsed_ms=$timer.Elapsed.TotalMilliseconds})
        }
    }
    $summary=@{}
    foreach($variant in @('linear','optimized')) {
        $group=@($samples|Where-Object {$_.variant -eq $variant})
        $summary[$variant]=@{p50_ms=(Median @($group|ForEach-Object {$_.p50_ms}));p95_ms=(Median @($group|ForEach-Object {$_.p95_ms}));core_cpu_ms=(Median @($group|ForEach-Object {$_.core_cpu_ms}));rules=$(if($variant -eq 'linear'){$baseline.rules.Count}else{$c.rules.Count})}
    }
    foreach($variant in @('full-settings','port-only')) {$summary[$variant]=@{median_ms=(Median @($apiSamples|Where-Object {$_.variant -eq $variant}|ForEach-Object {$_.elapsed_ms}))}}
    $report=@{schema=1;core='mihomo v1.19.32';scope='Windows loopback fresh HTTP connections; no Wintun, WAN, encryption, throughput or game latency measurement';entries_per_list=$Entries;connections_per_round=$Iterations;rounds=$Rounds;samples=$samples;api_samples=$apiSamples;summary=$summary}
    $reportPath=Join-Path $fixture 'results.json';Write-AtomicJson $reportPath $report
    Write-Host ($summary|ConvertTo-Json -Depth 5)
    Write-Host ('Benchmark results: '+$reportPath)
} finally {
    foreach($process in @($core,$origin)){if($process){try {if(!$process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}} finally {$process.Dispose()}}}
}
