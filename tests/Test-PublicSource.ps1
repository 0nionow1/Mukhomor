Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
# Inspect only files Git would publish. Do not read ignored profiles or parent folders.
$packaged=!(Test-Path -LiteralPath (Join-Path $package '.git'))
if($packaged){
    $manifestPath=Join-Path $package 'release-manifest.json'
    if(!(Test-Path -LiteralPath $manifestPath)){throw 'Expected a Git checkout or reviewed source package manifest'}
    $manifest=[IO.File]::ReadAllText($manifestPath)|ConvertFrom-Json
    $files=@($manifest.files | ForEach-Object {$_.path.Replace('\','/')})
}else{
    $files=@(& git.exe -C $package -c core.quotepath=false ls-files --cached --others --exclude-standard | Sort-Object -Unique)
    if($LASTEXITCODE -ne 0){throw 'Cannot enumerate public Git files'}
}
if(!$files.Count){throw 'No public source files'}
$issues=New-Object 'Collections.Generic.List[string]'
$checked=0
foreach($relative in $files){
    if($packaged -and $relative -eq 'Source/mihomo-v1.19.32-source.zip'){
        if((Get-FileHash -LiteralPath (Join-Path $package $relative) -Algorithm SHA256).Hash.ToLowerInvariant() -ne '135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1'){$issues.Add($relative+': source checksum mismatch')}
        continue
    }
    if($relative -match '(?i)(^|/)(private|runtime|dist|release-staging|target|graphify-out|bin|Source)/|\.(conf|ovpn|pem|key|pfx|p12|uri|sub|log|db|zip)($|\.)|(^|/)(\.env[^/]*|settings\.json|profiles\.json|api-token\.txt|owner\.sid|installation\.json)$'){$issues.Add($relative+': private/generated path');continue}
    if($relative -match '(?i)\.(yaml|yml)$' -and $relative -notin @('.github/workflows/ci.yml','.github/workflows/release.yml')){$issues.Add($relative+': unreviewed YAML');continue}
    $path=[IO.Path]::GetFullPath((Join-Path $package $relative))
    if(!$path.StartsWith($package+'\',[StringComparison]::OrdinalIgnoreCase) -or !(Test-Path -LiteralPath $path -PathType Leaf)){$issues.Add($relative+': invalid path');continue}
    if($relative -match '(?i)\.(png|ttf)$'){continue}
    $text=[IO.File]::ReadAllText($path)
    # Patterns identify credentials, not their values, so failures never print secrets.
    $checks=@{
        'GitHub token'='\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})\b'
        'private key block'='-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=\r\n]{64,}'
        'AWS access key'='\b(?:AKIA|ASIA)[A-Z0-9]{16}\b'
        'local build path'='(?i)(?:D:[/\\]VPS[/\\]|C:[/\\]Users[/\\](?!Public\b|Default\b|<)[^\s"''<>/\\]+)'
        'WireGuard credential'='(?im)^\s*(?:PrivateKey|PresharedKey)\s*=\s*[A-Za-z0-9+/]{43}='
    }
    foreach($entry in $checks.GetEnumerator()){
        if($text -match $entry.Value){
            # Only the canonical 32-byte repeated-A test key is public.
            if($entry.Key -eq 'WireGuard credential' -and $relative.StartsWith('tests/')){
                $matchesFound=[regex]::Matches($text,$entry.Value)
                $synthetic=[Convert]::ToBase64String([byte[]](1..32|ForEach-Object {65}))
                if(@($matchesFound | Where-Object {!$_.Value.TrimEnd().EndsWith($synthetic,[StringComparison]::Ordinal)}).Count -eq 0){continue}
            }
            $issues.Add($relative+': '+$entry.Key)
        }
    }
    $checked++
}
if($issues.Count){throw ('Publication scan failed (paths and categories only): '+($issues -join '; '))}
Write-Host ('PASS: '+$files.Count+' public paths and '+$checked+' text files checked; no ignored data read')
