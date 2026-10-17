Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'SplitVpn.psm1') -DisableNameChecking

function Invoke-SplitTrafficGuard([string]$Root, [ValidateSet('preview','arm','update','status','resolve-tun','disarm','inspect')][string]$Action, $Plan=$null) {
    $base=if($env:MUKHOMOR_ASSETS){$env:MUKHOMOR_ASSETS}else{$Root}
    $exe=Convert-SplitProgramPath (Join-Path $base 'mukhomor.exe')
    if (!(Test-Path -LiteralPath $exe -PathType Leaf)) {throw 'Traffic protection helper is missing'}
    $id=[guid]::NewGuid().ToString('N')
    $request=Join-Path $Root ('private\traffic-guard-'+$id+'.request')
    $output=Join-Path $Root ('private\traffic-guard-'+$id+'.reply')
    $arguments=@('--traffic-guard-action',$Action,'--base',(Convert-SplitProgramPath $base),'--output',$output)
    try {
        if ($null -ne $Plan) {Write-AtomicJson $request $Plan; $arguments+=@('--request',$request)}
        $result=Invoke-SplitBoundedProcess $exe $arguments 5000 'Windows traffic protection'
        if (!(Test-Path -LiteralPath $output -PathType Leaf) -or (Get-Item -LiteralPath $output).Length -gt 16384) {throw 'Traffic protection helper returned no bounded result'}
        $reply=Read-JsonFile $output
        if ($result.exit_code -ne 0 -and $reply.ContainsKey('error') -and $reply.error -eq 'WFP_HIGHER_PRIORITY_PERMIT') {
            throw 'Disconnect the other VPN: its Windows rules interfere with Mukhomor protection.'
        }
        if ($result.exit_code -ne 0) {throw ('Windows traffic protection failed: '+$(if($reply.ContainsKey('error')){[string]$reply.error}else{'helper error'}))}
        return $reply
    } finally {
        foreach ($path in @($request,$output)) {if([IO.File]::Exists($path)){[IO.File]::Delete($path)}}
    }
}
function New-SplitTrafficGuardPlan([string]$Root, $Settings, $Transport) {
    if($Transport.ContainsKey('network') -and $Transport.network -in @('tor','i2p')){
        Import-Module (Join-Path $PSScriptRoot 'Tor.psm1') -DisableNameChecking
        $options=Get-SplitTorOptions $Root
        $program=Get-SplitTorPath $Root;$kind='tor'
        if($Transport.network -eq 'i2p' -or $options.mode -ne 'direct'){
            $program=Join-Path (Split-Path -Parent $program) 'mukhomor-pt.exe'
            $kind=if($Transport.network -eq 'tor' -and $options.mode -in @('snowflake','snowflake-amp')){'tor-snowflake'}else{'tor-bridges'}
        }
        return @{schema=2;transport=$kind;core_executable=$program;tunnel_luid=[uint64]0;allow_private_lan=$false;endpoints=@()}
    }
    $proxy=Get-SelectedSplitProxy $Root
    $protocols=switch([string]$proxy.type) {
        'wireguard' {@('udp')}
        'mycelium' {@('tcp')}
        'hysteria2' {@('udp')}
        'tuic' {@('udp')}
        'ssh' {@('tcp')}
        default {@('tcp','udp')}
    }
    $endpoint=[Net.IPAddress]::Parse([string]$Transport.selected_address).ToString()
    return @{schema=1;core_executable=(Get-CorePath $Root);tunnel_luid=[uint64]0;allow_private_lan=[bool]$Settings.routing.allow_lan;endpoints=@($protocols|ForEach-Object {@{address=$endpoint;port=[int]$Transport.port;protocol=$_}})}
}
function Assert-SplitTrafficGuardResult($Reply, [bool]$RequireTun) {
    if (!$Reply.ContainsKey('armed') -or $Reply.armed -ne $true -or !$Reply.ContainsKey('verified') -or $Reply.verified -ne $true -or ($RequireTun -and (!$Reply.ContainsKey('protected') -or $Reply.protected -ne $true))) {throw 'Windows traffic protection could not be verified; traffic remains blocked'}
}
function Enable-SplitTrafficGuard([string]$Root, $Settings, $Transport) {
    $plan=New-SplitTrafficGuardPlan $Root $Settings $Transport
    # Persist recovery intent before the WFP transaction. Even a bootstrap
    # failure then leaves an explicit disconnect path and pinned endpoint.
    Write-AtomicJson (Join-Path $Root 'private\traffic-guard-transport.json') @{server=$Transport.server;port=$Transport.port;selected_address=$Transport.selected_address}
    Write-AtomicJson (Join-Path $Root 'private\traffic-guard-plan.json') $plan
    Assert-SplitTrafficGuardResult (Invoke-SplitTrafficGuard $Root 'arm' $plan) $false
}
function Complete-SplitTrafficGuard([string]$Root) {
    $plan=Read-JsonFile (Join-Path $Root 'private\traffic-guard-plan.json')
    $identity=Invoke-SplitTrafficGuard $Root 'resolve-tun'
    if (!$identity.ContainsKey('tunnel_luid') -or [uint64]$identity.tunnel_luid -eq 0) {throw 'Owned TUN identity was not verified'}
    $plan.tunnel_luid=[uint64]$identity.tunnel_luid
    Assert-SplitTrafficGuardResult (Invoke-SplitTrafficGuard $Root 'update' $plan) $true
    Write-AtomicJson (Join-Path $Root 'private\traffic-guard-plan.json') $plan
    Assert-SplitTrafficGuardResult (Invoke-SplitTrafficGuard $Root 'status' $plan) $true
}
function Disable-SplitTrafficGuard([string]$Root) {
    $path=Join-Path $Root 'private\traffic-guard-plan.json'
    if (!(Test-Path -LiteralPath $path)) {
        $present=Invoke-SplitTrafficGuard $Root 'inspect'
        if ($present.ContainsKey('armed') -and $present.armed -eq $false -and $present.ContainsKey('verified') -and $present.verified -eq $true) {return}
    }
    $reply=Invoke-SplitTrafficGuard $Root 'disarm'
    if (!$reply.ContainsKey('armed') -or $reply.armed -ne $false -or !$reply.ContainsKey('verified') -or $reply.verified -ne $true) {throw 'Windows traffic protection removal could not be verified'}
    if ([IO.File]::Exists($path)) {[IO.File]::Delete($path)}
    [IO.File]::Delete((Join-Path $Root 'private\traffic-guard-transport.json'))
}
function Get-SplitTrafficGuardStatus([string]$Root) {
    $path=Join-Path $Root 'private\traffic-guard-plan.json'
    try {
        if (!(Test-Path -LiteralPath $path)) {
            $reply=Invoke-SplitTrafficGuard $Root 'inspect'
            $reply.Remove('plan')
            $reply.configured=($reply.armed -ne $false)
            if ($reply.configured) {$reply.recovery_pending=$true}
            return $reply
        }
        if ((Get-Item -LiteralPath $path).Length -gt 16384) {throw 'Invalid traffic protection recovery request'}
        $reply=Invoke-SplitTrafficGuard $Root 'status' (Read-JsonFile $path)
        $reply.configured=$true
        return $reply
    } catch {return @{configured=$true;armed=$null;verified=$false;protected=$false;recovery_pending=$true}}
}
function Assert-SplitTrafficGuardRoutingChange([string]$Root, $Current, $Candidate) {
    if (($Candidate.mode -cne $Current.mode -or $Candidate.allow_lan -ne $Current.allow_lan) -and (Get-SplitTrafficGuardStatus $Root).configured) {throw 'Disconnect VPN before changing routing mode or LAN access'}
}
Export-ModuleMember -Function *
