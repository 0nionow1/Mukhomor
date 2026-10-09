Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -DisableNameChecking

function Test-SplitAdmin {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    return (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-OwnedCore([string]$Root) {
    $path=Join-Path $Root 'runtime\session.json'
    if (!(Test-Path -LiteralPath $path)) { return $null }
    $session=Read-JsonFile $path
    $process=Get-Process -Id $session.pid -ErrorAction SilentlyContinue
    if (!$process) { return $null }
    if ($process.Path -ne (Get-CorePath $Root) -or $process.StartTime.ToUniversalTime().ToString('o') -ne $session.started) { return $null }
    return $process
}

function Assert-NoOtherVpn([string]$Root) {
    $tunnels=@(Get-Service | Where-Object { $_.Status -eq 'Running' -and $_.Name -match '^(AmneziaWG|WireGuard)Tunnel\$' })
    if ($tunnels.Count) { throw ('Disconnect the current AmneziaWG/WireGuard tunnel before starting Mukhomor: ' + (($tunnels | ForEach-Object { $_.Name }) -join ', ')) }
    foreach ($name in @('sing-box','amneziawg-go','xray')) {
        if (Get-Process -Name $name -ErrorAction SilentlyContinue) { throw "Another VPN engine is running ($name). Disconnect it first." }
    }
    $owned=Get-OwnedCore $Root
    foreach ($p in @(Get-Process -Name 'mihomo*' -ErrorAction SilentlyContinue)) {
        if (!$owned -or $p.Id -ne $owned.Id) { throw 'Another Mihomo process is running. Stop it first.' }
    }
}

function Invoke-NetshDns([int]$Index, [string]$Family, [string]$Source, [string[]]$Servers=@(), [int]$TimeoutMilliseconds=5000) {
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $netsh=Join-Path $env:SystemRoot 'System32\netsh.exe'
    $invoke={
        param([string[]]$arguments)
        $remaining=[Math]::Min(5000,$TimeoutMilliseconds-[int]$clock.ElapsedMilliseconds)
        if ($remaining -le 0) { throw "DNS command deadline exceeded on adapter $Index" }
        $result=Invoke-SplitBoundedProcess $netsh $arguments $remaining 'Windows DNS configuration'
        if ($result.exit_code -ne 0) { throw "Unable to configure $Family DNS on adapter $Index" }
    }
    if ($Source -eq 'dhcp') {
        & $invoke @('interface',$Family,'set','dnsservers',"name=$Index",'source=dhcp')
        return
    }
    if (!$Servers.Count) { throw 'Static DNS servers missing' }
    & $invoke @('interface',$Family,'set','dnsservers',"name=$Index",'source=static',"address=$($Servers[0])",'validate=no')
    for ($i=1;$i -lt $Servers.Count;$i++) {
        & $invoke @('interface',$Family,'add','dnsservers',"name=$Index", "address=$($Servers[$i])", "index=$($i+1)", 'validate=no')
    }
}

function Set-SessionDns([string]$Root) {
    $snapshotPath=Join-Path $Root 'runtime\dns-backup.json'
    if (Test-Path -LiteralPath $snapshotPath) { throw 'A previous DNS snapshot exists. Run STOP.cmd to recover it first.' }
    $adapters=@(Get-NetAdapter | Where-Object { $_.Status -eq 'Up' -and $_.Name -ne 'Mukhomor' })
    $rows=@()
    foreach ($adapter in $adapters) {
        $routes=@(Get-NetRoute -InterfaceIndex $adapter.ifIndex -ErrorAction SilentlyContinue | Where-Object { $_.DestinationPrefix -in @('0.0.0.0/0','::/0') })
        if (!$routes.Count) { continue }
        $guid=([guid]$adapter.InterfaceGuid).ToString('B')
        foreach ($family in @('ipv4','ipv6')) {
            $protocol=if ($family -eq 'ipv4') { 'Tcpip' } else { 'Tcpip6' }
            $registry="HKLM:\SYSTEM\CurrentControlSet\Services\$protocol\Parameters\Interfaces\$guid"
            $item=Get-ItemProperty -LiteralPath $registry -ErrorAction SilentlyContinue
            $manual=if ($item -and $item.PSObject.Properties.Name -contains 'NameServer') { [string]$item.NameServer } else { '' }
            $servers=@($manual -split '[,;\s]+' | Where-Object { $_ })
            $rows+=@(@{guid=$guid;name=$adapter.Name;family=$family;source=$(if ($servers.Count) { 'static' } else { 'dhcp' });servers=$servers})
        }
    }
    if (!$rows.Count) { throw 'No active default network adapter was found' }
    Write-AtomicJson $snapshotPath @{schema=1;adapters=$rows}
    $clock=[Diagnostics.Stopwatch]::StartNew()
    try {
        foreach ($row in $rows) {
            Assert-SplitNotCancelled $Root
            if ($clock.ElapsedMilliseconds -ge 15000) { throw 'DNS setup deadline exceeded' }
            $adapter=Get-NetAdapter | Where-Object { ([guid]$_.InterfaceGuid).ToString('B') -eq $row.guid } | Select-Object -First 1
            if (!$adapter) { throw 'Network adapter disappeared during DNS setup' }
            # Mihomo reserves the address AFTER its TUN interface for DNS:
            # listener/sing_tun/server.go uses a.Addr().Next(), not the local IP.
            $address=if ($row.family -eq 'ipv4') { '198.18.0.2' } else { 'fdfe:dcba:9876::2' }
            $remaining=15000-[int]$clock.ElapsedMilliseconds
            if ($remaining -le 0) { throw 'DNS setup deadline exceeded' }
            Invoke-NetshDns $adapter.ifIndex $row.family 'static' @($address) -TimeoutMilliseconds ([Math]::Min(5000,$remaining))
        }
    } catch {
        $failure=$_
        try { Restore-SessionDns $Root }
        catch { $failure.Exception.Data['MukhomorRollbackFailures']=@('dns') }
        throw $failure
    }
}

function Restore-SessionDns([string]$Root, [int]$BudgetMilliseconds=15000) {
    if ($BudgetMilliseconds -lt 1 -or $BudgetMilliseconds -gt 15000) { throw 'Invalid DNS recovery deadline' }
    $path=Join-Path $Root 'runtime\dns-backup.json'
    if (!(Test-Path -LiteralPath $path)) { return }
    $backup=Read-JsonFile $path
    if ($backup.schema -ne 1) { throw 'Invalid DNS backup; recovery was not attempted' }
    $failed=@()
    $clock=[Diagnostics.Stopwatch]::StartNew()
    foreach ($row in $backup.adapters) {
        try {
            if ($clock.ElapsedMilliseconds -ge $BudgetMilliseconds) { throw 'DNS recovery deadline exceeded; remaining adapters are retained for retry' }
            $adapter=Get-NetAdapter -IncludeHidden | Where-Object { ([guid]$_.InterfaceGuid).ToString('B') -eq $row.guid } | Select-Object -First 1
            if (!$adapter) { throw 'Adapter is currently unavailable' }
            $remaining=$BudgetMilliseconds-[int]$clock.ElapsedMilliseconds
            if ($remaining -le 0) { throw 'DNS recovery deadline exceeded; remaining adapters are retained for retry' }
            Invoke-NetshDns $adapter.ifIndex $row.family $row.source @($row.servers) -TimeoutMilliseconds ([Math]::Min(5000,$remaining))
        } catch { $failed+=@($row) }
    }
    if ($failed.Count) {
        Write-AtomicJson $path @{schema=1;adapters=$failed}
        throw 'Some DNS settings could not be restored. Reconnect the adapter and run STOP.cmd again.'
    }
    [IO.File]::Delete($path)
}

function Stop-OwnedCore([string]$Root) {
    $core=Get-OwnedCore $Root
    if ($core) {
        try { Invoke-CoreApi $Root '/configs' 'PATCH' @{tun=@{enable=$false}} -TimeoutSeconds 1 | Out-Null } catch {}
        finally { if (!$core.HasExited) { $core.Kill(); if (!$core.WaitForExit(1000)) { throw 'Mihomo did not terminate after disconnect' } } }
    }
    $session=Join-Path $Root 'runtime\session.json'
    if (Test-Path -LiteralPath $session) { [IO.File]::Delete($session) }
}

function Resolve-SplitEndpoint([string]$Server) {
    $literal=$null
    if ([Net.IPAddress]::TryParse($Server,[ref]$literal)) { return @($literal) }
    else {
        $lookup=[Net.Dns]::GetHostAddressesAsync($Server)
        if (!$lookup.Wait(5000)) { throw 'VPN endpoint DNS lookup timed out before TUN startup' }
        return @($lookup.Result)
    }
}

function Get-SplitTransport([string]$Root, [scriptblock]$Resolver=${function:Resolve-SplitEndpoint}) {
    $proxy=Get-SelectedSplitProxy $Root
    $addresses=@((& $Resolver $proxy.server) | Sort-Object @{Expression={if ($_.AddressFamily -eq 'InterNetwork') {0} else {1}}},@{Expression={$_.ToString()}} -Unique)
    if (!$addresses.Count) { throw 'VPN endpoint has no IP addresses' }
    foreach ($address in $addresses) {
        $routes=@(Find-NetRoute -RemoteIPAddress $address.ToString() -ErrorAction SilentlyContinue | Where-Object { $_.PSObject.Properties.Name -contains 'NextHop' })
        foreach ($route in $routes) {
            $adapter=Get-NetAdapter -InterfaceIndex $route.InterfaceIndex -ErrorAction SilentlyContinue
            if (!$adapter -or $adapter.Status -ne 'Up' -or $adapter.Name -eq 'Mukhomor' -or $adapter.InterfaceDescription -match 'WireGuard|Amnezia|Wintun') { continue }
            return @{server=$proxy.server;port=$proxy.port;addresses=@($addresses | ForEach-Object {$_.ToString()});selected_address=$address.ToString();interface_name=$adapter.Name;interface_guid=([guid]$adapter.InterfaceGuid).ToString('B')}
        }
    }
    throw 'No ordinary network adapter can reach the VPN endpoint. Disconnect other VPNs and check the network.'
}

function Assert-SplitAwgHealth([string]$Root, [string]$Phase) {
    foreach ($url in @('https://www.gstatic.com/generate_204','https://cp.cloudflare.com/generate_204')) {
        if (Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) { throw 'Startup cancelled' }
        try {
            $result=Invoke-CoreApi $Root ('/proxies/AWG/delay?timeout=7000&url='+[Uri]::EscapeDataString($url))
            Assert-SplitNotCancelled $Root
            if ($result.delay -ge 0) { return }
        } catch {}
    }
    throw "VPN connection check failed ($Phase). Startup will be rolled back; see runtime/logs/core.log."
}

function Assert-SplitNotCancelled([string]$Root) {
    if (Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) { throw 'Startup cancelled' }
}

function Set-SplitTunFirewall([string]$Root) {
    $program=Get-CorePath $Root
    $sha=[Security.Cryptography.SHA256]::Create()
    try { $id=([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($program.ToLowerInvariant())))).Replace('-','').Substring(0,16) }
    finally { $sha.Dispose() }
    $name='Mukhomor-TUN-'+$id
    $rule=Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue
    $parameters=@{Name=$name;Enabled='True';Direction='Inbound';Action='Allow';Profile='Any';Program=$program;InterfaceAlias='Mukhomor';Protocol='Any';EdgeTraversalPolicy='Block'}
    # System-stack packets need firewall permission. This grants it only to
    # this binary on our virtual interface, never to physical LAN interfaces.
    if ($rule) { Set-NetFirewallRule @parameters | Out-Null }
    else { New-NetFirewallRule @parameters -DisplayName 'Mukhomor: Mihomo on TUN only' -Group 'Mukhomor' | Out-Null }
}

function Assert-SplitTunDns([string]$Server='198.18.0.2',[int]$Port=53) {
    $random=[Security.Cryptography.RandomNumberGenerator]::Create()
    $id=New-Object byte[] 2
    try {$random.GetBytes($id)} finally {$random.Dispose()}
    # A real UDP DNS query to the intercepted peer checks the Windows route,
    # firewall and DNS handler, independently of the controller health API.
    $query=[byte[]]@($id[0],$id[1],1,0,0,1,0,0,0,0,0,0,7,101,120,97,109,112,108,101,3,99,111,109,0,0,1,0,1)
    $client=New-Object Net.Sockets.UdpClient([Net.Sockets.AddressFamily]::InterNetwork)
    try {
        $client.Client.ReceiveTimeout=5000
        $client.Connect($Server,$Port)
        $client.Send($query,$query.Length) | Out-Null
        $peer=New-Object Net.IPEndPoint([Net.IPAddress]::Any,0)
        $response=$client.Receive([ref]$peer)
        if($response.Length -lt 12 -or $response[0] -ne $id[0] -or $response[1] -ne $id[1] -or ($response[2] -band 128) -eq 0 -or ($response[3] -band 15) -ne 0 -or ($response[6]*256+$response[7]) -lt 1) {throw 'Invalid DNS response from TUN'}
    } catch {throw 'Windows TUN DNS check failed after DNS settings were applied. Startup will be rolled back.'}
    finally {$client.Dispose()}
}

function Start-SplitSession([string]$Root, [bool]$TunEnabled=$true) {
    Assert-SplitNotCancelled $Root
    if ($TunEnabled -and !(Test-SplitAdmin)) { throw 'Run START.cmd as administrator to use TUN' }
    if ($TunEnabled -and !(Test-Path -LiteralPath (Join-Path (Split-Path -Parent (Get-CorePath $Root)) 'wintun.dll'))) { throw 'Signed Wintun DLL is missing' }
    Assert-NoOtherVpn $Root
    if (Get-OwnedCore $Root) { throw 'Mukhomor is already running' }
    if (Test-Path -LiteralPath (Join-Path $Root 'runtime\dns-backup.json')) { Restore-SessionDns $Root }
    $s=Get-SplitSettings $Root
    $transport=Get-SplitTransport $Root
    Assert-SplitNotCancelled $Root
    Set-SplitConfig $Root $s $false -Transport $transport
    $configPath=Join-Path $Root 'private\config.json'
    Assert-SplitNotCancelled $Root
    $core=$null
    try {
        $core=Start-Process -FilePath (Get-CorePath $Root) -ArgumentList @('-d',('"'+$Root+'"'),'-f',('"'+$configPath+'"')) -WorkingDirectory $Root -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $Root 'runtime\logs\core.log') -RedirectStandardError (Join-Path $Root 'runtime\logs\core-error.log')
        $session=@{pid=$core.Id;started=$core.StartTime.ToUniversalTime().ToString('o');mode=$(if ($TunEnabled) {'tun'} else {'proxy'});phase='starting';transport=$transport}
        Write-AtomicJson (Join-Path $Root 'runtime\session.json') $session
        $ready=$false
        $readyClock=[Diagnostics.Stopwatch]::StartNew()
        while ($readyClock.ElapsedMilliseconds -lt 12000) {
            Assert-SplitNotCancelled $Root
            if ($core.HasExited) { throw 'Mihomo exited during startup; see runtime/logs/core-error.log' }
            try { Invoke-CoreApi $Root '/version' -TimeoutSeconds 1 | Out-Null; $ready=$true; break } catch { Start-Sleep -Milliseconds 100 }
        }
        if (!$ready) { throw 'Mihomo controller did not start' }
        Assert-SplitAwgHealth $Root 'before TUN'
        if (Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) { throw 'Startup cancelled' }
        if ($TunEnabled) {
            Set-SplitConfig $Root $s $true -Reload -Transport $transport
            $adapterReady=$false
            for ($i=0;$i -lt 40;$i++) {
                $adapter=Get-NetAdapter -Name 'Mukhomor' -ErrorAction SilentlyContinue
                if ($adapter -and $adapter.Status -eq 'Up') { $adapterReady=$true; break }
                if (Test-Path -LiteralPath (Join-Path $Root 'runtime\stop.request')) { throw 'Startup cancelled' }
                Start-Sleep -Milliseconds 100
            }
            if (!$adapterReady) { throw 'TUN adapter did not start. DNS was not changed; inspect the core log.' }
            Set-SplitTunFirewall $Root
            # A working proxy before TUN does not prove that outer UDP survives
            # the new routes. Verify it again before touching adapter DNS.
            Assert-SplitAwgHealth $Root 'after TUN routing'
            Assert-SplitNotCancelled $Root
            Set-SessionDns $Root
            Assert-SplitTunDns
        }
        Assert-SplitNotCancelled $Root
        $session.phase='running'; Write-AtomicJson (Join-Path $Root 'runtime\session.json') $session
        Write-SplitLog $Root ("Session started; VPN health checks passed; outer transport bound to "+$transport.interface_name)
    } catch {
        $failure=$_
        $rollback=New-Object 'Collections.Generic.List[string]'
        if ($failure.Exception.Data.Contains('MukhomorRollbackFailures')) {
            foreach ($stage in @($failure.Exception.Data['MukhomorRollbackFailures'])) { $rollback.Add([string]$stage) }
        }
        try { Restore-SessionDns $Root } catch { $rollback.Add('dns') }
        try { Stop-OwnedCore $Root } catch { $rollback.Add('core') }
        # The process was created by this call, so it remains owned even when
        # registering the session failed. Cleanup errors must not replace the
        # original startup ErrorRecord or prevent the remaining cleanup steps.
        try {
            if ($core -and !$core.HasExited) {
                $core.Kill()
                if (!$core.WaitForExit(1000)) { throw 'Owned startup process did not stop' }
            }
        } catch { $rollback.Add('core-direct') }
        if ($rollback.Count) { $failure.Exception.Data['MukhomorRollbackFailures']=@($rollback | Sort-Object -Unique) }
        throw $failure
    }
}

