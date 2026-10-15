# Rebuild repository artwork from the website's palette, font and original art.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$fonts=New-Object Drawing.Text.PrivateFontCollection
$fonts.AddFontFile((Join-Path $root 'assets\fonts\Tiny5-Regular.ttf'))
$family=$fonts.Families[0]
$bitmap=New-Object Drawing.Bitmap(1280,640)
$graphics=[Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode=[Drawing.Drawing2D.SmoothingMode]::None
$graphics.Clear([Drawing.ColorTranslator]::FromHtml('#120c18'))
$svg=New-Object Text.StringBuilder
[void]$svg.Append('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 640" role="img" aria-label="Мухомор 0.8.1 — Windows, Linux, Android. Rust и Slint."><rect width="1280" height="640" fill="#120c18"/>')
function Rect([float]$X,[float]$Y,[float]$W,[float]$H,[string]$Color){
    $brush=New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml($Color))
    try{$graphics.FillRectangle($brush,$X,$Y,$W,$H)}finally{$brush.Dispose()}
    [void]$svg.Append(('<rect x="{0}" y="{1}" width="{2}" height="{3}" fill="{4}"/>' -f $X,$Y,$W,$H,$Color))
}
function Label([string]$Text,[float]$X,[float]$Y,[float]$Size,[string]$Color){
    $shape=New-Object Drawing.Drawing2D.GraphicsPath
    $brush=New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml($Color))
    try{
        $shape.AddString($Text,$family,0,$Size,(New-Object Drawing.PointF($X,$Y)),[Drawing.StringFormat]::GenericTypographic)
        $graphics.FillPath($brush,$shape)
        $shape.Flatten()
        $d=New-Object Text.StringBuilder
        for($i=0;$i -lt $shape.PointCount;$i++){
            $point=$shape.PathPoints[$i];$kind=$shape.PathTypes[$i]
            $command=if(($kind -band 7) -eq 0){'M'}else{'L'}
            [void]$d.Append($command+$point.X.ToString('0.##',[Globalization.CultureInfo]::InvariantCulture)+' '+$point.Y.ToString('0.##',[Globalization.CultureInfo]::InvariantCulture))
            if($kind -band 128){[void]$d.Append('Z')}
        }
        [void]$svg.Append('<path fill="'+$Color+'" d="'+$d.ToString()+'"/>')
    }finally{$shape.Dispose();$brush.Dispose()}
}
# Restrained pixel field, like the site's hero rather than a second visual system.
for($y=0;$y -lt 640;$y+=16){for($x=720;$x -lt 1280;$x+=16){
    $wave=[Math]::Sin($x/125+$y/160)+[Math]::Cos($y/110-$x/190)
    if($wave -gt 0.8){Rect $x $y 16 16 '#191322'}elseif($wave -lt -1.1){Rect $x $y 16 16 '#142426'}
}}
Label 'СВОЙ СЕРВЕР · СВОИ ПРАВИЛА' 64 54 26 '#b5a3bd'
Label 'Мухомор' 56 120 128 '#fff0fa'
Label 'VPN для игр и работы.' 64 286 40 '#fff0fa'
Label 'Выберите свой маршрут.' 64 340 32 '#b5a3bd'
Label 'WINDOWS / LINUX / ANDROID' 64 420 30 '#16f5d1'
# Large original mushroom and the quiet pink stepped mycelium used by the site.
[xml]$art=[IO.File]::ReadAllText((Join-Path $root 'website\public\assets\mushroom-hero.svg'))
foreach($item in $art.svg.path){
    foreach($match in [regex]::Matches($item.d,'M(\d+) (\d+)h(\d+)v1H\d+z')){
        Rect (864+4*[int]$match.Groups[1].Value) (156+4*[int]$match.Groups[2].Value) (4*[int]$match.Groups[3].Value) 4 $item.fill
    }
}
foreach($r in @(@(64,508,432,4),@(496,504,16,4),@(512,500,16,4),@(528,504,16,4),@(544,508,244,4),@(788,512,16,4),@(804,516,16,4),@(820,520,396,4),@(212,512,4,16),@(196,528,20,4),@(196,532,4,12),@(680,512,4,20),@(664,532,20,4),@(1128,524,4,12),@(1132,536,20,4))){Rect $r[0] $r[1] $r[2] $r[3] '#6c3c71'}
Label '0.8.1 / RUST + SLINT / OPEN SOURCE' 64 556 26 '#16f5d1'
[void]$svg.Append('</svg>')
try{
    [IO.File]::WriteAllText((Join-Path $root 'assets\github-banner.svg'),$svg.ToString()+"`n",(New-Object Text.UTF8Encoding($false)))
    $bitmap.Save((Join-Path $root 'assets\github-social-preview.png'),[Drawing.Imaging.ImageFormat]::Png)
}finally{$graphics.Dispose();$bitmap.Dispose();$fonts.Dispose()}
Copy-Item -LiteralPath (Join-Path $root 'assets\github-social-preview.png') -Destination (Join-Path $root 'website\public\assets\social-preview.png') -Force
$preview=Join-Path $root 'target\public-release-preview'
Copy-Item -LiteralPath (Join-Path $preview 'connection-reference.png') -Destination (Join-Path $root 'assets\preview.png') -Force
Copy-Item -LiteralPath (Join-Path $preview 'connection-reference.png') -Destination (Join-Path $root 'website\public\assets\app-preview.png') -Force
$sheet=New-Object Drawing.Bitmap(1712,740)
$drawing=[Drawing.Graphics]::FromImage($sheet)
$drawing.Clear([Drawing.ColorTranslator]::FromHtml('#120c18'))
try{
    $offset=0
    foreach($name in @('connection-reference.png','connection-paper.png','connection-glacier.png')){
        $frame=[Drawing.Image]::FromFile((Join-Path $preview $name))
        try{$drawing.DrawImageUnscaled($frame,$offset,0)}finally{$frame.Dispose()}
        $offset+=576
    }
    $sheet.Save((Join-Path $root 'assets\appearance-preview.png'),[Drawing.Imaging.ImageFormat]::Png)
}finally{$drawing.Dispose();$sheet.Dispose()}
Write-Host 'Updated GitHub banner, social image and current Slint screenshots using website artwork.'
