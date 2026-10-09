Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$fontPath=Join-Path (Split-Path -Parent $PSScriptRoot) 'assets\fonts\Tiny5-Regular.ttf'
if((Get-FileHash -LiteralPath $fontPath).Hash.ToLowerInvariant() -ne 'cb8168f80cfee2f47f6db59f2a7afbde31cdcdcdcf262e7a993e4d468a5bf4b0'){throw 'Font checksum mismatch'}
Add-Type -TypeDefinition @'
using System; using System.IO; using System.Text; using System.Runtime.InteropServices;
public static class MukhomorFontProbe {
 [DllImport("gdi32.dll")] static extern IntPtr AddFontMemResourceEx(IntPtr bytes,uint length,IntPtr reserved,out uint count);
 [DllImport("gdi32.dll")] static extern bool RemoveFontMemResourceEx(IntPtr font);
 [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
 [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
 [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr obj);
 [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc,IntPtr obj);
 [DllImport("gdi32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateFont(int height,int width,int angle,int orientation,int weight,uint italic,uint underline,uint strike,uint charset,uint output,uint clip,uint quality,uint pitch,string family);
 [DllImport("gdi32.dll",CharSet=CharSet.Unicode)] static extern int GetTextFace(IntPtr dc,int size,StringBuilder face);
 [DllImport("gdi32.dll",CharSet=CharSet.Unicode)] static extern uint GetGlyphIndices(IntPtr dc,string text,int count,[Out]ushort[] glyphs,uint flags);
 public static int Verify(string path) {
  byte[] data=File.ReadAllBytes(path); IntPtr bytes=Marshal.AllocHGlobal(data.Length);
  IntPtr resource=IntPtr.Zero,dc=IntPtr.Zero,font=IntPtr.Zero,prior=IntPtr.Zero;
  try {
   Marshal.Copy(data,0,bytes,data.Length); uint faces;
   resource=AddFontMemResourceEx(bytes,(uint)data.Length,IntPtr.Zero,out faces);
   if(resource==IntPtr.Zero || faces==0) throw new Exception("Private font registration failed");
   dc=CreateCompatibleDC(IntPtr.Zero); font=CreateFont(-16,0,0,0,400,0,0,0,1,0,0,3,0,"Tiny5");
   if(dc==IntPtr.Zero || font==IntPtr.Zero) throw new Exception("Native font creation failed");
   prior=SelectObject(dc,font); var family=new StringBuilder(128); GetTextFace(dc,128,family);
   if(family.ToString()!="Tiny5") throw new Exception("Windows selected a fallback font");
   string text="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
   for(int code=0x0410;code<=0x044F;code++) text+=(char)code;
   text+="\u0401\u0451\u00B7\u2026";
   ushort[] glyphs=new ushort[text.Length];
   if(GetGlyphIndices(dc,text,text.Length,glyphs,1)==0xFFFFFFFF) throw new Exception("Glyph lookup failed");
   for(int i=0;i<glyphs.Length;i++) if(glyphs[i]==0xFFFF) throw new Exception("Missing glyph U+"+((int)text[i]).ToString("X4"));
   return glyphs.Length;
  } finally {
   if(prior!=IntPtr.Zero) SelectObject(dc,prior);
   if(font!=IntPtr.Zero) DeleteObject(font); if(dc!=IntPtr.Zero) DeleteDC(dc);
   if(resource!=IntPtr.Zero) RemoveFontMemResourceEx(resource); Marshal.FreeHGlobal(bytes);
  }
 }
}
'@
$glyphCount=[MukhomorFontProbe]::Verify($fontPath)
Write-Host ('PASS: bundled Tiny5 face, hash and '+$glyphCount+' Latin/Cyrillic/UI glyphs; no system font installation')