function Stop-SplitSession([string]$Root) {
    $dnsFailure=$null
    try { Restore-SessionDns $Root } catch { $dnsFailure=$_ }
    finally { Stop-OwnedCore $Root }
    if ($dnsFailure) { throw $dnsFailure }
}

function Write-SplitDiagnostics([string]$Root) {
    $status=Get-SplitStatus $Root
    $report=@{time_utc=[datetime]::UtcNow.ToString('o');running=$status.running;starting=$status.starting;udp_connections=@();notes=@();recent_errors=@();udp_buffer_errors=0;configured_tun_stack='system'}
    $log=Join-Path $Root 'runtime\logs\core.log'
    if (Test-Path -LiteralPath $log) {
        $lines=@(Get-Content -LiteralPath $log -Encoding UTF8 -Tail 2000)
        $report.udp_buffer_errors=@($lines | Where-Object {$_ -match 'wsasend:.*buffer space|WSAENOBUFS|queue was full'}).Count
        $report.recent_errors=@($lines | Where-Object {$_ -match 'Failed to send data packets|\[UDP\].*(error|failed|timeout|resolve)|\[TUN\].*(error|failed)'} | Select-Object -Last 20 | ForEach-Object {$_ -replace 'peer\([^)]*\)','peer(redacted)'})
        if (!$status.running) { $report.notes+=@('Core is stopped; log errors are from the previous session.') }
    }
    if ($status.running) {
        try {
            $config=Invoke-CoreApi $Root '/configs'
            $report.active_tun_stack=$config.tun.stack
            $connections=Invoke-CoreApi $Root '/connections'
            $report.udp_connections=@($connections.connections | Where-Object {$_.metadata.network -eq 'udp'} | Select-Object -First 80 | ForEach-Object {
                @{process=$_.metadata.process;process_path=$_.metadata.processPath;host=$_.metadata.host;destination_ip=$_.metadata.destinationIP;destination_port=$_.metadata.destinationPort;chains=@($_.chains);rule=$_.rule;rule_payload=$_.rulePayload}
            })
        } catch { $report.notes+=@('Controller query failed: '+$_.Exception.Message) }
    }
    try {
        $report.adapters=@(Get-NetAdapter | Where-Object {$_.Status -eq 'Up'} | ForEach-Object {
            $adapter=$_
            $routes=@(Get-NetRoute -InterfaceIndex $adapter.ifIndex -ErrorAction SilentlyContinue | Where-Object {$_.DestinationPrefix -in @('0.0.0.0/0','::/0')})
            @{name=$adapter.Name;description=$adapter.InterfaceDescription;index=$adapter.ifIndex;default_route_families=@($routes | ForEach-Object {$_.AddressFamily.ToString()})}
        })
    } catch { $report.notes+=@('Adapter query unavailable.') }
    $path=Join-Path $Root 'runtime\udp-diagnostics.json'
    Write-AtomicJson $path $report
    return $path
}

