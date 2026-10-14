Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$package=Split-Path -Parent $PSScriptRoot
Import-Module (Join-Path $package 'ReleasePe.psm1') -Force -DisableNameChecking
$script:checks=0
function Assert($Condition,[string]$Label){if(!$Condition){throw ('TEST_RELEASE_PE_'+$Label)};$script:checks++}
function Put-U16([byte[]]$Bytes,[int]$Offset,[uint16]$Value){[BitConverter]::GetBytes($Value).CopyTo($Bytes,$Offset)}
function Put-U32([byte[]]$Bytes,[int]$Offset,[uint32]$Value){[BitConverter]::GetBytes($Value).CopyTo($Bytes,$Offset)}
function Hash-Bytes([byte[]]$Bytes){$hash=[Security.Cryptography.SHA256]::Create();try{return ([BitConverter]::ToString($hash.ComputeHash($Bytes))).Replace('-','').ToLowerInvariant()}finally{$hash.Dispose()}}

# Independently laid-out PE32+ fixture: one file-backed section, a descriptor
# array terminated by a zero descriptor, and separate ASCII DLL names.
# No fixture is executable or passed to a loader.
function New-Pe([string[]]$Dlls=@('KERNEL32.dll','USER32.dll')){
    $bytes=New-Object byte[] 65536
    $bytes[0]=0x4d;$bytes[1]=0x5a
    Put-U32 $bytes 0x3c 128
    Put-U32 $bytes 128 0x4550
    Put-U16 $bytes 132 0x8664
    Put-U16 $bytes 134 1
    Put-U16 $bytes 148 240
    Put-U16 $bytes 150 2
    Put-U16 $bytes 152 0x20b
    Put-U32 $bytes 212 512
    Put-U32 $bytes 260 16
    Put-U32 $bytes 272 0x1000
    Put-U32 $bytes 276 ([uint32](20*($Dlls.Count+1)))
    Put-U32 $bytes 400 65024
    Put-U32 $bytes 404 0x1000
    Put-U32 $bytes 408 65024
    Put-U32 $bytes 412 512
    for($i=0;$i -lt $Dlls.Count;$i++){
        Put-U32 $bytes (512+20*$i) 0x1300
        Put-U32 $bytes (524+20*$i) ([uint32](0x4000+64*$i))
        Put-U32 $bytes (528+20*$i) 0x1308
        [Text.Encoding]::ASCII.GetBytes($Dlls[$i]).CopyTo($bytes,(12800+64*$i))
    }
    return ,$bytes
}
function Clone-Valid([byte[]]$Bytes){$null=ConvertFrom-ReleasePeBytes $Bytes;return ,([byte[]]$Bytes.Clone())}
function Reject-Bytes([byte[]]$Bytes,[string]$Category){
    $rejected=$false
    try{$null=ConvertFrom-ReleasePeBytes $Bytes}catch{if($_.Exception.Message -cne $Category){throw};$rejected=$true}
    Assert $rejected $Category
}
$good=New-Pe
$report=ConvertFrom-ReleasePeBytes $good
Assert ($report.schema -eq 1 -and $report.machine -ceq 'AMD64' -and $report.format -ceq 'PE32+') 'POSITIVE_FORMAT'
Assert ($report.bytes -eq $good.Length -and $report.sha256 -ceq (Hash-Bytes $good)) 'SAME_BYTES_HASH'
Assert ($report.imports.Count -eq 2 -and $report.imports[0] -ceq 'kernel32.dll' -and $report.imports[1] -ceq 'user32.dll') 'IMPORTS'
Assert ($report.dynamic_crt_absent -and $report.delay_imports_absent -and !$report.runtime_compatibility_tested) 'SCOPE'
foreach($dll in @('VCRUNTIME140.dll','vcruntime140_1.dll','MSVCP140.dll','msvcr120.dll','msvcrt.dll','ucrtbase.dll','ucrtbased.dll','api-ms-win-crt-runtime-l1-1-0.dll','API-MS-WIN-CRT-MATH-L1-1-0.DLL')){Reject-Bytes (New-Pe @('KERNEL32.dll',$dll)) 'RELEASE_PE_DYNAMIC_CRT'}
$normal=ConvertFrom-ReleasePeBytes (New-Pe @('KERNEL32.dll','ntdll.dll','api-ms-win-core-file-l1-1-0.dll'))
Assert ($normal.imports.Count -eq 3) 'OS_API_IMPORT_ALLOWED'
Reject-Bytes ([byte[]]@()) 'RELEASE_PE_SIZE'
Reject-Bytes (New-Object byte[] 255) 'RELEASE_PE_SIZE'
$bad=Clone-Valid $good;$bad[0]=0;Reject-Bytes $bad 'RELEASE_PE_DOS_HEADER'
$bad=Clone-Valid $good;Put-U32 $bad 0x3c 32;Reject-Bytes $bad 'RELEASE_PE_HEADER'
$bad=Clone-Valid $good;Put-U32 $bad 0x3c ([uint32]::MaxValue);Reject-Bytes $bad 'RELEASE_PE_RANGE'
$bad=Clone-Valid $good;Put-U32 $bad 128 0x4551;Reject-Bytes $bad 'RELEASE_PE_SIGNATURE'
$bad=Clone-Valid $good;Put-U16 $bad 132 0x14c;Reject-Bytes $bad 'RELEASE_PE_MACHINE'
foreach($count in @([uint16]0,[uint16]97)){$bad=Clone-Valid $good;Put-U16 $bad 134 $count;Reject-Bytes $bad 'RELEASE_PE_SECTIONS'}
foreach($flags in @([uint16]0,[uint16]0x2002)){$bad=Clone-Valid $good;Put-U16 $bad 150 $flags;Reject-Bytes $bad 'RELEASE_PE_EXECUTABLE'}
$bad=Clone-Valid $good;Put-U16 $bad 148 112;Reject-Bytes $bad 'RELEASE_PE_OPTIONAL_HEADER'
$bad=Clone-Valid $good;Put-U16 $bad 148 ([uint16]::MaxValue);Reject-Bytes $bad 'RELEASE_PE_RANGE'
$bad=Clone-Valid $good;Put-U16 $bad 152 0x10b;Reject-Bytes $bad 'RELEASE_PE_FORMAT'
foreach($count in @([uint32]1,[uint32]17,[uint32]::MaxValue)){$bad=Clone-Valid $good;Put-U32 $bad 260 $count;Reject-Bytes $bad 'RELEASE_PE_DIRECTORIES'}
$bad=Clone-Valid $good;Put-U16 $bad 148 128;Reject-Bytes $bad 'RELEASE_PE_DIRECTORIES'
foreach($headers in @([uint32]431,[uint32]65537)){$bad=Clone-Valid $good;Put-U32 $bad 212 $headers;Reject-Bytes $bad 'RELEASE_PE_HEADERS_SIZE'}
$bad=Clone-Valid $good;Put-U32 $bad 412 511;Reject-Bytes $bad 'RELEASE_PE_SECTION_RAW'
$bad=Clone-Valid $good;Put-U32 $bad 408 65025;Reject-Bytes $bad 'RELEASE_PE_RANGE'
$bad=Clone-Valid $good;Put-U32 $bad 404 ([uint32]::MaxValue);Reject-Bytes $bad 'RELEASE_PE_SECTION_RVA'
$bad=Clone-Valid $good;Put-U32 $bad 404 0;Reject-Bytes $bad 'RELEASE_PE_SECTION_RVA'
foreach($kind in @('virtual','raw')){
    $bad=Clone-Valid $good;Put-U16 $bad 134 2
    Put-U32 $bad 400 32000;Put-U32 $bad 408 32000
    Put-U32 $bad 440 256;Put-U32 $bad 444 0x20000;Put-U32 $bad 448 256;Put-U32 $bad 452 65280
    if($kind -ceq 'virtual'){Put-U32 $bad 444 0x1800}else{Put-U32 $bad 452 1024}
    Reject-Bytes $bad 'RELEASE_PE_SECTION_OVERLAP'
}
foreach($offset in @(368,372)){$bad=Clone-Valid $good;Put-U32 $bad $offset 1;Reject-Bytes $bad 'RELEASE_PE_DELAY_IMPORT_UNAUDITED'}
foreach($size in @([uint32]0,[uint32]19,[uint32]131073)){$bad=Clone-Valid $good;Put-U32 $bad 276 $size;Reject-Bytes $bad 'RELEASE_PE_IMPORT_DIRECTORY'}
foreach($rva in @([uint32]0,[uint32]0x200,[uint32]::MaxValue)){$bad=Clone-Valid $good;Put-U32 $bad 272 $rva;Reject-Bytes $bad 'RELEASE_PE_RVA'}
$bad=Clone-Valid $good;Put-U32 $bad 276 40;Reject-Bytes $bad 'RELEASE_PE_IMPORT_TERMINATOR'
$bad=Clone-Valid $good;Put-U32 $bad 524 0;Reject-Bytes $bad 'RELEASE_PE_IMPORT_DESCRIPTOR'
$bad=Clone-Valid $good;Put-U32 $bad 528 0;Reject-Bytes $bad 'RELEASE_PE_IMPORT_DESCRIPTOR'
$bad=Clone-Valid $good;Put-U32 $bad 524 0x30000;Reject-Bytes $bad 'RELEASE_PE_RVA'
foreach($dll in @('folder/file.dll','name with space.dll','no-extension','name.dll:stream')){Reject-Bytes (New-Pe @($dll)) 'RELEASE_PE_DLL_NAME'}
foreach($value in @(31,127,255)){$bad=Clone-Valid $good;$bad[12800]=[byte]$value;Reject-Bytes $bad 'RELEASE_PE_DLL_NAME'}
$bad=Clone-Valid $good;for($i=0;$i -lt 260;$i++){$bad[12800+$i]=120};Reject-Bytes $bad 'RELEASE_PE_DLL_NAME'
$duplicate=ConvertFrom-ReleasePeBytes (New-Pe @('KERNEL32.dll','USER32.dll','kernel32.DLL'))
Assert ($duplicate.imports.Count -eq 3 -and $duplicate.imports[0] -ceq 'kernel32.dll' -and $duplicate.imports[2] -ceq 'kernel32.dll') 'VALID_DUPLICATE_DESCRIPTOR_SEQUENCE'
$dlls=@();for($i=0;$i -lt 256;$i++){$dlls+=('os'+$i+'.dll')}
$full=New-Pe $dlls;Assert ((ConvertFrom-ReleasePeBytes $full).imports.Count -eq 256) 'COUNT_LIMIT_ACCEPTED'
$bad=Clone-Valid $full;Put-U32 $bad 276 5160;Put-U32 $bad (512+20*256) 1;Reject-Bytes $bad 'RELEASE_PE_IMPORT_COUNT'

