# Shared saved model and runtime projection. Dot-sourced by SplitVpn.psm1.
$script:PresetRuleKeys=@('process_names','process_paths','domains','domain_suffixes','ip_cidrs','rule_set_exclusions')
$script:ProjectionCache=@{}
$script:ProjectionVersion=$null
$script:BundledPresets=$null

function Get-BundledRulePresets {
    if($null -ne $script:BundledPresets){return ,(Convert-ToMap $script:BundledPresets)}
    $defaults=Read-JsonFile (Join-Path $PSScriptRoot 'assets\settings.default.json')
    $presets=Convert-ToMap $defaults.presets
    $script:BundledPresets=Convert-ToMap $presets
    return ,$presets
}

function Convert-RuleSettings($Settings,[switch]$Owned) {
    $s=if($Owned){$Settings}else{Convert-ToMap $Settings}
    if ($s.schema -eq 1) {
        # Partition only the rules actually present: never resurrect a user's deletions.
        $presets=New-Object 'Collections.Generic.List[object]'
        foreach ($baseline in (Get-BundledRulePresets)) {
            $p=Convert-ToMap $baseline
            foreach ($key in $script:PresetRuleKeys) {
                $p.rules[$key]=@($s.direct[$key] | Where-Object {$_ -in $baseline.rules[$key]})
                $s.direct[$key]=@($s.direct[$key] | Where-Object {$_ -notin $baseline.rules[$key]})
            }
            $p.domain_rule_sets=@($s.direct.domain_rule_sets | Where-Object {$_ -in $baseline.domain_rule_sets})
            $s.direct.domain_rule_sets=@($s.direct.domain_rule_sets | Where-Object {$_ -notin $baseline.domain_rule_sets})
            $p.visible_sections=@($script:PresetRuleKeys | Where-Object {$p.rules[$_].Count})
            $p.enabled=(@($p.domain_rule_sets).Count -gt 0 -or @($p.visible_sections).Count -gt 0)
            $presets.Add($p)
        }
        $s.presets=@($presets.ToArray()); $s.schema=2
    }
    if ($s.schema -ne 2) {throw 'Unsupported settings schema'}
    if ($s.presets -isnot [array] -or $s.presets.Count -gt 128) {throw 'Expected at most 128 presets'}
    $ids=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    $bundled=Get-BundledRulePresets
    $builtinIds=@($bundled | ForEach-Object {$_.builtin_id})
    $owners=@(@{id='';rules=$s.direct;domain_rule_sets=$s.direct.domain_rule_sets})
    foreach ($p in $s.presets) {
        if ($p -isnot [hashtable] -or $p.id -isnot [string] -or $p.id -cnotmatch '^[A-Za-z0-9_-]{1,80}$' -or !$ids.Add($p.id)) {throw 'Invalid or duplicate preset id'}
        if ($p.name -isnot [string] -or $p.name.Trim().Length -lt 1 -or $p.name.Length -gt 120 -or $p.name -match '[\r\n\x00-\x1f]') {throw 'Preset name must contain 1-120 characters on one line'}
        if ($p.description -isnot [string] -or $p.description.Length -gt 1000) {throw 'Invalid preset description'}
        if ($p.enabled -isnot [bool]) {throw 'Preset enabled must be boolean'}
        if ($p.ContainsKey('builtin_id') -and $p.builtin_id -cnotin $builtinIds) {throw 'Unknown bundled preset identity'}
        # Earlier releases shipped UTF-8 descriptions decoded as Windows-1251.
        # Match the exact shipped text and identity; never reinterpret user text.
        if ($p.ContainsKey('builtin_id')) {
            $original=@($bundled | Where-Object {$_.builtin_id -ceq $p.builtin_id})[0]
            $broken=[Text.Encoding]::GetEncoding(1251).GetString([Text.Encoding]::UTF8.GetBytes($original.description))
            if ($p.description -ceq $broken) {$p.description=$original.description}
        }
        if (!$p.ContainsKey('disabled_sections')) {$p.disabled_sections=@()}
        foreach ($field in @('visible_sections','disabled_sections')) {
            if ($p[$field] -isnot [array] -or @($p[$field] | Where-Object {$_ -cnotin $script:PresetRuleKeys}).Count) {throw "Invalid preset $field"}
            $p[$field]=@($p[$field] | Select-Object -Unique)
        }
        $owners+=@(@{id=$p.id;rules=$p.rules;domain_rule_sets=$p.domain_rule_sets})
    }
    $total=0
    foreach ($owner in $owners) {
        if ($owner.rules -isnot [hashtable] -or $owner.domain_rule_sets -isnot [array] -or $owner.domain_rule_sets.Count -gt 32) {throw 'Invalid preset rules or sources'}
        foreach ($value in $owner.domain_rule_sets) {if ($value -isnot [string] -or $value.Length -gt 120) {throw 'Invalid source value'}}
        foreach ($key in $script:PresetRuleKeys) {
            if ($owner.rules[$key] -isnot [array] -or $owner.rules[$key].Count -gt 5000) {throw "Rule section $key exceeds 5000 entries or is not an array"}
            $total+=$owner.rules[$key].Count
            foreach ($value in $owner.rules[$key]) {if ($value -isnot [string] -or $value.Length -gt 1024) {throw 'Rules must be strings of at most 1024 characters'}}
        }
    }
    if ($total -gt 30000) {throw 'Total rule count exceeds 30000 entries'}
    return $s
}

