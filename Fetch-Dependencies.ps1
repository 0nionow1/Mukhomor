Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
Add-Type -AssemblyName System.IO.Compression.FileSystem
$cache=Join-Path $PSScriptRoot 'runtime\downloads'
$bin=Join-Path $PSScriptRoot 'bin'
foreach($path in @($cache,$bin,(Join-Path $PSScriptRoot 'Source'))){[IO.Directory]::CreateDirectory($path)|Out-Null}
function Get-Verified([string]$Url,[string]$Path,[string]$Hash){
    if(!(Test-Path -LiteralPath $Path) -or (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Hash){
        Invoke-WebRequest -Uri $Url -OutFile ($Path+'.download') -UseBasicParsing -TimeoutSec 60
        if((Get-FileHash -LiteralPath ($Path+'.download') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Hash){[IO.File]::Delete($Path+'.download');throw 'Dependency checksum mismatch'}
        Move-Item -LiteralPath ($Path+'.download') -Destination $Path -Force
    }
}
function Extract-One([string]$Archive,[string]$Name,[string]$Target){
    $zip=[IO.Compression.ZipFile]::OpenRead($Archive)
    try{
        $entry=$zip.GetEntry($Name)
        if(!$entry){throw ('Missing ZIP entry: '+$Name)}
        $stream=$entry.Open();$output=[IO.File]::Create($Target)
        try{$stream.CopyTo($output)}finally{$output.Dispose();$stream.Dispose()}
    }finally{$zip.Dispose()}
}
$mihomo=Join-Path $cache 'mihomo-v1.19.32.zip'
Get-Verified 'https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/mihomo-windows-amd64-compatible-v1.19.32.zip' $mihomo '974a4d7ad69aed27aa2e8f91d61113573c14dadb14562c63e58effabf59816f0'
Extract-One $mihomo 'mihomo-windows-amd64-compatible.exe' (Join-Path $bin 'mihomo-windows-amd64-compatible.exe')
$wintun=Join-Path $cache 'wintun-0.14.1.zip'
Get-Verified 'https://www.wintun.net/builds/wintun-0.14.1.zip' $wintun '07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51'
Extract-One $wintun 'wintun/bin/amd64/wintun.dll' (Join-Path $bin 'wintun.dll')
$source=Join-Path $PSScriptRoot 'Source\mihomo-v1.19.32-source.zip'
Get-Verified 'https://codeload.github.com/MetaCubeX/mihomo/zip/refs/tags/v1.19.32' $source '135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1'
Write-Host 'Pinned Mihomo, Wintun and corresponding Mihomo source verified.'
