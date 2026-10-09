param([Parameter(Mandatory=$true)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -Force -DisableNameChecking
Import-Module (Join-Path $PSScriptRoot 'SplitWindows.psm1') -Force -DisableNameChecking

function Read-ProfileIndex {
    $path=Join-Path $Root 'private\profiles.json'
    $index=if (Test-Path -LiteralPath $path) {Read-JsonFile $path} else {@{schema=1;selected='';autoconnect=$false;profiles=@()}}
    if ($index.schema -ne 1 -or $index.profiles -isnot [array] -or $index.profiles.Count -gt 512) {throw 'Invalid profile library'}
    $ids=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($profile in $index.profiles) {
        if ($profile.id -notmatch '^[a-f0-9]{32}$' -or !$ids.Add($profile.id) -or !$profile.name -or $profile.name.Length -gt 80 -or $profile.name -match '[\x00-\x1f]') {throw 'Invalid profile library entry'}
        if ($profile.ContainsKey('format') -and $profile.format -notin @('conf','node')) {throw 'Invalid profile storage format'}
        if ($profile.ContainsKey('protocol') -and (!$profile.protocol -or $profile.protocol.Length -gt 40 -or $profile.protocol -match '[\x00-\x1f]')) {throw 'Invalid profile protocol label'}
    }
    if ($index.selected -and !$ids.Contains($index.selected)) {throw 'Selected server is missing from the profile library'}
    return $index
}
function Save-ProfileIndex($Index) {Write-AtomicJson (Join-Path $Root 'private\profiles.json') $Index}
function Profile-Path([string]$Id, [string]$Format='conf') {
    if ($Id -notmatch '^[a-f0-9]{32}$') {throw 'Invalid profile ID'}
    if ($Format -notin @('conf','node')) {throw 'Invalid profile storage format'}
    $extension=if ($Format -eq 'node') {'json'} else {'conf'}
    return Join-Path $Root "private\profiles\$Id.$extension"
}
function Indexed-ProfilePath($Index, [string]$Id) {
    if ($Id -notmatch '^[a-f0-9]{32}$') {throw 'Invalid profile ID'}
    $target=@($Index.profiles | Where-Object {$_.id -eq $Id})
    if ($target.Count -ne 1) {throw 'Server not found'}
    return Profile-Path $Id $(if ($target[0].ContainsKey('format')) {$target[0].format} else {'conf'})
}
function Profile-Name([string]$Name) {
    $name=$Name.Trim()
    if (!$name -or $name.Length -gt 80 -or $name -match '[\x00-\x1f]') {throw 'Server name must contain 1 to 80 printable characters'}
    return $name
}
function Get-Snapshot {
    param($ValidatedSettings=$null)
    $index=Read-ProfileIndex
    $status=Get-SplitStatus $Root
    $settings=if($null -ne $ValidatedSettings){$ValidatedSettings}else{Get-SplitSettings $Root}
    return @{profiles=@($index.profiles | ForEach-Object {@{id=$_.id;name=$_.name;protocol=$(if ($_.ContainsKey('protocol')) {$_.protocol} else {'WireGuard / AmneziaWG'})}});selected=$index.selected;autoconnect=$index.autoconnect;running=($status.running -and !$status.starting);starting=$status.starting;dns_recovery_pending=($status.dns_recovery_pending -and !$status.running -and !$status.starting);dns_hosts=$status.dns_hosts;dns_pending=$status.dns_refresh_pending;dns_failed=$status.dns_failed_hosts;settings=$settings}
}

function Get-FailureDetails($Failure, [string]$Action) {
    $invocation=$Failure.InvocationInfo
    $scriptName=if ($invocation) { [IO.Path]::GetFileName([string]$invocation.ScriptName) } else { '' }
    if ($scriptName -notin @('NativeBridge.ps1','SplitVpn.psm1','SplitWindows.psm1','ProfileImport.psm1')) { $scriptName='' }
    $command=if ($invocation -and $invocation.MyCommand) { [string]$invocation.MyCommand.Name } else { '' }
    if ($command -notmatch '^[A-Za-z][A-Za-z0-9-]{0,100}$') { $command='' }
    if ($Action -notin @('Init','Snapshot','Import','Select','Rename','Remove','Connect','Disconnect','Exit','Autoconnect','ApplySettings','Update','UpdateDns','Diagnose')) { $Action='Unknown' }
    $rollback=@()
    if ($Failure.Exception.Data.Contains('MukhomorRollbackFailures')) {
        $rollback=@($Failure.Exception.Data['MukhomorRollbackFailures'] | Where-Object { $_ -in @('dns','core','core-direct','profiles') } | Sort-Object -Unique)
    }
    if ($Failure.Exception.Data.Contains('MukhomorProfileRollbackFailure')) {$rollback+=@('profiles')}
    # Invocation lines and raw FQIDs can contain private config values. Retain
    # only source location, typed codes and our own recovery-stage labels.
    $providerCode=if ([string]$Failure.FullyQualifiedErrorId -match '(?i)\b0x[0-9a-f]{8}\b') { $Matches[0].ToUpperInvariant() } else { '' }
    return @{schema=1;action=$Action;exception_type=$Failure.Exception.GetType().FullName;hresult=$Failure.Exception.HResult.ToString('X8');provider_code=$providerCode;command=$command;script=$scriptName;line=$(if ($invocation -and $scriptName) {[int]$invocation.ScriptLineNumber} else {0});rollback_failures=$rollback;utc=[datetime]::UtcNow.ToString('o')}
}

$request=@{}
try {
    $raw=[Console]::In.ReadToEnd()
    if ($raw.Length -gt 4194304) {throw 'Request is too large'}
    $request=Convert-ToMap ($raw | ConvertFrom-Json)
    if (!(Test-Path -LiteralPath (Join-Path $Root 'settings.json'))) {
        [IO.Directory]::CreateDirectory($Root) | Out-Null
        [IO.File]::Copy((Join-Path $PSScriptRoot 'assets\settings.default.json'),(Join-Path $Root 'settings.json'),$false)
    }
    Initialize-SplitRoot $Root
    [IO.Directory]::CreateDirectory((Join-Path $Root 'private\profiles')) | Out-Null
    $index=Read-ProfileIndex
    $recoveryError=''; $importedCount=0
    switch ([string]$request.action) {
        'Init' {
            # A crashed service can leave a DNS snapshot. Recover before retrying.
            try {
                $sessionPath=Join-Path $Root 'runtime\session.json'
                $owned=Get-OwnedCore $Root
                if($owned -and (Test-Path -LiteralPath $sessionPath) -and (Read-JsonFile $sessionPath).phase -ne 'running'){Stop-SplitSession $Root}
                elseif (!$owned -and (Test-Path -LiteralPath (Join-Path $Root 'runtime\dns-backup.json'))) {Restore-SessionDns $Root}
            } catch {$recoveryError=$_.Exception.Message}
        }
        'Snapshot' { } # Read-only refresh after a failed operation; never reconnects.
        'Import' {
            if ($request.content -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($request.content) -gt 524288) {throw 'Profile exceeds 512 KiB'}
            $name=Profile-Name $request.name
            $format=if ($request.ContainsKey('format')) {[string]$request.format} else {'auto'}
            $parsed=@(Convert-SplitProfileText $request.content $format $name)
            if (!$parsed.Count -or $parsed.Count -gt 128 -or $index.profiles.Count+$parsed.Count -gt 512) {throw 'Import supports up to 128 servers and a library of up to 512 servers'}
            Test-SplitProfileProxies $Root @($parsed | ForEach-Object {$_.proxy})
            $activateFirst=![bool]$index.selected
            $oldIndexText=if (Test-Path -LiteralPath (Join-Path $Root 'private\profiles.json')) {[IO.File]::ReadAllText((Join-Path $Root 'private\profiles.json'))} else {$null}
            $paths=New-Object 'Collections.Generic.List[string]'
            try {
                $firstId=''; $firstPath=''
                foreach ($entry in $parsed) {
                    $id=[guid]::NewGuid().ToString('N'); $path=Profile-Path $id 'node'
                    $paths.Add($path)
                    Write-AtomicJson $path @{schema=1;proxy=$entry.proxy}
                    $index.profiles+=@(@{id=$id;name=(Profile-Name $entry.name);format='node';protocol=$entry.protocol})
                    if (!$firstId) {$firstId=$id;$firstPath=$path}
                }
                if (!$index.selected) {$index.selected=$firstId}
                Save-ProfileIndex $index
                # Existing selection and its live connection remain authoritative.
                # The first import activates only its first server, without connecting.
                if ($activateFirst) {Import-SplitProfile $Root $firstPath}
                $importedCount=$parsed.Count
            } catch {
                $failure=$_; $rollbackFailed=$false
                try {
                    if ($oldIndexText) {Write-AtomicText (Join-Path $Root 'private\profiles.json') $oldIndexText}
                    else {[IO.File]::Delete((Join-Path $Root 'private\profiles.json'))}
                } catch {$rollbackFailed=$true}
                # If index restoration failed, its committed entries may still
                # reference these files. Keep them available for recovery.
                if (!$rollbackFailed) {
                    foreach ($path in $paths) {try {[IO.File]::Delete($path)} catch {$rollbackFailed=$true}}
                }
                if ($rollbackFailed) {$failure.Exception.Data['MukhomorProfileRollbackFailure']=$true}
                throw $failure
            }
        }
        'Select' {
            $target=@($index.profiles | Where-Object {$_.id -eq $request.id})
            if ($target.Count -ne 1) {throw 'Server not found'}
            if ($index.selected -ne $request.id) {
                $wasRunning=$null -ne (Get-OwnedCore $Root); $old=$index.selected
                if ($wasRunning) {Stop-SplitSession $Root}
                try {
                    Import-SplitProfile $Root (Indexed-ProfilePath $index $request.id)
                    $index.selected=$request.id; Save-ProfileIndex $index
                    if ($wasRunning) {Start-SplitSession $Root $true}
                } catch {
                    $failure=$_; $rollbackFailed=$false; $profileRestored=$false
                    if ($old) {
                        try {Import-SplitProfile $Root (Indexed-ProfilePath $index $old);$profileRestored=$true} catch {$rollbackFailed=$true}
                        $index.selected=$old
                        try {Save-ProfileIndex $index} catch {$rollbackFailed=$true}
                        if ($wasRunning -and $profileRestored -and !$rollbackFailed -and !(Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request'))) {
                            try {Start-SplitSession $Root $true} catch {
                                $stages=@('core')
                                if ($failure.Exception.Data.Contains('MukhomorRollbackFailures')) {$stages+=@($failure.Exception.Data['MukhomorRollbackFailures'])}
                                $failure.Exception.Data['MukhomorRollbackFailures']=@($stages | Sort-Object -Unique)
                            }
                        }
                    }
                    if ($rollbackFailed) {$failure.Exception.Data['MukhomorProfileRollbackFailure']=$true}
                    throw $failure
                }
            }
        }
        'Rename' {
            $target=@($index.profiles | Where-Object {$_.id -eq $request.id})
            if ($target.Count -ne 1) {throw 'Server not found'}
            $target[0].name=Profile-Name $request.name; Save-ProfileIndex $index
        }
        'Remove' {
            $path=Indexed-ProfilePath $index $request.id
            if ($index.selected -eq $request.id -and (Get-OwnedCore $Root)) {throw 'Disconnect before removing the active server'}
            $index.profiles=@($index.profiles | Where-Object {$_.id -ne $request.id})
            if ($index.selected -eq $request.id) {
                $index.selected=''
                foreach ($file in @('awg.conf','selected-profile.json','config.json','candidate.json','last-good.json','profile-info.json')) {[IO.File]::Delete((Join-Path $Root "private\$file"))}
            }
            Save-ProfileIndex $index; [IO.File]::Delete($path)
        }
        'Connect' {
            if (!$index.selected) {throw 'Import and select a server first'}
            if (!(Get-OwnedCore $Root)) {
                # The library is authoritative after a forced cancellation may
                # have interrupted a previous server switch between commits.
                Import-SplitProfile $Root (Indexed-ProfilePath $index $index.selected)
                Start-SplitSession $Root $true
            }
        }
        'Disconnect' {Stop-SplitSession $Root}
        'Exit' {Stop-SplitSession $Root}
        'Autoconnect' {$index.autoconnect=[bool]$request.enabled; Save-ProfileIndex $index}
        'ApplySettings' {
            if ($index.selected) {$s=Set-SplitConfig $Root $request.settings $true -Reload:($null -ne (Get-OwnedCore $Root)) -PassThru}
            else {$s=Get-SplitSettings $Root $request.settings;Write-AtomicJson (Join-Path $Root 'settings.json') $s}
        }
        'Update' {if (Get-OwnedCore $Root) {Update-SplitRuleSets $Root | Out-Null}; Update-SplitDns $Root -Force | Out-Null}
        'UpdateDns' {Update-SplitDns $Root | Out-Null}
        'Diagnose' {Write-SplitDiagnostics $Root | Out-Null}
        default {throw 'Unsupported operation'}
    }
    $snapshot=if($request.action -eq 'ApplySettings'){Get-Snapshot -ValidatedSettings $s}else{Get-Snapshot}
    if ($importedCount -gt 0) {$snapshot.imported_count=$importedCount}
    if($recoveryError){$snapshot.last_error=$recoveryError}
    [Console]::WriteLine((@{ok=$true;data=$snapshot} | ConvertTo-Json -Depth 40 -Compress))
    [Console]::Out.Flush()
} catch {
    $failure=$_
    $action=if ($request -is [System.Collections.IDictionary] -and $request.Contains('action')) { [string]$request.action } else { 'Unknown' }
    $details=Get-FailureDetails $failure $action
    try { Write-AtomicJson (Join-Path $Root 'runtime\last-error.json') $details } catch {}
    [Console]::WriteLine((@{ok=$false;error=$failure.Exception.Message;error_details=$details} | ConvertTo-Json -Depth 8 -Compress))
    [Console]::Out.Flush()
    exit 1
}
