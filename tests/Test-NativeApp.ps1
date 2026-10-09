param([string]$Executable,[string]$Base,[switch]$RecoveryOnly)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=if($Base){[IO.Path]::GetFullPath($Base)}else{Split-Path -Parent $PSScriptRoot}
if (!$Executable) {$Executable=Join-Path $package 'native\target\release\mukhomor.exe'}
$Executable=[IO.Path]::GetFullPath($Executable)
$fixture=Join-Path $PSScriptRoot ('native-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture) | Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'qa.marker'),'isolated test; no network connection')
$passed=0; $worker=$null
function Assert($Condition,[string]$Message) {if(!$Condition){throw ('FAIL: '+$Message)}; $script:passed++}
function Request($Value) {
    $request=Join-Path $fixture 'request.json'; $output=Join-Path $fixture 'response.json'
    [IO.File]::WriteAllText($request,($Value|ConvertTo-Json -Depth 40 -Compress),(New-Object Text.UTF8Encoding($false)))
    if(Test-Path -LiteralPath $output){[IO.File]::Delete($output)}
    $arguments='--rpc --root "'+$fixture+'" --request "'+$request+'" --output "'+$output+'"'
    $client=Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $null=$client.Handle
    if(!$client.WaitForExit(155000)) {$client.Kill();throw 'RPC timed out'}
    $raw=[IO.File]::ReadAllText($output)
    if($client.ExitCode -ne 0){throw ('RPC '+$Value.action+' failed: '+$raw)}
    return ($raw|ConvertFrom-Json)
}
function Start-Worker {
    $arguments='--worker --base "'+$package+'" --root "'+$fixture+'" --output "'+(Join-Path $fixture 'worker-error.txt')+'"'
    $script:worker=Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $ready=$false
    $readiness=[Diagnostics.Stopwatch]::StartNew();$lastStartupError=''
    while($readiness.Elapsed.TotalSeconds -lt 60){
        try {$snapshot=Request @{action='Status'}; if($snapshot.ok){$ready=$true;break};$lastStartupError=$snapshot|ConvertTo-Json -Depth 8 -Compress} catch {if($worker.HasExited){throw 'Worker exited during initialization'};$lastStartupError=$_.Exception.Message}
        Start-Sleep -Milliseconds 200
    }
    if(!$ready){Write-Host ('Startup did not become ready within 60 seconds: '+$lastStartupError)}
    Assert $ready 'worker starts with no imported profile'
}
function Test-SelectRollback {
    # Execute the real bridge switch/catch flow with pure fault-injection
    # functions. This child has no worker, core, adapter, DNS or service access.
    $mockRoot=Join-Path $fixture 'select-rollback'
    [IO.Directory]::CreateDirectory((Join-Path $mockRoot 'private'))|Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $mockRoot 'runtime'))|Out-Null
    [IO.File]::WriteAllText((Join-Path $mockRoot 'settings.json'),'{}')
    $oldId='22222222222222222222222222222222';$targetId='11111111111111111111111111111111'
    $mockIndex=@{schema=1;selected=$oldId;autoconnect=$false;profiles=@(@{id=$oldId;name='Old fixture';format='node';protocol='ss'},@{id=$targetId;name='Target fixture';format='node';protocol='ss'})}
    [IO.File]::WriteAllText((Join-Path $mockRoot 'private\profiles.json'),($mockIndex|ConvertTo-Json -Depth 12))
    $source=[IO.File]::ReadAllText((Join-Path $package 'NativeBridge.ps1'))
    foreach($name in @('SplitVpn.psm1','SplitWindows.psm1')){
        $original="Import-Module (Join-Path `$PSScriptRoot '$name')"
        $absolute=(Join-Path $package $name).Replace("'","''")
        $source=$source.Replace($original,("Import-Module '$absolute'"))
    }
    $mocks=@'
