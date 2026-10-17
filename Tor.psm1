Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -DisableNameChecking
$script:TorProcess=$null

function Get-SplitP2PNetwork([string]$Root) {
    $marker=Join-Path $Root 'private\tor-mode'
    if(!(Test-Path -LiteralPath $marker)){return 'vps'}
    if(([IO.File]::ReadAllText($marker)).Trim() -ceq 'i2p'){return 'i2p'}
    return 'tor'
}
function Get-SplitTorPath([string]$Root) {
    $base=if($env:MUKHOMOR_ASSETS){$env:MUKHOMOR_ASSETS}else{$Root}
    return Convert-SplitProgramPath (Join-Path $base 'bin\mukhomor-tor.exe')
}
function Get-SplitTorOptions([string]$Root) {
    $path=Join-Path $Root 'private\tor-options.json'
    if(Test-Path -LiteralPath $path){return Read-JsonFile $path}
    return @{mode='snowflake';bridges=''}
}
function Set-SplitTorOptions([string]$Root, $Options) {
    if($Options -isnot [Collections.IDictionary] -or $Options.Count -ne 2 -or !$Options.Contains('mode') -or !$Options.Contains('bridges') -or $Options.mode -isnot [string] -or $Options.bridges -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($Options.bridges) -gt 8192 -or $Options.mode -notin @('direct','obfs4','webtunnel','snowflake','snowflake-amp')){throw 'Invalid Tor bridge configuration'}
    $process=New-Object Diagnostics.Process
    $process.StartInfo.FileName=Get-SplitTorPath $Root
    $process.StartInfo.Arguments='--validate-options'
    $process.StartInfo.UseShellExecute=$false;$process.StartInfo.CreateNoWindow=$true
    $process.StartInfo.RedirectStandardInput=$true;$process.StartInfo.RedirectStandardOutput=$true;$process.StartInfo.RedirectStandardError=$true
    try {
        if(!$process.Start()){throw 'Cannot validate Tor bridge configuration'}
        $replyTask=$process.StandardOutput.ReadToEndAsync();$errorTask=$process.StandardError.ReadToEndAsync()
        $bytes=[Text.Encoding]::UTF8.GetBytes(($Options|ConvertTo-Json -Compress))
        $process.StandardInput.BaseStream.Write($bytes,0,$bytes.Length);$process.StandardInput.Close()
        if(!$process.WaitForExit(5000)){$process.Kill();throw 'Tor bridge validation timed out'}
        if($process.ExitCode -ne 0){throw 'Invalid Tor bridge configuration'}
        $text=$replyTask.GetAwaiter().GetResult();if($text.Length -gt 32768){throw 'Invalid Tor bridge configuration'}
        try{$reply=Convert-ToMap ($text|ConvertFrom-Json)}catch{throw 'Invalid Tor bridge configuration'}
        if(!$reply.ContainsKey('options')){throw 'Invalid Tor bridge configuration'}
        Write-AtomicJson (Join-Path $Root 'private\tor-options.json') $reply.options
    } finally {$process.Dispose()}
}
function Test-SplitTor {return $script:TorProcess -and !$script:TorProcess.HasExited}
function Stop-SplitTor {
    if($script:TorProcess){
        try {
            try {$script:TorProcess.StandardInput.Close()} catch {}
            if(!$script:TorProcess.WaitForExit(1000)){$script:TorProcess.Kill();$script:TorProcess.WaitForExit(1000)|Out-Null}
        }
        finally {$script:TorProcess.Dispose();$script:TorProcess=$null}
    }
}
function Start-SplitTor([string]$Root, [string]$Network='tor') {
    Stop-SplitTor
    if($Network -notin @('tor','i2p')){throw 'Unsupported P2P connector'}
    $path=Get-SplitTorPath $Root
    if($Network -eq 'i2p'){$path=Join-Path (Split-Path -Parent $path) 'mukhomor-pt.exe'}
    if(!(Test-Path -LiteralPath $path -PathType Leaf)){throw 'Bundled P2P transport is missing; reinstall Mukhomor'}
    $adapter=$null
    foreach($route in @(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction Stop|Sort-Object RouteMetric)){
        $candidate=Get-NetAdapter -InterfaceIndex $route.InterfaceIndex -ErrorAction SilentlyContinue
        if($candidate -and $candidate.HardwareInterface -and $candidate.Status -eq 'Up'){$adapter=$candidate;break}
    }
    if(!$adapter){throw 'No physical default route for P2P'}
    $user=[guid]::NewGuid().ToString('N')+[guid]::NewGuid().ToString('N')
    $password=[guid]::NewGuid().ToString('N')+[guid]::NewGuid().ToString('N')
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=$path;$info.UseShellExecute=$false;$info.CreateNoWindow=$true
    $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    $process=New-Object Diagnostics.Process;$process.StartInfo=$info
    $started=$false
    try {
        try {$started=$process.Start()} catch {throw 'Cannot start bundled P2P transport'}
        if(!$started){throw 'Cannot start bundled P2P transport'}
        $script:TorProcess=$process
        $errors=$process.StandardError.ReadToEndAsync()
        $init=@{state_dir=(Join-Path $Root 'private\tor\state');cache_dir=(Join-Path $Root 'private\tor\cache');username=$user;password=$password;interface=[string]$adapter.ifIndex;tor_options=(Get-SplitTorOptions $Root)}
        if($Network -eq 'i2p'){$init=@{state_dir=(Join-Path $Root 'private\i2p\state');interface=[string]$adapter.ifIndex;token=$user;password=$password;mode='i2p';bridges=@()}}
        # Windows PowerShell 5.1 uses .NET Framework, which has no
        # ProcessStartInfo.StandardInputEncoding. Write explicit UTF-8 bytes
        # without a BOM; keep stdin open so EOF still supervises the helper.
        $bytes=(New-Object Text.UTF8Encoding($false)).GetBytes(($init|ConvertTo-Json -Compress)+"`n")
        $process.StandardInput.BaseStream.Write($bytes,0,$bytes.Length)
        $process.StandardInput.BaseStream.Flush()
        $ready=$process.StandardOutput.ReadLineAsync()
        $clock=[Diagnostics.Stopwatch]::StartNew()
        $budget=if($Network -eq 'i2p'){480000}elseif($init.tor_options.mode -eq 'snowflake-amp'){420000}else{300000}
        while(!$ready.IsCompleted -and $clock.ElapsedMilliseconds -lt $budget){
            if(Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')){throw 'Startup cancelled'}
            if($process.HasExited){throw 'P2P bootstrap failed; the selected network may be unavailable or blocked'}
            Start-Sleep -Milliseconds 50
        }
        if(!$ready.IsCompleted){throw 'P2P bootstrap timed out; try another connector or Tor bridge'}
        $text=$ready.Result
        if(!$text -or $text.Length -gt 4096){throw 'P2P transport initialization failed'}
        try {$reply=Convert-ToMap ($text|ConvertFrom-Json)} catch {throw 'P2P transport initialization failed'}
        if($reply -isnot [Collections.IDictionary] -or !$reply.ContainsKey('ready') -or $reply.ready -isnot [bool] -or !$reply.ready -or !$reply.ContainsKey('port') -or ($reply.port -isnot [int] -and $reply.port -isnot [long]) -or $reply.port -lt 1 -or $reply.port -gt 65535 -or $process.HasExited){throw 'P2P transport initialization failed'}
        return @{network=$Network;server='127.0.0.1';port=[int]$reply.port;addresses=@('127.0.0.1');selected_address='127.0.0.1';interface_name=$adapter.Name;interface_guid=([guid]$adapter.InterfaceGuid).ToString('B');tor_proxy=@{name='AWG';type='socks5';server='127.0.0.1';port=[int]$reply.port;username=$user;password=$password;udp=$false}}
    } catch {if($started){Stop-SplitTor}else{$process.Dispose()};throw}
}
Export-ModuleMember -Function Get-SplitP2PNetwork,Get-SplitTorPath,Get-SplitTorOptions,Set-SplitTorOptions,Start-SplitTor,Stop-SplitTor,Test-SplitTor
