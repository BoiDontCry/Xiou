$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$src = 'C:\Users\lzzwf\.minimax\v2\assets\2026\09\18\17-28-41-917-asset_20260918-172841-917_bde49f803dbc_eb3b0fa4-bluehair_pinkbow_transparent.png'
$dst = 'D:\code\foxmail-rs\xiou.ico'

if (-not (Test-Path $src)) { throw "source png missing: $src" }

$orig = [System.Drawing.Image]::FromFile($src)
$sizes = 16, 32, 48, 64, 128, 256

$pngBytes = @()
foreach ($s in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap($s, $s, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($orig, 0, 0, $s, $s)
    $g.Dispose()

    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    $pngBytes += , $ms.ToArray()
    $ms.Dispose()
}
$orig.Dispose()

$count = $sizes.Count
$headerSize = 6 + 16 * $count
$offset = $headerSize

$icoStream = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($icoStream)

# ICONDIR
$bw.Write([uint16]0)        # reserved
$bw.Write([uint16]1)        # type = icon
$bw.Write([uint16]$count)   # count

# ICONDIRENTRY[]
for ($i = 0; $i -lt $count; $i++) {
    $s = $sizes[$i]
    $w = if ($s -ge 256) { 0 } else { $s }
    $h = if ($s -ge 256) { 0 } else { $s }
    $bytes = $pngBytes[$i]
    $bw.Write([byte]$w)                 # width
    $bw.Write([byte]$h)                 # height
    $bw.Write([byte]0)                  # colors
    $bw.Write([byte]0)                  # reserved
    $bw.Write([uint16]1)                # planes
    $bw.Write([uint16]32)               # bpp
    $bw.Write([uint32]$bytes.Length)    # size
    $bw.Write([uint32]$offset)          # offset
    $offset += $bytes.Length
}

# data
foreach ($b in $pngBytes) { $bw.Write($b) }

$bw.Flush()
[System.IO.File]::WriteAllBytes($dst, $icoStream.ToArray())
$bw.Dispose()
$icoStream.Dispose()

Write-Host "wrote $dst ($(((Get-Item $dst).Length)) bytes)"