$script:mockActivations=0
function Initialize-SplitRoot([string]$Root) {}
function Get-OwnedCore([string]$Root) {return $null}
function Import-SplitProfile([string]$Root,[string]$Source) {
    $script:mockActivations++
    if($script:mockActivations -eq 1){throw 'ORIGINAL_SELECT_FAILURE'}
    throw 'ROLLBACK_SELECT_FAILURE'
}
function Save-ProfileIndex($Index) {
    Write-AtomicJson (Join-Path $Root 'runtime\mock-save.json') @{selected=$Index.selected;activations=$script:mockActivations}
    Write-AtomicJson (Join-Path $Root 'private\profiles.json') $Index
}
'@
    Assert ($source.Contains('$request=@{}')) 'the pure Select recovery fixture locates the real bridge entry point'
    $source=$source.Replace('$request=@{}',($mocks+"`n"+'$request=@{}'))
    # This fault-injection fixture tests Select recovery, while the native RPC
    # checks below cover the real stdin transport. Feed its public request via
    # a file to avoid inheriting the hosted PowerShell shell's input stream.
    $inputPath=Join-Path $mockRoot 'select-request.json'
    [IO.File]::WriteAllText($inputPath,(@{action='Select';id=$targetId}|ConvertTo-Json -Compress),(New-Object Text.UTF8Encoding($false)))
    $inputExpression="`$raw=[IO.File]::ReadAllText('"+$inputPath.Replace("'","''")+"')"
    Assert ($source.Contains('$raw=[Console]::In.ReadToEnd()')) 'the pure Select fixture locates the request reader'
    $source=$source.Replace('$raw=[Console]::In.ReadToEnd()',$inputExpression)
    $scriptPath=Join-Path $mockRoot 'bridge-fixture.ps1'
    [IO.File]::WriteAllText($scriptPath,$source,(New-Object Text.UTF8Encoding($true)))
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $info.Arguments='-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'+$scriptPath+'" -Root "'+$mockRoot+'"'
    $info.UseShellExecute=$false;$info.CreateNoWindow=$true
    $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    $child=New-Object Diagnostics.Process;$child.StartInfo=$info
    try {
        [void]$child.Start()
        $stdout=$child.StandardOutput.ReadToEndAsync();$stderr=$child.StandardError.ReadToEndAsync()
        # This is a functional recovery check, not a startup latency assertion.
        # Allow cold PowerShell/module loading on hosted Windows runners.
        if(!$child.WaitForExit(30000)){throw 'Pure Select recovery fixture timed out'}
        if(!$stdout.Wait(1000) -or !$stderr.Wait(1000)){throw 'Pure Select recovery fixture output timed out'}
        $reply=$stdout.Result|ConvertFrom-Json
        if($child.ExitCode -ne 1 -or $reply.ok -or $reply.error -ne 'ORIGINAL_SELECT_FAILURE' -or $reply.error_details.rollback_failures -notcontains 'profiles'){
            # This child uses only pure mock functions and public fixture IDs.
            # Its reply makes environment-specific fixture failures diagnosable.
            Write-Host ('Pure recovery fixture exit: '+$child.ExitCode)
            Write-Host $stdout.Result
            Write-Host $stderr.Result
        }
        Assert ($child.ExitCode -eq 1 -and !$reply.ok -and $reply.error -eq 'ORIGINAL_SELECT_FAILURE' -and $reply.error_details.rollback_failures -contains 'profiles') 'failed old-profile restoration preserves the original Select error and marks recovery failure'
        $saved=[IO.File]::ReadAllText((Join-Path $mockRoot 'runtime\mock-save.json'))|ConvertFrom-Json
        $restored=[IO.File]::ReadAllText((Join-Path $mockRoot 'private\profiles.json'))|ConvertFrom-Json
        Assert ($saved.activations -eq 2 -and $saved.selected -eq $oldId -and $restored.selected -eq $oldId) 'Select recovery still saves the old authoritative index after its profile restoration throws'
    } finally {
        try {if(!$child.HasExited){$child.Kill();$child.WaitForExit(1000)|Out-Null}} catch {}
        $child.Dispose()
    }
}
# Public, non-routable fixture; these are test-vector keys, never a real profile.
$profile=@'
[Interface]
PrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=
Address = 10.2.0.2/32, fd00::2/128
MTU = 1280
Jc = 5
Jmin = 40
Jmax = 1000
S1 = 122
S2 = 110
H1 = 3309746145
H2 = 1247832263
H3 = 3073505960
H4 = 1410779371
[Peer]
PublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=
Endpoint = 192.0.2.1:51820
AllowedIPs = 0.0.0.0/0, ::/0
PersistentKeepalive = 25
'@
try {
    Test-SelectRollback
    if($RecoveryOnly){Write-Host ('PASS: '+$passed+' pure Select rollback assertions');return}
    Start-Worker
    $fresh=Request @{action='Status'}
    Assert (!$fresh.data.running -and @($fresh.data.profiles).Count -eq 0) 'fresh install remains disconnected'
    Assert ($fresh.data.settings.direct.process_names -contains 'qbittorrent.exe') 'public torrent bypass preset'
    $first=Request @{action='Import';name='Test A';content=$profile}
    Assert ($first.ok -and @($first.data.profiles).Count -eq 1) 'first profile imported'
    $idA=$first.data.selected
    $second=Request @{action='Import';name='Test B';content=$profile.Replace('192.0.2.1','192.0.2.2')}
    Assert ($second.ok -and @($second.data.profiles).Count -eq 2 -and $second.data.selected -eq $idA) 'second import keeps current selection'
    $idB=@($second.data.profiles|Where-Object {$_.id -ne $idA})[0].id
    $unicode=-join ([char[]](0x0422,0x0435,0x0441,0x0442,0x0020,0x0411))
    $rename=Request @{action='Rename';id=$idB;name=$unicode}
    Assert ($rename.ok -and @($rename.data.profiles|Where-Object {$_.id -eq $idB})[0].name -eq $unicode) 'Unicode server names survive IPC'
    $bad=Request @{action='Import';name='Bad';content='PRIVATE-CANARY-NATIVE-DIAGNOSTICS-invalid-profile'}
    Assert (!$bad.ok) 'malformed profile rejected'
    $detailPath=Join-Path $fixture 'runtime\last-error.json'
    Assert (Test-Path -LiteralPath $detailPath) 'helper saves safe error location for failed operations'
    $detailRaw=[IO.File]::ReadAllText($detailPath)
    $detail=$detailRaw|ConvertFrom-Json
    Assert ($detail.schema -eq 1 -and $detail.action -eq 'Import' -and $detail.script -in @('SplitVpn.psm1','ProfileImport.psm1') -and $detail.line -gt 0 -and $detail.exception_type) 'diagnostics identifies the actual failure stage and source location'
    Assert (!$detailRaw.Contains('PRIVATE-CANARY-NATIVE-DIAGNOSTICS') -and !$detailRaw.Contains($fixture) -and !$detailRaw.Contains('QUFBQUFBQUFB') -and !$detailRaw.Contains('Cr8hWlKvt')) 'diagnostics excludes source lines, request content, profile values and local paths'
    Assert (@($detail.PSObject.Properties.Name|Where-Object {$_ -notin @('schema','action','exception_type','hresult','provider_code','command','script','line','rollback_failures','utc')}).Count -eq 0) 'diagnostics contains only typed codes, location and safe rollback labels'
    $snapshot=Request @{action='Status'}
    Assert (@($snapshot.data.profiles).Count -eq 2 -and $snapshot.data.selected -eq $idA) 'failed import retains library and active selection'
    $traversal=Request @{action='Remove';id='..\..\other'}
    Assert (!$traversal.ok) 'profile ID traversal rejected'
    $s=$snapshot.data.settings
    $s.direct.process_names=@('cs2.exe','qbittorrent.exe')
    $s.direct.process_paths=@('C:\Games\Example\game.exe')
    $s.direct.domains=@('exact.example.com')
    $s.direct.domain_suffixes=@('ru','xn--p1ai','example.com')
    $s.direct.ip_cidrs=@('8.8.8.8','2606:4700::/32')
    $s.direct.domain_rule_sets=@('russia','steam','ozon')
    $s.direct.rule_set_exclusions=@('ubs.com')
    $s.dns_update.enabled=$false
    $s.dns_update.observe_subdomains=$true
    $s.dns_update.seed_hosts=@('api.example.com')
    $s.dns_update.min_refresh_seconds=45
    $s.dns_update.max_refresh_seconds=1200
    $s.dns_update.max_stale_seconds=7200
    $saved=Request @{action='ApplySettings';settings=$s}
    Assert $saved.ok 'all types of bypass rules accepted together'
    $config=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
    foreach($rule in @('PROCESS-NAME,cs2.exe,DIRECT','PROCESS-PATH,C:\Games\Example\game.exe,DIRECT','DOMAIN,exact.example.com,DIRECT','DOMAIN-SUFFIX,ru,DIRECT','DOMAIN-SUFFIX,example.com,DIRECT','IP-CIDR,8.8.8.8/32,DIRECT','IP-CIDR6,2606:4700::/32,DIRECT','RULE-SET,service-russia,DIRECT')) {Assert ($config.rules -contains $rule) ('generated route '+$rule)}
    $invalid=$saved.data.settings|ConvertTo-Json -Depth 40|ConvertFrom-Json
    $invalid.direct.ip_cidrs=@('0.0.0.0/0')
    $rejected=Request @{action='ApplySettings';settings=$invalid}
    Assert (!$rejected.ok) 'invalid full-traffic bypass rejected'
    $switch=Request @{action='Select';id=$idB}
    Assert ($switch.ok -and $switch.data.selected -eq $idB) 'profile switches while disconnected'
    Assert ($switch.data.settings.direct.domains -contains 'exact.example.com' -and $switch.data.settings.dns_update.min_refresh_seconds -eq 45) 'switch preserves exclusions and DNS preferences'
    $public=$switch|ConvertTo-Json -Depth 40
    Assert (!$public.Contains('QUFBQUFBQUFB') -and !$public.Contains('private-key') -and !$public.Contains('secret')) 'IPC status excludes profile keys and controller token'
    # Decoded profile size and JSON transport size are separate limits.
    # WG ignores this comment; JSON must escape each control byte to six bytes.
    $escapedContent=$profile+"`n#"+(([string][char]1)*200000)
    $escapedRequest=@{action='Import';name='Escaped comment';content=$escapedContent;request_id='native-escaped-comment'}
    Assert ([Text.Encoding]::UTF8.GetByteCount($escapedContent) -lt 524288 -and [Text.Encoding]::UTF8.GetByteCount(($escapedRequest|ConvertTo-Json -Depth 40 -Compress)) -gt 1048576) 'escaped fixture stays below the decoded profile limit while exceeding one MiB on IPC'
    $escaped=Request $escapedRequest
    Assert ($escaped.ok -and $escaped.data.imported_count -eq 1 -and @($escaped.data.profiles).Count -eq 3 -and $escaped.data.selected -eq $idB) 'a valid control-comment WG profile reaches the parser through expanded JSON IPC'
    $escapedId=@($escaped.data.profiles|Where-Object {$_.id -ne $idA -and $_.id -ne $idB})[0].id
    Assert (Request @{action='Remove';id=$escapedId}).ok 'expanded-JSON import can be removed without changing the selected server'
    # Fault only an inactive fixture node. The worker must retain its active
    # selection/config, and the original library file is restored afterward.
    $inactivePath=Join-Path $fixture "private\profiles\$idA.json"
    $inactiveText=[IO.File]::ReadAllText($inactivePath)
    try {
        [IO.File]::WriteAllText($inactivePath,'PRIVATE-CANARY-NATIVE-SELECT-invalid-json',(New-Object Text.UTF8Encoding($false)))
        $failedSwitch=Request @{action='Select';id=$idA;request_id='native-invalid-select'}
        Assert (!$failedSwitch.ok -and $failedSwitch.data.selected -eq $idB -and @($failedSwitch.data.profiles).Count -eq 2 -and !$failedSwitch.data.running -and !$failedSwitch.data.busy) 'an invalid inactive node fails selection while retaining the previous selected server'
        $restoredConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
        Assert (@($restoredConfig.proxies)[0].type -eq 'wireguard' -and @($restoredConfig.proxies)[0].server -eq '192.0.2.2' -and $restoredConfig.rules -contains 'PROCESS-NAME,qbittorrent.exe,DIRECT' -and $failedSwitch.data.settings.dns_update.min_refresh_seconds -eq 45) 'failed selection restores the previous generated transport and bypass settings'
    } finally {[IO.File]::WriteAllText($inactivePath,$inactiveText,(New-Object Text.UTF8Encoding($false)))}
    # A valid maximum-sized manual domain list produces a successful helper
    # snapshot above one MiB after settings commit. Exercise both the receipt
    # and the subsequent cached Status before restoring ordinary preferences.
    $previousSettings=$failedSwitch.data.settings
    $largeSettings=$previousSettings|ConvertTo-Json -Depth 40|ConvertFrom-Json
    $longDomains=@(for($n=0;$n -lt 5000;$n++){
        ('d'+$n.ToString('D4')+('a'*58))+'.'+('b'*63)+'.'+('c'*63)+'.'+('d'*61)
    })
    $largeSettings.direct.domains=$longDomains
    Assert ($longDomains.Count -eq 5000 -and $longDomains[0].Length -eq 253 -and [Text.Encoding]::UTF8.GetByteCount(($largeSettings|ConvertTo-Json -Depth 40 -Compress)) -gt 1048576) 'the valid 5000-domain fixture creates a settings snapshot above one MiB'
    try {
        $large=Request @{action='ApplySettings';settings=$largeSettings;request_id='native-large-settings'}
        Assert ($large.ok -and !$large.data.busy -and $large.data.selected -eq $idB -and @($large.data.settings.direct.domains).Count -eq 5000) 'a large valid settings commit returns a complete successful helper frame'
        $largeStatus=Request @{action='Status'}
        Assert ($largeStatus.ok -and !$largeStatus.data.busy -and @($largeStatus.data.settings.direct.domains).Count -eq 5000 -and $largeStatus.data.settings.direct.domains[0] -eq $longDomains[0]) 'large committed settings remain complete in the subsequent native Status response'
    } finally {$largeRestore=Request @{action='ApplySettings';settings=$previousSettings}}
    Assert ($largeRestore.ok -and @($largeRestore.data.settings.direct.domains).Count -eq 1 -and $largeRestore.data.settings.direct.domains[0] -eq 'exact.example.com') 'the large-frame regression restores the previous bypass preferences'
    # Imported endpoints are TEST-NET addresses and .invalid hostnames. Import,
    # config validation and disconnected selection must not dial these nodes.
    $realityKey='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
    $jsonNodes=@{proxies=@(
        @{name='Fixture JSON VLESS';type='vless';server='192.0.2.41';port=443;uuid='11111111-1111-4111-8111-111111111111';tls=$true;servername='front.example.invalid';'client-fingerprint'='chrome';'reality-opts'=@{'public-key'=$realityKey;'short-id'='0123456789abcdef'};network='tcp';udp=$true},
        @{name='Fixture JSON SS';type='ss';server='192.0.2.42';port=8388;cipher='aes-128-gcm';password='PRIVATE-CANARY-NATIVE-JSON-SS';udp=$true}
    )}
    $jsonImport=Request @{action='Import';name='JSON batch';format='json';content=($jsonNodes|ConvertTo-Json -Depth 20 -Compress);request_id='native-json-batch'}
    Assert ($jsonImport.ok -and @($jsonImport.data.profiles).Count -eq 4 -and $jsonImport.data.selected -eq $idB -and !$jsonImport.data.running) 'JSON batch imports all nodes without changing selection or connecting'
    Assert ($jsonImport.data.imported_count -eq 2 -and $jsonImport.data.last_operation.imported_count -eq 2 -and $jsonImport.data.last_operation.request_id -eq 'native-json-batch') 'successful import receipt identifies the exact batch and server count'
    $jsonStatus=Request @{action='Status'}
    Assert ($jsonStatus.data.last_operation.imported_count -eq 2 -and $jsonStatus.data.last_operation.request_id -eq 'native-json-batch') 'import count remains available when the UI reconciles through Status'
    $vless=@($jsonImport.data.profiles|Where-Object {$_.name -eq 'Fixture JSON VLESS'})
    $ss=@($jsonImport.data.profiles|Where-Object {$_.name -eq 'Fixture JSON SS'})
    Assert ($vless.Count -eq 1 -and $ss.Count -eq 1 -and $vless[0].protocol -match 'VLESS' -and $ss[0].protocol) 'batch preserves node names and exposes protocol labels'
    foreach($node in @($vless[0],$ss[0])) {
        Assert (@($node.PSObject.Properties.Name|Where-Object {$_ -notin @('id','name','protocol')}).Count -eq 0) 'public profile metadata contains only ID, name and protocol'
    }
    $advanced=Request @{action='Select';id=$vless[0].id}
    Assert ($advanced.ok -and $advanced.data.selected -eq $vless[0].id -and !$advanced.data.running) 'a JSON protocol node can be selected while disconnected'
    $advancedConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
    $advancedProxy=@($advancedConfig.proxies)[0]
    Assert ($advancedProxy.type -eq 'vless' -and $advancedProxy.server -eq '192.0.2.41' -and $advancedProxy.servername -eq 'front.example.invalid' -and $advancedProxy.'client-fingerprint' -eq 'chrome' -and $advancedProxy.'reality-opts'.'public-key' -eq $realityKey -and $advancedProxy.'reality-opts'.'short-id' -eq '0123456789abcdef') 'generated config preserves VLESS TLS and Reality identity options'
    Assert ($advancedConfig.rules -contains 'PROCESS-NAME,qbittorrent.exe,DIRECT' -and $advancedConfig.rules -contains 'DOMAIN-SUFFIX,ru,DIRECT' -and $advanced.data.settings.dns_update.min_refresh_seconds -eq 45) 'advanced node selection preserves direct bypass and DNS preferences'
    Assert (Request @{action='Select';id=$idB}).ok 'a legacy imported WG node remains selectable after an advanced node'
    $ssAuth=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('aes-128-gcm:PRIVATE-CANARY-NATIVE-URI-SS')).TrimEnd('=').Replace('+','-').Replace('/','_')
    $uriNodes='trojan://PRIVATE-CANARY-NATIVE-URI-TROJAN@192.0.2.44:443?type=ws&sni=front.example.invalid&host=cdn.example.invalid&path=%2Fsocket#Fixture%20URI%20Trojan'+"`n"+'ss://'+$ssAuth+'@192.0.2.45:8388#Fixture%20URI%20SS'
    $uriImport=Request @{action='Import';name='URI batch';content=$uriNodes;request_id='native-uri-batch'}
    Assert ($uriImport.ok -and @($uriImport.data.profiles).Count -eq 6 -and $uriImport.data.imported_count -eq 2 -and $uriImport.data.selected -eq $idB) 'automatic URI detection imports every newline-delimited node'
    $trojan=@($uriImport.data.profiles|Where-Object {$_.name -eq 'Fixture URI Trojan'})
    Assert ($trojan.Count -eq 1 -and $trojan[0].protocol -match 'Trojan') 'URI fragment and protocol metadata survive import'
    Assert (Request @{action='Select';id=$trojan[0].id}).ok 'a URI protocol node can be selected without connecting'
    $uriConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
    $uriProxy=@($uriConfig.proxies)[0]
    Assert ($uriProxy.type -eq 'trojan' -and $uriProxy.password -eq 'PRIVATE-CANARY-NATIVE-URI-TROJAN' -and $uriProxy.sni -eq 'front.example.invalid' -and $uriProxy.network -eq 'ws' -and $uriProxy.'ws-opts'.path -eq '/socket' -and $uriProxy.'ws-opts'.headers.Host -eq 'cdn.example.invalid') 'URI import preserves credentials, SNI, WebSocket Host and decoded path'
    Assert (Request @{action='Select';id=$idB}).ok 'WG selection restores after URI node selection'
    $libraryPath=Join-Path $fixture 'private\profiles.json'
    $beforeLibrary=[IO.File]::ReadAllText($libraryPath)
    $beforeConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))
    $beforeFiles=@(Get-ChildItem -LiteralPath (Join-Path $fixture 'private\profiles') -File|Sort-Object Name|Select-Object -ExpandProperty Name)
    $badNodes=@{proxies=@(
        @{name='Uncommitted valid node';type='ss';server='192.0.2.46';port=8388;cipher='aes-128-gcm';password='PRIVATE-CANARY-NATIVE-ATOMIC'},
        @{name='Uncommitted unsupported node';type='unsupported-native-fixture';server='192.0.2.47';port=443;password='PRIVATE-CANARY-NATIVE-ATOMIC'}
    )}
    $badBatch=Request @{action='Import';name='Bad batch';format='json';content=($badNodes|ConvertTo-Json -Depth 12 -Compress);request_id='native-bad-batch'}
    Assert (!$badBatch.ok -and @($badBatch.data.profiles).Count -eq 6 -and $badBatch.data.selected -eq $idB -and !$badBatch.data.busy) 'one invalid node rejects the entire batch with a terminal operation state'
    Assert ([IO.File]::ReadAllText($libraryPath) -ceq $beforeLibrary -and [IO.File]::ReadAllText((Join-Path $fixture 'private\config.json')) -ceq $beforeConfig -and (@(Get-ChildItem -LiteralPath (Join-Path $fixture 'private\profiles') -File|Sort-Object Name|Select-Object -ExpandProperty Name) -join '|') -ceq ($beforeFiles -join '|')) 'failed batch leaves library, active config and profile storage unchanged'
    $batchDetails=[IO.File]::ReadAllText($detailPath)
    Assert (!$batchDetails.Contains('PRIVATE-CANARY-NATIVE-ATOMIC') -and !$batchDetails.Contains($fixture) -and !$batchDetails.Contains('192.0.2.46')) 'batch failure diagnostics omit credentials, endpoints and local paths'
    $tooLarge=Request @{action='Import';name='Oversized';content=('x'*524289)}
    Assert (!$tooLarge.ok -and @($tooLarge.data.profiles).Count -eq 6 -and $tooLarge.data.selected -eq $idB) '512 KiB import limit rejects oversized content without changing the library'
    $batchPublic=(Request @{action='Status'})|ConvertTo-Json -Depth 40 -Compress
    Assert (!$batchPublic.Contains('PRIVATE-CANARY-NATIVE-JSON-SS') -and !$batchPublic.Contains('PRIVATE-CANARY-NATIVE-URI-SS') -and !$batchPublic.Contains('PRIVATE-CANARY-NATIVE-URI-TROJAN') -and !$batchPublic.Contains($realityKey) -and !$batchPublic.Contains('private-key') -and !$batchPublic.Contains('password') -and !$batchPublic.Contains('secret')) 'public status excludes advanced protocol credentials and private transport options'
    $newIds=@($uriImport.data.profiles|Where-Object {$_.id -ne $idA -and $_.id -ne $idB}|Select-Object -ExpandProperty id)
    foreach($id in $newIds){Assert (Request @{action='Remove';id=$id}).ok 'each batch node can be removed while preserving the WG selection'}
    $restoredLibrary=Request @{action='Status'}
    Assert (@($restoredLibrary.data.profiles).Count -eq 2 -and $restoredLibrary.data.selected -eq $idB) 'removing every batch node restores the two original profiles'
    $auto=Request @{action='Autoconnect';enabled=$true}
    Assert ($auto.ok -and $auto.data.autoconnect) 'automatic connection preference persists'
    $off=Request @{action='Autoconnect';enabled=$false}
    Assert ($off.ok -and !$off.data.autoconnect) 'automatic connection can be disabled'
    for($n=0;$n -lt 10;$n++){Request @{action='Status'}|Out-Null}
    Start-Sleep -Seconds 2
    $worker.Refresh();$cpuBefore=$worker.TotalProcessorTime.TotalMilliseconds
    Start-Sleep -Seconds 5
    $worker.Refresh();$cpuDelta=$worker.TotalProcessorTime.TotalMilliseconds-$cpuBefore
    Assert ($cpuDelta -lt 150) 'idle controller waits on events rather than polling PowerShell'
    $metrics=@{working_set_mb=[math]::Round($worker.WorkingSet64/1MB,2);private_mb=[math]::Round($worker.PrivateMemorySize64/1MB,2);idle_cpu_ms_over_5s=[math]::Round($cpuDelta,2);core_connected=$false}
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(15000)) 'controller shuts down cleanly'
    # Model an on-disk library created before normalized node storage existed.
    # Change only this stopped worker's TEST-NET fixture, retaining its IDs.
    $legacyIndex=[IO.File]::ReadAllText($libraryPath)|ConvertFrom-Json
    $legacyEntry=@($legacyIndex.profiles|Where-Object {$_.id -eq $idA})[0]
    $legacyEntry.PSObject.Properties.Remove('format')
    $legacyEntry.PSObject.Properties.Remove('protocol')
    [IO.File]::WriteAllText((Join-Path $fixture "private\profiles\$idA.conf"),$profile,(New-Object Text.UTF8Encoding($false)))
    [IO.File]::Delete((Join-Path $fixture "private\profiles\$idA.json"))
    [IO.File]::WriteAllText($libraryPath,($legacyIndex|ConvertTo-Json -Depth 20 -Compress),(New-Object Text.UTF8Encoding($false)))
    # A disconnected, non-existent adapter must keep its snapshot without
    # making the management interface unavailable. No real adapter is changed.
    $backup=Join-Path $fixture 'runtime\dns-backup.json'
    [IO.File]::WriteAllText($backup,(@{schema=1;adapters=@(@{guid=[guid]::NewGuid().ToString('B');name='Nonexistent test adapter';family='ipv4';source='dhcp';servers=@()})}|ConvertTo-Json -Depth 8))
    [IO.File]::WriteAllText((Join-Path $fixture 'runtime\stop.request'),'stale stop marker')
    Start-Worker
    $restart=Request @{action='Status'}
    Assert ($restart.data.selected -eq $idB -and @($restart.data.profiles).Count -eq 2 -and !$restart.data.running) 'profile library survives service process restart'
    Assert ($restart.data.service_ready -and $restart.data.dns_recovery_pending -and (Test-Path -LiteralPath $backup)) 'deferred DNS recovery keeps IPC available and snapshot intact'
    Assert (!(Test-Path -LiteralPath (Join-Path $fixture 'runtime\stop.request'))) 'stale shutdown marker is removed at service startup'
    [IO.File]::Delete($backup)
    $legacySwitch=Request @{action='Select';id=$idA}
    Assert ($legacySwitch.ok -and $legacySwitch.data.selected -eq $idA -and !$legacySwitch.data.running -and @($legacySwitch.data.profiles).Count -eq 2) 'a pre-upgrade conf entry without format and protocol remains selectable'
    $legacyConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
    Assert (@($legacyConfig.proxies)[0].type -eq 'wireguard' -and @($legacyConfig.proxies)[0].server -eq '192.0.2.1' -and $legacyConfig.rules -contains 'PROCESS-NAME,qbittorrent.exe,DIRECT') 'legacy conf conversion preserves its endpoint and split-routing settings'
    Assert (Request @{action='Select';id=$idB}).ok 'selection can return from pre-upgrade conf storage to normalized node storage'
    $beforeLarge=(Request @{action='Status'}).data.settings
    $largeSettings=$beforeLarge|ConvertTo-Json -Depth 40|ConvertFrom-Json
    $largeSettings.direct.domains=@(1..5000|ForEach-Object {"exact$_.native-fixture.test"})
    $largeSettings.direct.domain_suffixes=@(1..5000|ForEach-Object {"suffix$_.native-fixture.test"})
    $largeSettings.direct.ip_cidrs=@(1..5000|ForEach-Object {'203.0.'+[int][Math]::Floor($_/256)+'.'+($_%256)+'/32'})
    $largeSettings.dns_update.enabled=$false;$largeSettings.dns_update.seed_hosts=@()
    $largeApplied=Request @{action='ApplySettings';settings=$largeSettings}
    Assert ($largeApplied.ok -and !$largeApplied.data.busy -and !$largeApplied.data.running -and $largeApplied.data.settings.direct.domains.Count -eq 5000 -and $largeApplied.data.settings.direct.domain_suffixes.Count -eq 5000 -and $largeApplied.data.settings.direct.ip_cidrs.Count -eq 5000) '15000 exceptions apply through the real controller within its normal operation deadline'
    $largeConfig=[IO.File]::ReadAllText((Join-Path $fixture 'private\config.json'))|ConvertFrom-Json
    Assert ($largeConfig.'rule-providers'.'custom-domains'.payload.Count -eq 10000 -and $largeConfig.'rule-providers'.'custom-ips'.payload.Count -eq 5000 -and $largeConfig.rules.Count -lt 100) 'maximum custom lists compile into bounded native rule groups without losing entries'
    Assert (Request @{action='ApplySettings';settings=$beforeLarge}).ok 'large-list apply can restore the previous preferences without altering the selected profile'
    $remove=Request @{action='Remove';id=$idA}
    Assert ($remove.ok -and @($remove.data.profiles).Count -eq 1 -and $remove.data.selected -eq $idB) 'remove inactive profile preserves selection'
    Request @{action='ShutdownWorker'}|Out-Null
    Assert ($worker.WaitForExit(15000)) 'restarted controller shuts down cleanly'
    [IO.Directory]::CreateDirectory((Join-Path $package 'runtime'))|Out-Null
    [IO.File]::WriteAllText((Join-Path $package 'runtime\native-test-results.json'),(@{assertions=$passed;metrics=$metrics;network_tun_tested=$false}|ConvertTo-Json -Depth 6))
    Write-Host ('PASS: '+$passed+' native IPC / profile / persistence / routing assertions')
    Write-Host ($metrics|ConvertTo-Json -Compress)
} finally {
    if($worker -and !$worker.HasExited){$worker.Kill();$worker.WaitForExit()}
    $resolved=[IO.Path]::GetFullPath($fixture)
    if($resolved.StartsWith(([IO.Path]::GetFullPath($PSScriptRoot)+'\native-'),[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^native-[a-f0-9]{32}$') {Remove-Item -LiteralPath $resolved -Recurse -Force}
}