function Convert-PresetRule([string]$Section,[string]$Value) {
    $value=$Value.Trim()
    if (!$value -or $value -match '[\x00-\x1f\x7f]') {throw 'Empty rule or control character'}
    switch ($Section) {
        'process_names' {if ($value -notmatch '^[^\\/:,*?\r\n]+\.exe$') {throw 'Expected an EXE name without a path'};return $value}
        'process_paths' {if ($value -match '[,\r\n*?]' -or $value -notmatch '^[A-Za-z]:\\.+\.exe$') {throw 'Expected an absolute EXE path'};return $value}
        'domains' {return Normalize-Domain $value}
        'domain_suffixes' {return Normalize-Domain $value -Suffix}
        'rule_set_exclusions' {return Normalize-Domain $value -Suffix}
        'ip_cidrs' {return Normalize-Cidr $value}
    }
    throw 'Unknown rule section'
}

function Convert-SplitCanonicalValue($Value) {
    if($Value -is [System.Collections.IDictionary]) {
        $result=[ordered]@{}
        foreach($key in @($Value.Keys | Sort-Object -CaseSensitive)) {$result[$key]=Convert-SplitCanonicalValue $Value[$key]}
        return $result
    }
    if($Value -is [array]) {
        $result=[object[]]::new($Value.Count)
        for($i=0;$i -lt $Value.Count;$i++) {
            $item=$Value[$i]
            $result[$i]=if($item -is [System.Collections.IDictionary] -or $item -is [array]) {Convert-SplitCanonicalValue $item}else{$item}
        }
        return ,$result
    }
    return $Value
}
function Get-SplitTextHash([string]$Text) {
    $hash=[Security.Cryptography.SHA256]::Create()
    try {return [Convert]::ToBase64String($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)))}finally{$hash.Dispose()}
}
function Get-RuleSettingsProjection($Settings) {
    if($null -eq $script:ProjectionVersion) {
        $script:ProjectionVersion='projection-v1:'+((@('RulePresets.ps1','RuleNormalizer.cs','SplitVpn.psm1','assets\settings.default.json') | ForEach-Object {(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $_)).Hash}) -join ':')
    }
    $key=Get-SplitTextHash ($script:ProjectionVersion+(Convert-SplitCanonicalValue $Settings | ConvertTo-Json -Depth 40 -Compress))
    if($script:ProjectionCache.ContainsKey($key)){return Convert-ToMap $script:ProjectionCache[$key]}
    $cachePath=if($env:MUKHOMOR_STATE_ROOT){Join-Path $env:MUKHOMOR_STATE_ROOT 'runtime\rule-projection-cache.json'}else{''}
    if($cachePath) {
        try {
            $item=Get-Item -LiteralPath $cachePath -ErrorAction Stop
            if($item.Length -le 16777216 -and !($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                $cache=Read-JsonFile $cachePath
                if($cache.schema -eq 1 -and $cache.key -ceq $key -and $cache.payload -is [string] -and (Get-SplitTextHash $cache.payload) -ceq $cache.hash) {
                    $projection=Convert-ToMap ($cache.payload | ConvertFrom-Json)
                    if($projection.settings -is [hashtable] -and $projection.issues -is [array]) {
                        $script:ProjectionCache.Clear();$script:ProjectionCache[$key]=$projection
                        return Convert-ToMap $projection
                    }
                }
            }
        } catch {} # A missing or damaged performance cache is always rebuilt.
    }
    $projection=New-RuleSettingsProjection $Settings
    $script:ProjectionCache.Clear();$script:ProjectionCache[$key]=Convert-ToMap $projection
    if($cachePath) {
        try {
            $payload=$projection | ConvertTo-Json -Depth 40 -Compress
            if([Text.Encoding]::UTF8.GetByteCount($payload) -le 8388608) {Write-AtomicJson $cachePath @{schema=1;key=$key;payload=$payload;hash=(Get-SplitTextHash $payload)}}
        } catch {} # Caching must not turn a valid operation into a failure.
    }
    return $projection
}
function New-RuleSettingsProjection($Settings) {
    # Never write this projection back to settings.json: saved lists keep ownership and raw text.
    $s=Convert-ToMap $Settings
    $direct=@{domain_rule_sets=@()};$issues=New-Object 'Collections.Generic.List[object]'
    $seen=@{};$buffers=@{};$exclusionOrigins=@{};foreach ($key in $script:PresetRuleKeys) {$buffers[$key]=New-Object 'Collections.Generic.List[string]';$seen[$key]=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)}
    $owners=@($s.presets)+@(@{id='';name='';enabled=$true;disabled_sections=@();rules=$s.direct;domain_rule_sets=$s.direct.domain_rule_sets})
    $ruleCount=0;foreach($owner in $owners){foreach($key in $script:PresetRuleKeys){$ruleCount+=$owner.rules[$key].Count}}
    $fastNormalizer=$null -ne ('MukhomorRuleNormalizer' -as [type])
    if(!$fastNormalizer -and $ruleCount -gt 512) {Add-Type -Path (Join-Path $PSScriptRoot 'RuleNormalizer.cs');$fastNormalizer=$true}
    $catalog=Get-DomainRuleSetCatalog
    foreach ($p in $owners) {
        foreach ($key in $script:PresetRuleKeys) {
            $active=($p.enabled -and $key -notin $p.disabled_sections);$line=0
            foreach ($raw in $p.rules[$key]) {
                $line++
                try {
                    $rule=if($fastNormalizer){[MukhomorRuleNormalizer]::Convert($key,$raw)}else{Convert-PresetRule $key $raw}
                    if ($active) {
                        if ($seen[$key].Add($rule)) {
                            $buffers[$key].Add($rule)
                            if ($key -eq 'rule_set_exclusions') {$exclusionOrigins[$rule]=@{preset_id=$p.id;section=$key;line=$line;value=$raw;code='shadowed';message='A direct domain rule has higher priority than this VPN exception'}}
                        }
                        else {$issues.Add(@{preset_id=$p.id;section=$key;line=$line;value=$raw;code='duplicate';message='Rule duplicates another active entry'})}
                    }
                } catch {
                    $message=if($fastNormalizer -and $_.Exception.InnerException){$_.Exception.InnerException.Message}else{$_.Exception.Message}
                    $issues.Add(@{preset_id=$p.id;section=$key;line=$line;value=$raw;code='invalid_rule';message=$message})
                }
            }
        }
        $line=0
        foreach ($source in $p.domain_rule_sets) {
            $line++;$name=$source.Trim().ToLowerInvariant()
            if (!$catalog.ContainsKey($name)) {$issues.Add(@{preset_id=$p.id;section='domain_rule_sets';line=$line;value=$source;code='invalid_source';message='Unknown service source'})}
            elseif ($p.enabled -and $name -notin $direct.domain_rule_sets) {$direct.domain_rule_sets+=@($name)}
        }
    }
    foreach ($key in $script:PresetRuleKeys) {$direct[$key]=@($buffers[$key].ToArray())}
    $s.direct=$direct
    # IP provider ordering remains most-specific first as in schema 1.
    $s.direct.ip_cidrs=@(if($fastNormalizer){[MukhomorRuleNormalizer]::SortCidrs([string[]]$s.direct.ip_cidrs)}else{$s.direct.ip_cidrs | Sort-Object {Get-CidrSortKey $_}})
    foreach ($domain in $s.direct.rule_set_exclusions) {
        # Reuse the normalized sets: walking the domain's label ancestors avoids
        # scanning every suffix for every exception (up to 30000 saved rules).
        $shadowed=$seen.domains.Contains($domain)
        $suffix=$domain
        while (!$shadowed) {
            if ($seen.domain_suffixes.Contains($suffix)) {$shadowed=$true;break}
            $dot=$suffix.IndexOf('.')
            if ($dot -lt 0) {break}
            $suffix=$suffix.Substring($dot+1)
        }
        if ($shadowed) {$issues.Add($exclusionOrigins[$domain])}
    }
    $seeds=@();$line=0
    foreach ($raw in $s.dns_update.seed_hosts) {
        $line++
        try {
            $seed=Normalize-Domain $raw
            if (!(Test-DirectDomain $seed $s)) {throw 'DNS seed must match an active domain exception'}
            if ($seed -notin $seeds) {$seeds+=@($seed)}
        } catch {$issues.Add(@{preset_id='';section='seed_hosts';line=$line;value=$raw;code='invalid_seed';message=$_.Exception.Message})}
    }
    $s.dns_update.seed_hosts=$seeds
    return @{settings=$s;issues=@($issues.ToArray())}
}

function Get-EffectiveSplitSettings($Settings) {return (Get-RuleSettingsProjection $Settings).settings}
function Get-RuleSettingsIssues($Settings) {return ,@((Get-RuleSettingsProjection $Settings).issues)}

function Select-RuleSettingsIssues($Issues) {
    # Raw values also occur in parser messages, and PowerShell HTML-escapes them.
    # Bound actual serialized bytes as well as count; full text stays in settings.
    $selected=New-Object 'Collections.Generic.List[object]';$bytes=2
    foreach ($issue in $Issues) {
        if ($selected.Count -ge 200) {break}
        $size=[Text.Encoding]::UTF8.GetByteCount(($issue|ConvertTo-Json -Depth 40 -Compress))+1
        if ($bytes+$size -gt 524288) {break}
        $selected.Add($issue);$bytes+=$size
    }
    return ,@($selected.ToArray())
}
