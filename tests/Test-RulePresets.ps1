param([switch]$SkipCore)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'SplitVpn.psm1') -Force -DisableNameChecking
$passed=0
function Assert($Condition,[string]$Message) {if (!$Condition) {throw ('FAIL: '+$Message)};$script:passed++}
function Assert-Throws([scriptblock]$Action,[string]$Message) {$thrown=$false;try {& $Action|Out-Null}catch{$thrown=$true};Assert $thrown $Message}
$defaults=Read-JsonFile (Join-Path $package 'assets\settings.default.json')
$s=Get-SplitSettings $package $defaults
$baseline=Get-BundledRulePresets
$brokenDescriptions=Convert-ToMap $defaults
foreach ($preset in $brokenDescriptions.presets) {
    $preset.description=[Text.Encoding]::GetEncoding(1251).GetString([Text.Encoding]::UTF8.GetBytes($preset.description))
}
$repairedDescriptions=Get-SplitSettings $package $brokenDescriptions
foreach ($preset in $repairedDescriptions.presets) {
    $expected=@($baseline | Where-Object {$_.builtin_id -ceq $preset.builtin_id})[0]
    Assert ($preset.description -ceq $expected.description) ('repair legacy description: '+$preset.builtin_id)
}
$repairedDescriptions.presets[0].description='Custom description: '+[char]0x0416
$customDescriptions=Get-SplitSettings $package $repairedDescriptions
Assert ($customDescriptions.presets[0].description -ceq $repairedDescriptions.presets[0].description) 'custom description survives encoding migration'
# Hashtable insertion order may change after a defensive copy. Compare the
# complete canonical structure, including array order and exact text.
Assert ((Convert-SplitCanonicalValue $customDescriptions | ConvertTo-Json -Depth 40 -Compress) -ceq (Convert-SplitCanonicalValue (Get-SplitSettings $package $customDescriptions) | ConvertTo-Json -Depth 40 -Compress)) 'description migration is idempotent'
Assert ($s.schema -eq 2 -and $s.presets.Count -eq 5 -and $s.direct.process_names.Count -eq 0) 'editable bundled presets own their rules'
$effective=Get-EffectiveSplitSettings $s
Assert ($effective.direct.process_names -contains 'cs2.exe' -and $effective.direct.process_names -contains 'qbittorrent.exe') 'effective defaults preserve routing'
Assert ((Get-RuleSettingsIssues $s).Count -eq 0) 'baseline has no warnings'
$legacy=Convert-ToMap $defaults;$legacy.schema=1;$legacy.Remove('presets')
$legacy.direct.process_names=@('CS2.exe','custom.exe','qbittorrent.exe');$legacy.direct.domain_rule_sets=@('steam');$legacy.direct.domains=@('custom.example')
$migrated=Get-SplitSettings $package $legacy
$games=@($migrated.presets|Where-Object {$_.builtin_id -eq 'games'})[0]
Assert ($games.rules.process_names.Count -eq 1 -and $games.rules.process_names[0] -ceq 'CS2.exe') 'migration preserves user deletions and case'
Assert ($migrated.direct.process_names.Count -eq 1 -and $migrated.direct.process_names[0] -eq 'custom.exe') 'migration preserves standalone ownership'
Assert ($migrated.direct.domains[0] -eq 'custom.example') 'custom domain survives migration'
Assert (!(@($migrated.presets|Where-Object {$_.builtin_id -eq 'russia'})[0].enabled)) 'migration does not silently activate absent defaults'
$again=Get-SplitSettings $package $migrated
Assert ((Convert-SplitCanonicalValue $again|ConvertTo-Json -Depth 40 -Compress) -ceq (Convert-SplitCanonicalValue $migrated|ConvertTo-Json -Depth 40 -Compress)) 'migration is idempotent'
$migrated.presets=@($migrated.presets|Where-Object {$_.builtin_id -ne 'games'})
Assert (!(@((Get-SplitSettings $package $migrated).presets|Where-Object {$_.builtin_id -eq 'games'}).Count)) 'ordinary loading does not resurrect deleted builtins'
$nearLimit=Convert-ToMap $defaults
$nearLimit.direct.domains=@(1..2000 | ForEach-Object {('x'*1000)+'.example'})
$nearLimit=Get-SplitSettings $package $nearLimit
Assert ([Text.Encoding]::UTF8.GetByteCount(($nearLimit|ConvertTo-Json -Depth 40 -Compress)) -gt 2000000) 'near-limit fixture exercises serialized byte budget'
$nearIssues=Get-RuleSettingsIssues $nearLimit
$summaries=@(1..512|ForEach-Object {@{id=('a'*32);name=(-join (([char]0x0410).ToString()*80));protocol=('p'*40)}})
$frame=@{ok=$true;data=@{settings=$nearLimit;profiles=$summaries;rule_issues=(Select-RuleSettingsIssues $nearIssues);rule_issues_total=$nearIssues.Count}}
Assert ($nearIssues.Count -eq 2000 -and [Text.Encoding]::UTF8.GetByteCount(($frame|ConvertTo-Json -Depth 40 -Compress)) -lt 4194304) 'near-limit settings plus capped diagnostics and full server library fit IPC frame'
$overLimit=Convert-ToMap $nearLimit;$overLimit.direct.domains=@(1..2200|ForEach-Object {('x'*1000)+'.example'})
Assert-Throws {Get-SplitSettings $package $overLimit} 'serialized settings exceeding byte budget rejected before persistence'
$overLimit.direct.domains=@(1..1100|ForEach-Object {([char]0x0410).ToString()*1000})
Assert-Throws {Get-SplitSettings $package $overLimit} 'byte budget includes multibyte UTF-8 text'
$escaped=Convert-ToMap $defaults;$escaped.direct.domains=@(1..200|ForEach-Object {"'"*1024})+@(1..800|ForEach-Object {'x'*1024})
$escaped=Get-SplitSettings $package $escaped;$escapedIssues=Get-RuleSettingsIssues $escaped;$bounded=Select-RuleSettingsIssues $escapedIssues
Assert ($bounded.Count -gt 0 -and $bounded.Count -lt 200 -and [Text.Encoding]::UTF8.GetByteCount((ConvertTo-Json -InputObject $bounded -Depth 40 -Compress)) -le 524288) 'HTML-escaped diagnostics are bounded by serialized bytes'
$frame.data.settings=$escaped;$frame.data.rule_issues=$bounded;$frame.data.rule_issues_total=$escapedIssues.Count
Assert ($escapedIssues.Count -eq 1000 -and [Text.Encoding]::UTF8.GetByteCount(($frame|ConvertTo-Json -Depth 40 -Compress)) -lt 4194304) 'worst HTML-escaped warning payload with full server library fits frame'
$s.presets=@();$s.direct.domain_rule_sets=@()
$p=Convert-ToMap $baseline[0];$p.id='custom';$p.Remove('builtin_id');$p.name='Custom';$p.visible_sections=@();$p.disabled_sections=@()
foreach ($key in @($p.rules.Keys)) {$p.rules[$key]=@()}
$p.rules.process_names=@('app.exe');$p.rules.process_paths=@('C:\Apps\app.exe');$p.rules.domains=@('valid.example','https://bad.example','invalid,REJECT');$p.rules.domain_suffixes=@('suffix.example');$p.rules.ip_cidrs=@('8.8.8.8/24','0.0.0.0/0');$p.rules.rule_set_exclusions=@('vpn.example')
$s.presets=@($p);$s.direct.domains=@('standalone.example');$s.dns_update.seed_hosts=@('valid.example','missing.example')
$normalized=Get-SplitSettings $package $s;$projection=Get-RuleSettingsProjection $normalized
Assert ($normalized.presets[0].rules.domains.Count -eq 3) 'invalid raw rules remain editable'
Assert ($normalized.presets[0].rules.ip_cidrs[0] -eq '8.8.8.8/24') 'raw CIDR is not silently rewritten'
Assert ($projection.settings.direct.domains -join ',' -eq 'valid.example,standalone.example') 'presets project before standalone rules'
Assert ($projection.settings.direct.ip_cidrs -contains '8.8.8.0/24' -and $projection.settings.direct.ip_cidrs.Count -eq 1) 'runtime uses valid canonical CIDR only'
Assert (@($projection.issues|Where-Object {$_.code -eq 'invalid_rule'}).Count -eq 3) 'advisory validator reports every malformed rule'
Assert (@($projection.issues|Where-Object {$_.preset_id -eq 'custom' -and $_.section -eq 'domains' -and $_.line -eq 2}).Count -eq 1) 'warning identifies owner, section and line'
Assert ($projection.settings.dns_update.seed_hosts.Count -eq 1 -and @($projection.issues|Where-Object {$_.code -eq 'invalid_seed'}).Count -eq 1) 'unmatched seed does not break valid DNS rules'
foreach ($key in @('process_names','process_paths','domains','domain_suffixes','ip_cidrs','rule_set_exclusions')) {Assert ($projection.settings.direct[$key].Count -gt 0) ('hidden preset works in '+$key)}
$s.presets[0].disabled_sections=@('domains');$e=Get-EffectiveSplitSettings (Get-SplitSettings $package $s)
Assert ($e.direct.domains.Count -eq 1 -and $e.direct.process_names.Count -eq 1) 'section switch disables only its own rules'
$s.presets[0].enabled=$false;$e=Get-EffectiveSplitSettings (Get-SplitSettings $package $s)
Assert ($e.direct.process_names.Count -eq 0 -and $e.direct.domains[0] -eq 'standalone.example') 'preset switch preserves ordinary rules'
Assert (@((Get-RuleSettingsIssues $s)|Where-Object {$_.code -eq 'invalid_rule'}).Count -eq 3) 'disabled rules are still diagnosed'
$s.presets[0].enabled=$true;$s.presets[0].disabled_sections=@();$s.presets[0].domain_rule_sets=@('missing-source','steam');$e=Get-RuleSettingsProjection (Get-SplitSettings $package $s)
Assert ($e.settings.direct.domain_rule_sets.Count -eq 1 -and @($e.issues|Where-Object {$_.code -eq 'invalid_source'}).Count -eq 1) 'unknown source is advisory and known source works'
$shadow=Convert-ToMap $defaults;$shadow.presets=@();$shadow.dns_update.seed_hosts=@()
$shadow.direct.domains=@('exact.example')
$shadow.direct.domain_suffixes=@('suffix.example','ru','*.XN--P1AI')
$shadow.direct.rule_set_exclusions=@('exact.example','sub.exact.example','suffix.example','deep.sub.suffix.example','evilsuffix.example','suffix.example.other','host.ru','ru','child.xn--p1ai','free.other')
$shadowProjection=Get-RuleSettingsProjection (Get-SplitSettings $package $shadow)
$shadowIssues=@($shadowProjection.issues | Where-Object {$_.code -eq 'shadowed'})
Assert (($shadowIssues.line -join ',') -eq '1,3,4,7,8,9') 'shadow warnings preserve exact matches, label boundaries, deep suffixes, TLDs and normalized punycode'
Assert (@($shadowIssues | Where-Object {$_.preset_id -ne '' -or $_.section -ne 'rule_set_exclusions'}).Count -eq 0) 'standalone shadow warnings retain their owner and section'
Assert ($shadowProjection.settings.direct.rule_set_exclusions.Count -eq 10) 'shadow warnings never remove saved VPN exceptions from projection'
$shadowPreset=Convert-ToMap $baseline[0];$shadowPreset.id='shadow-owner';$shadowPreset.Remove('builtin_id');$shadowPreset.enabled=$true;$shadowPreset.disabled_sections=@()
foreach ($key in @($shadowPreset.rules.Keys)) {$shadowPreset.rules[$key]=@()}
$shadowPreset.domain_rule_sets=@();$shadowPreset.rules.domain_suffixes=@('*.SuFfIx.Example.');$shadowPreset.rules.rule_set_exclusions=@(' CHILD.SUFFIX.EXAMPLE. ')
$shadow.presets=@($shadowPreset);$shadow.direct.domains=@();$shadow.direct.domain_suffixes=@();$shadow.direct.rule_set_exclusions=@('child.suffix.example')
$shadowProjection=Get-RuleSettingsProjection (Get-SplitSettings $package $shadow)
$shadowIssues=@($shadowProjection.issues | Where-Object {$_.code -eq 'shadowed'})
Assert ($shadowIssues.Count -eq 1 -and $shadowIssues[0].preset_id -eq 'shadow-owner' -and $shadowIssues[0].line -eq 1 -and $shadowIssues[0].value -ceq ' CHILD.SUFFIX.EXAMPLE. ') 'shadow warning keeps first active owner, original text and line after normalization'
Assert (@($shadowProjection.issues | Where-Object {$_.code -eq 'duplicate' -and $_.preset_id -eq ''}).Count -eq 1) 'duplicate exceptions keep independent diagnostics'
$shadow.presets[0].disabled_sections=@('domain_suffixes')
Assert (@((Get-RuleSettingsIssues $shadow) | Where-Object {$_.code -eq 'shadowed'}).Count -eq 0) 'disabled suffix section cannot shadow exceptions'
$shadow.presets[0].disabled_sections=@();$shadow.presets[0].enabled=$false
Assert (@((Get-RuleSettingsIssues $shadow) | Where-Object {$_.code -eq 'shadowed'}).Count -eq 0) 'disabled preset cannot shadow exceptions'
$shadow.presets[0].enabled=$true;$shadow.presets[0].rules.domain_suffixes=@('https://suffix.example')
$shadowIssues=Get-RuleSettingsIssues $shadow
Assert (@($shadowIssues | Where-Object {$_.code -eq 'shadowed'}).Count -eq 0 -and @($shadowIssues | Where-Object {$_.code -eq 'invalid_rule'}).Count -eq 1) 'invalid suffix stays advisory and cannot shadow exceptions'
$invalid=Convert-ToMap $s;$invalid.presets+=@(Convert-ToMap $invalid.presets[0]);Assert-Throws {Get-SplitSettings $package $invalid} 'duplicate preset ids rejected'
$invalid=Convert-ToMap $s;$invalid.presets[0].visible_sections=@('unknown');Assert-Throws {Get-SplitSettings $package $invalid} 'unknown visibility section rejected'
$s.presets[0].domain_rule_sets=@();$s.dns_update.seed_hosts=@();$s.presets[0].rules.domains=@('valid.example','invalid,REJECT');$s.presets[0].rules.ip_cidrs=@('8.8.8.8/24')
$fixture=Join-Path $PSScriptRoot ('presets-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory((Join-Path $fixture 'private'))|Out-Null
Write-AtomicJson (Join-Path $fixture 'settings.json') $s
Write-AtomicText (Join-Path $fixture 'private\api-token.txt') ('A'*64)
Write-AtomicText (Join-Path $fixture 'private\awg.conf') "[Interface]`nPrivateKey = QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=`nAddress = 10.2.0.2/32`n[Peer]`nPublicKey = Cr8hWlKvtDt7nrvf+f0brNQQzabAqrjfBvas9pmowjo=`nEndpoint = 192.0.2.1:51820`nAllowedIPs = 0.0.0.0/0`n"
$config=New-CoreConfig $fixture $s $false
Assert ($config.rules -contains 'DOMAIN,valid.example,DIRECT' -and $config.rules -contains 'DOMAIN-SUFFIX,vpn.example,AWG') 'core includes presets and independent VPN exception without service source'
Assert (!(@($config.rules|Where-Object {$_.Contains('invalid')}).Count)) 'malformed saved rule cannot inject core syntax'
if (!$SkipCore) {$configPath=Join-Path $fixture 'private\config.json';Write-AtomicJson $configPath $config;Test-CoreConfig $package $configPath;Assert $true 'pinned core accepts preset configuration'}
$s.routing.mode='full';$config=New-CoreConfig $fixture $s $false
Assert (!(@($config.rules|Where-Object {$_ -match '^PROCESS-|^DOMAIN|^RULE-SET'}).Count)) 'full routing ignores saved preset bypasses'
$saved=Get-SplitSettings $fixture
Assert ($saved.presets[0].rules.domains -contains 'invalid,REJECT' -and $saved.direct.domains.Count -eq 1) 'serialization preserves raw text and separate ownership'
Write-Host ('PASS: '+$passed+' preset assertions; fixture '+$fixture)