# Original stream owns the pin. Other readers are allowed; writers and deletes
# stay denied until Dispose, including a failed packaging scope's finally.
$fixture=Join-Path $PSScriptRoot ('release-pe-'+[guid]::NewGuid().ToString('N'))
$resolved=[IO.Path]::GetFullPath($fixture)
$testsRoot=[IO.Path]::GetFullPath($PSScriptRoot)
Assert ($resolved.StartsWith(($testsRoot+[IO.Path]::DirectorySeparatorChar),[StringComparison]::OrdinalIgnoreCase) -and [IO.Path]::GetFileName($resolved) -match '^release-pe-[a-f0-9]{32}$') 'FIXTURE_CONTAINED'
if(Test-Path -LiteralPath $fixture){throw 'TEST_RELEASE_PE_FRESH_REQUIRED'}
[void][IO.Directory]::CreateDirectory($fixture)
$lease=$null;$failureLease=$null
try{
    $leaf=Join-Path $fixture 'fixture.bin'
    [IO.File]::WriteAllBytes($leaf,$good)
    $lease=Open-ReleasePe $leaf
    Assert ($lease.Report.sha256 -ceq (Hash-Bytes $good)) 'LEASE_HASH'
    $reader=[IO.File]::Open($leaf,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try{Assert ($reader.Length -eq $good.Length) 'SECOND_READER'}finally{$reader.Dispose()}
    $denied=$false
    try{$writer=[IO.File]::Open($leaf,[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite);$writer.Dispose()}catch [IO.IOException]{$denied=$true}
    Assert $denied 'WRITE_DENIED_WHILE_PINNED'
    $denied=$false;try{[IO.File]::Delete($leaf)}catch [IO.IOException]{$denied=$true}
    Assert $denied 'DELETE_DENIED_WHILE_PINNED'
    $lease.Dispose();$lease.Dispose()
    Assert ($null -eq $lease.Stream) 'IDEMPOTENT_DISPOSE'
    $writer=[IO.File]::Open($leaf,[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::None);$writer.Dispose()
    Assert $true 'WRITE_ALLOWED_AFTER_DISPOSE'
    try{$failureLease=Open-ReleasePe $leaf;throw 'SYNTHETIC_PACKAGE_FAILURE'}catch{if($_.Exception.Message -cne 'SYNTHETIC_PACKAGE_FAILURE'){throw}}finally{if($null -ne $failureLease){$failureLease.Dispose()}}
    [IO.File]::Delete($leaf);Assert (![IO.File]::Exists($leaf)) 'PACKAGE_FAILURE_RELEASES_PIN'
    $oversize=Join-Path $fixture 'oversize.bin';$stream=[IO.File]::Open($oversize,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{$stream.SetLength(33554433)}finally{$stream.Dispose()}
    $denied=$false;try{$unexpected=Open-ReleasePe $oversize;$unexpected.Dispose()}catch{if($_.Exception.Message -cne 'RELEASE_PE_SIZE'){throw};$denied=$true}
    Assert $denied 'LENGTH_LIMIT_BEFORE_READ'
    [IO.File]::Delete($oversize);Assert (![IO.File]::Exists($oversize)) 'SIZE_FAILURE_RELEASES_HANDLE'
    $invalid=Join-Path $fixture 'invalid.bin';[IO.File]::WriteAllBytes($invalid,(New-Pe @('vcruntime140.dll')))
    $denied=$false;try{$unexpected=Open-ReleasePe $invalid;$unexpected.Dispose()}catch{if($_.Exception.Message -cne 'RELEASE_PE_DYNAMIC_CRT'){throw};$denied=$true}
    Assert $denied 'INVALID_FILE_STATIC_CATEGORY'
    [IO.File]::Delete($invalid);Assert (![IO.File]::Exists($invalid)) 'PARSE_FAILURE_RELEASES_HANDLE'
    $denied=$false;try{$unexpected=Open-ReleasePe (Join-Path $fixture 'missing-sensitive-name.bin');$unexpected.Dispose()}catch{if($_.Exception.Message -cne 'RELEASE_PE_OPEN'){throw};$denied=$true}
    Assert $denied 'OPEN_ERROR_WITHOUT_PATH'
}finally{
    if($null -ne $lease){$lease.Dispose()};if($null -ne $failureLease){$failureLease.Dispose()}
    # Only the literal freshly-owned fixture and its four ordinary files exist.
    foreach($name in @('fixture.bin','oversize.bin','invalid.bin')){[IO.File]::Delete((Join-Path $fixture $name))}
    [IO.Directory]::Delete($fixture,$false)
}

# Parse build sources; never invoke the packager or a native executable here.
$buildPath=Join-Path $package 'Build-Release.ps1'
$tokens=$null;$errors=$null
$buildAst=[Management.Automation.Language.Parser]::ParseFile($buildPath,[ref]$tokens,[ref]$errors)
Assert ($errors.Count -eq 0) 'BUILD_AST'
$openCalls=@($buildAst.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -ceq 'Open-ReleasePe'},$true))
Assert ($openCalls.Count -eq 1) 'ONE_GATE'
$outer=@($buildAst.EndBlock.Statements|Where-Object {$_ -is [Management.Automation.Language.TryStatementAst] -and $null -ne $_.Finally -and $_.Finally.Extent.Text.Contains('$releasePePin.Dispose()')})
Assert ($outer.Count -eq 1 -and $openCalls[0].Extent.StartOffset -gt $outer[0].Body.Extent.StartOffset -and $openCalls[0].Extent.EndOffset -lt $outer[0].Body.Extent.EndOffset) 'GATE_FINALLY_SCOPE'
$body=$outer[0].Body.Extent.Text
$copy=$body.IndexOf("[IO.File]::Copy((Join-Path `$nativeTarget 'release\mukhomor.exe')",[StringComparison]::Ordinal)
$gate=$body.IndexOf('$releasePePin=Open-ReleasePe',[StringComparison]::Ordinal)
$manifest=$body.IndexOf('foreach($package in @($runtime,$source))',[StringComparison]::Ordinal)
$zip=$body.IndexOf("foreach(`$kind in @('windows','source'))",[StringComparison]::Ordinal)
Assert ($copy -ge 0 -and $copy -lt $gate -and $gate -lt $manifest -and $manifest -lt $zip) 'COPY_GATE_MANIFEST_ZIP_ORDER'
Assert ($body.Contains('RELEASE_PE_MANIFEST_MISMATCH') -and $body.Contains('$releasePePin.Report.sha256')) 'MANIFEST_SAME_HASH'
Assert ($body.Contains("'ReleasePe.psm1'") -and $body.Contains("Copy-Reviewed 'tests\Test-ReleasePe.ps1' `$source")) 'EXPLICIT_SOURCE_PACKAGE'
$run=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Run-Checks.ps1'))
Assert ($run.IndexOf("'Test-ReleasePe.ps1'",[StringComparison]::Ordinal) -ge 0 -and $run.IndexOf("'Test-ReleasePe.ps1'",[StringComparison]::Ordinal) -lt $run.IndexOf('if(!$SkipNative)',[StringComparison]::Ordinal)) 'SKIP_NATIVE_STILL_CHECKS_PE'
$ignore=[IO.File]::ReadAllText((Join-Path $package '.gitignore'))
Assert ($ignore -match '(?m)^!/ReleasePe\.psm1\r?$' -and $ignore -match '(?m)^!/tests/Test-ReleasePe\.ps1\r?$') 'PUBLIC_ALLOWLIST'
$privacy=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Test-ReleasePrivacy.ps1'))
Assert ($privacy.Contains("'ReleasePe.psm1','tests\Test-ReleasePe.ps1'")) 'SOURCE_ARCHIVE_REQUIRED'
Write-Host ('PASS: '+$script:checks+' PE metadata, bounded file pin and release integration controls')