function Get-SplitStatus([string]$Root) {
    $p=Get-OwnedCore $Root
    $statePath=Join-Path $Root 'runtime\dns-state.json'
    $count=0; $pending=0; $failed=0
    if (Test-Path -LiteralPath $statePath) {
        $dnsState=Read-JsonFile $statePath
        $count=@($dnsState.hosts.Values | Where-Object {$_.addresses.Count -gt 0}).Count
        $failed=@($dnsState.hosts.Values | Where-Object {$_.failures -gt 0}).Count
        if ($dnsState.ContainsKey('refresh_pending')) { $pending=$dnsState.refresh_pending.Count }
    }
    $starting=$false
    if ($p) { $starting=(Read-JsonFile (Join-Path $Root 'runtime\session.json')).phase -eq 'starting' }
    return @{running=($null -ne $p);starting=$starting;dns_hosts=$count;dns_refresh_pending=$pending;dns_failed_hosts=$failed;dns_recovery_pending=(Test-Path -LiteralPath (Join-Path $Root 'runtime\dns-backup.json'));profile_present=((Test-Path -LiteralPath (Join-Path $Root 'private\selected-profile.json')) -or (Test-Path -LiteralPath (Join-Path $Root 'private\awg.conf')))}
}

Export-ModuleMember -Function *
