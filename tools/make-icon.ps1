param(
  [string]$Source = "E:\DnD-rs\crates\dnd-desktop\icons\icon-source.png",
  [string]$OutDir = "E:\DnD-rs\crates\dnd-desktop\icons"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

if (-not (Test-Path $Source)) { throw "source image not found: $Source" }
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force -Path $OutDir | Out-Null }

$image = [System.Drawing.Image]::FromFile($Source)
try {
  $side = [Math]::Min($image.Width, $image.Height)
  $srcX = [int](($image.Width - $side) / 2)
  $srcY = [int](($image.Height - $side) / 2)
  $srcRect = New-Object System.Drawing.Rectangle $srcX, $srcY, $side, $side

  foreach ($size in 16, 24, 32, 48, 64, 128, 256, 512) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bmp)
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $destRect = New-Object System.Drawing.Rectangle 0, 0, $size, $size
    $graphics.DrawImage($image, $destRect, $srcRect, [System.Drawing.GraphicsUnit]::Pixel)
    $graphics.Dispose()
    $bmp.Save((Join-Path $OutDir ("{0}x{0}.png" -f $size)), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
  }
} finally {
  $image.Dispose()
}

Copy-Item (Join-Path $OutDir '256x256.png') (Join-Path $OutDir '128x128@2x.png') -Force

$icoSizes = 16, 24, 32, 48, 64, 128, 256
$entries = @()
$offset = 6 + 16 * $icoSizes.Count
foreach ($size in $icoSizes) {
  $bytes = [IO.File]::ReadAllBytes((Join-Path $OutDir ("{0}x{0}.png" -f $size)))
  $entries += [pscustomobject]@{ Size = $size; Bytes = $bytes; Offset = $offset }
  $offset += $bytes.Length
}

$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([UInt16]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]$icoSizes.Count)
foreach ($entry in $entries) {
  $dimension = if ($entry.Size -ge 256) { 0 } else { $entry.Size }
  $writer.Write([Byte]$dimension)
  $writer.Write([Byte]$dimension)
  $writer.Write([Byte]0)
  $writer.Write([Byte]0)
  $writer.Write([UInt16]1)
  $writer.Write([UInt16]32)
  $writer.Write([UInt32]$entry.Bytes.Length)
  $writer.Write([UInt32]$entry.Offset)
}
foreach ($entry in $entries) { $writer.Write($entry.Bytes) }
$writer.Flush()
[IO.File]::WriteAllBytes((Join-Path $OutDir 'icon.ico'), $stream.ToArray())
$writer.Dispose()
$stream.Dispose()

Write-Host "Icons written to $OutDir"
