# Renders the application icon, src/app.ico: the tray icon's speaker glyph from Segoe
# Fluent Icons in the default Windows 11 accent blue. Run it again only to change the
# icon; build.rs embeds the committed file.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$glyph = [string][char]0xE995
$color = [Drawing.Color]::FromArgb(255, 0, 95, 184)
$family = 'Segoe Fluent Icons'
if (-not ([Drawing.Text.InstalledFontCollection]::new().Families.Name -contains $family)) {
    throw "$family is required to render the icon."
}

$images = foreach ($size in 16, 20, 24, 32, 40, 48, 64, 256) {
    $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $graphics.Clear([Drawing.Color]::Transparent)
    $font = [Drawing.Font]::new($family, [single]$size, [Drawing.GraphicsUnit]::Pixel)
    $format = [Drawing.StringFormat]::GenericTypographic
    $format.Alignment = [Drawing.StringAlignment]::Center
    $format.LineAlignment = [Drawing.StringAlignment]::Center
    $brush = [Drawing.SolidBrush]::new($color)
    # Lay the line out in a larger box: a line taller than the icon would be dropped.
    $format.FormatFlags = [Drawing.StringFormatFlags]::NoClip
    $graphics.DrawString($glyph, $font, $brush, [Drawing.RectangleF]::new(-$size, -$size, 3 * $size, 3 * $size), $format)
    $stream = [IO.MemoryStream]::new()
    $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
    $brush.Dispose(); $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
    [pscustomobject]@{ Size = $size; Data = $stream.ToArray() }
}

# ICONDIR, one ICONDIRENTRY per image, then the PNG-compressed images.
$out = [IO.MemoryStream]::new()
$writer = [IO.BinaryWriter]::new($out)
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($image in $images) {
    $edge = if ($image.Size -ge 256) { 0 } else { $image.Size }
    $writer.Write([byte]$edge); $writer.Write([byte]$edge); $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$image.Data.Length); $writer.Write([uint32]$offset)
    $offset += $image.Data.Length
}
foreach ($image in $images) { $writer.Write($image.Data) }
$writer.Flush()
[IO.File]::WriteAllBytes((Join-Path $root 'src/app.ico'), $out.ToArray())
Write-Host "Wrote src/app.ico ($($images.Count) sizes)."
