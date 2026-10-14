Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'

# Metadata inspection only. This module never loads or executes a PE image.
# Unsupported layouts fail closed; this is the Windows x64 release gate,
# not a general-purpose PE parser or a proof of runtime compatibility.
function Assert-ReleasePeRange([byte[]]$Bytes,[uint64]$Offset,[uint64]$Length){
    if($Offset -gt [uint64]$Bytes.LongLength -or $Length -gt ([uint64]$Bytes.LongLength-$Offset)){throw 'RELEASE_PE_RANGE'}
}
function Read-ReleasePeU16([byte[]]$Bytes,[uint64]$Offset){
    Assert-ReleasePeRange $Bytes $Offset 2
    return [BitConverter]::ToUInt16($Bytes,[int]$Offset)
}
function Read-ReleasePeU32([byte[]]$Bytes,[uint64]$Offset){
    Assert-ReleasePeRange $Bytes $Offset 4
    return [BitConverter]::ToUInt32($Bytes,[int]$Offset)
}
function Resolve-ReleasePeRva([object[]]$Sections,[uint64]$Rva,[uint64]$Length){
    if($Rva -eq 0 -or $Length -eq 0 -or $Rva -gt 4294967295 -or $Length -gt (4294967296-$Rva)){throw 'RELEASE_PE_RVA'}
    $match=$null
    foreach($section in $Sections){
        if($section.RawSize -gt 0 -and $Rva -ge $section.Rva -and $Rva -lt ($section.Rva+$section.RawSize)){
            if($null -ne $match -or $Length -gt ($section.Rva+$section.RawSize-$Rva)){throw 'RELEASE_PE_RVA'}
            $match=[pscustomobject]@{Offset=($section.RawOffset+$Rva-$section.Rva);Available=($section.Rva+$section.RawSize-$Rva)}
        }
    }
    if($null -eq $match){throw 'RELEASE_PE_RVA'}
    return $match
}
function Read-ReleasePeDll([byte[]]$Bytes,[object[]]$Sections,[uint64]$Rva){
    $location=Resolve-ReleasePeRva $Sections $Rva 1
    $limit=[int][Math]::Min([uint64]260,[uint64]$location.Available)
    $name=New-Object Text.StringBuilder
    for($i=0;$i -lt $limit;$i++){
        $value=$Bytes[[int]($location.Offset+$i)]
        if($value -eq 0){
            $result=$name.ToString()
            if($result -cnotmatch '^[A-Za-z0-9_.-]+\.[dD][lL][lL]$'){throw 'RELEASE_PE_DLL_NAME'}
            return $result.ToLowerInvariant()
        }
        if($value -lt 33 -or $value -gt 126){throw 'RELEASE_PE_DLL_NAME'}
        [void]$name.Append([char]$value)
    }
    throw 'RELEASE_PE_DLL_NAME'
}
function ConvertFrom-ReleasePeBytes {
    [CmdletBinding()]
    param([Parameter(Mandatory=$true)][AllowEmptyCollection()][byte[]]$Bytes)
    if($null -eq $Bytes -or $Bytes.LongLength -lt 256 -or $Bytes.LongLength -gt 33554432){throw 'RELEASE_PE_SIZE'}
    if(![BitConverter]::IsLittleEndian){throw 'RELEASE_PE_ENDIAN'}
    if($Bytes[0] -ne 0x4d -or $Bytes[1] -ne 0x5a){throw 'RELEASE_PE_DOS_HEADER'}
    [uint64]$pe=Read-ReleasePeU32 $Bytes 0x3c
    if($pe -lt 0x40){throw 'RELEASE_PE_HEADER'}
    Assert-ReleasePeRange $Bytes $pe 24
    if((Read-ReleasePeU32 $Bytes $pe) -ne 0x4550){throw 'RELEASE_PE_SIGNATURE'}
    if((Read-ReleasePeU16 $Bytes ($pe+4)) -ne 0x8664){throw 'RELEASE_PE_MACHINE'}
    $count=Read-ReleasePeU16 $Bytes ($pe+6)
    if($count -lt 1 -or $count -gt 96){throw 'RELEASE_PE_SECTIONS'}
    $characteristics=Read-ReleasePeU16 $Bytes ($pe+22)
    if(($characteristics -band 2) -eq 0 -or ($characteristics -band 0x2000) -ne 0){throw 'RELEASE_PE_EXECUTABLE'}
    [uint64]$optional=$pe+24
    [uint64]$optionalSize=Read-ReleasePeU16 $Bytes ($pe+20)
    if($optionalSize -lt 128){throw 'RELEASE_PE_OPTIONAL_HEADER'}
    Assert-ReleasePeRange $Bytes $optional $optionalSize
    if((Read-ReleasePeU16 $Bytes $optional) -ne 0x20b){throw 'RELEASE_PE_FORMAT'}
    [uint64]$directories=Read-ReleasePeU32 $Bytes ($optional+108)
    if($directories -lt 2 -or $directories -gt 16 -or (112+8*$directories) -gt $optionalSize){throw 'RELEASE_PE_DIRECTORIES'}
    [uint64]$table=$optional+$optionalSize
    Assert-ReleasePeRange $Bytes $table (40*[uint64]$count)
    [uint64]$headers=Read-ReleasePeU32 $Bytes ($optional+60)
    if($headers -lt ($table+40*[uint64]$count) -or $headers -gt [uint64]$Bytes.LongLength){throw 'RELEASE_PE_HEADERS_SIZE'}
    $sections=New-Object 'Collections.Generic.List[object]'
    for($i=0;$i -lt $count;$i++){
        [uint64]$row=$table+40*[uint64]$i
        [uint64]$virtualSize=Read-ReleasePeU32 $Bytes ($row+8)
        [uint64]$rva=Read-ReleasePeU32 $Bytes ($row+12)
        [uint64]$rawSize=Read-ReleasePeU32 $Bytes ($row+16)
        [uint64]$rawOffset=Read-ReleasePeU32 $Bytes ($row+20)
        [uint64]$extent=[Math]::Max($virtualSize,$rawSize)
        if($extent -gt 0 -and ($rva -eq 0 -or $extent -gt (4294967296-$rva))){throw 'RELEASE_PE_SECTION_RVA'}
        if($rawSize -gt 0){
            if($rawOffset -lt $headers){throw 'RELEASE_PE_SECTION_RAW'}
            Assert-ReleasePeRange $Bytes $rawOffset $rawSize
        }
        foreach($previous in $sections){
            if($extent -gt 0 -and $previous.Extent -gt 0 -and $rva -lt ($previous.Rva+$previous.Extent) -and $previous.Rva -lt ($rva+$extent)){throw 'RELEASE_PE_SECTION_OVERLAP'}
            if($rawSize -gt 0 -and $previous.RawSize -gt 0 -and $rawOffset -lt ($previous.RawOffset+$previous.RawSize) -and $previous.RawOffset -lt ($rawOffset+$rawSize)){throw 'RELEASE_PE_SECTION_OVERLAP'}
        }
        [void]$sections.Add([pscustomobject]@{Rva=$rva;Extent=$extent;RawSize=$rawSize;RawOffset=$rawOffset})
    }
    if($directories -gt 13 -and ((Read-ReleasePeU32 $Bytes ($optional+112+8*13)) -ne 0 -or (Read-ReleasePeU32 $Bytes ($optional+116+8*13)) -ne 0)){throw 'RELEASE_PE_DELAY_IMPORT_UNAUDITED'}
    [uint64]$importRva=Read-ReleasePeU32 $Bytes ($optional+120)
    [uint64]$importSize=Read-ReleasePeU32 $Bytes ($optional+124)
    if($importSize -lt 20 -or $importSize -gt 131072){throw 'RELEASE_PE_IMPORT_DIRECTORY'}
    $importsLocation=Resolve-ReleasePeRva $sections.ToArray() $importRva $importSize
    $imports=New-Object 'Collections.Generic.List[string]'
    $terminated=$false
    for($i=0;$i -le 256;$i++){
        [uint64]$relative=20*[uint64]$i
        if(($relative+20) -gt $importSize){throw 'RELEASE_PE_IMPORT_TERMINATOR'}
        [uint64]$descriptor=$importsLocation.Offset+$relative
        $words=@((Read-ReleasePeU32 $Bytes $descriptor),(Read-ReleasePeU32 $Bytes ($descriptor+4)),(Read-ReleasePeU32 $Bytes ($descriptor+8)),(Read-ReleasePeU32 $Bytes ($descriptor+12)),(Read-ReleasePeU32 $Bytes ($descriptor+16)))
        if(@($words|Where-Object {$_ -ne 0}).Count -eq 0){$terminated=$true;break}
        if($i -eq 256){throw 'RELEASE_PE_IMPORT_COUNT'}
        if($words[3] -eq 0 -or $words[4] -eq 0){throw 'RELEASE_PE_IMPORT_DESCRIPTOR'}
        $dll=Read-ReleasePeDll $Bytes $sections.ToArray() $words[3]
        if($dll -match '^(?:(?:vcruntime|msvcp|msvcr).*|ucrtbased?|api-ms-win-crt-.*)\.dll$'){throw 'RELEASE_PE_DYNAMIC_CRT'}
        # MSVC may emit multiple descriptors for the same DLL. Preserve the
        # bounded descriptor sequence; apply the CRT gate to every entry.
        [void]$imports.Add($dll)
    }
    if(!$terminated -or $imports.Count -eq 0){throw 'RELEASE_PE_IMPORT_TERMINATOR'}
    $hash=[Security.Cryptography.SHA256]::Create()
    try{$sha=([BitConverter]::ToString($hash.ComputeHash($Bytes))).Replace('-','').ToLowerInvariant()}finally{$hash.Dispose()}
    return [pscustomobject]@{schema=1;bytes=$Bytes.LongLength;sha256=$sha;machine='AMD64';format='PE32+';imports=$imports.ToArray();dynamic_crt_absent=$true;delay_imports_absent=$true;runtime_compatibility_tested=$false}
}
function Open-ReleasePe {
    [CmdletBinding()]
    param([Parameter(Mandatory=$true)][string]$Path)
    $stream=$null
    try{
        try{$stream=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)}catch{throw 'RELEASE_PE_OPEN'}
        [long]$length=$stream.Length
        if($length -lt 256 -or $length -gt 33554432){throw 'RELEASE_PE_SIZE'}
        $bytes=New-Object byte[] ([int]$length)
        $position=0
        while($position -lt $bytes.Length){$read=$stream.Read($bytes,$position,$bytes.Length-$position);if($read -le 0){throw 'RELEASE_PE_READ'};$position+=$read}
        if($stream.ReadByte() -ne -1 -or $stream.Length -ne $length){throw 'RELEASE_PE_CHANGED'}
        $report=ConvertFrom-ReleasePeBytes $bytes
        $lease=[pscustomobject]@{Stream=$stream;Report=$report}
        $lease|Add-Member -MemberType ScriptMethod -Name Dispose -Value {if($null -ne $this.Stream){$this.Stream.Dispose();$this.Stream=$null}}
        $stream=$null
        return $lease
    }catch{
        $category=[string]$_.Exception.Message
        if($category -cnotmatch '^RELEASE_PE_[A-Z_]+$'){$category='RELEASE_PE_INSPECTION_FAILED'}
        throw $category
    }finally{if($null -ne $stream){$stream.Dispose()}}
}
Export-ModuleMember -Function ConvertFrom-ReleasePeBytes,Open-ReleasePe
