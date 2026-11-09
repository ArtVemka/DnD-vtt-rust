$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$out = 'E:\DnD-rs\crates\dnd-desktop\icons\icon-source.png'
$size = 1024
$bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

function Col([int]$a, [int]$r, [int]$gg, [int]$b) {
  return [System.Drawing.Color]::FromArgb($a, $r, $gg, $b)
}
function P([float]$x, [float]$y) {
  return New-Object System.Drawing.PointF $x, $y
}

$canvas = New-Object System.Drawing.Rectangle 0, 0, $size, $size

# Warm glow behind the medallion.
$glowPath = New-Object System.Drawing.Drawing2D.GraphicsPath
$glowPath.AddEllipse(70, 70, 884, 884)
$glow = New-Object System.Drawing.Drawing2D.PathGradientBrush $glowPath
$glow.CenterColor = Col 110 231 203 146
$glow.SurroundColors = [System.Drawing.Color[]]@(Col 0 231 203 146)
$g.FillPath($glow, $glowPath)
$glow.Dispose()
$glowPath.Dispose()

# Dark oak medallion.
$medallion = New-Object System.Drawing.Drawing2D.GraphicsPath
$medallion.AddEllipse(112, 112, 800, 800)
$wood = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
  (New-Object System.Drawing.Rectangle 112, 112, 800, 800),
  (Col 255 58 36 22),
  (Col 255 16 10 6),
  58
)
$g.FillPath($wood, $medallion)

# Wood grain, clipped to the medallion.
$state = $g.Save()
$g.SetClip($medallion)
for ($i = 0; $i -lt 22; $i++) {
  $alpha = 10 + (($i * 7) % 18)
  $pen = New-Object System.Drawing.Pen (Col $alpha 120 80 38), (1 + ($i % 4))
  $x = 90 + $i * 19
  $y = 120 + $i * 11
  $w = 850 - $i * 30
  $h = 780 - $i * 10
  $g.DrawArc($pen, $x, $y, $w, $h, 190 + (($i * 13) % 90), 150)
  $pen.Dispose()
}
$g.Restore($state)

# Rims.
$outerRim = New-Object System.Drawing.Pen (Col 255 74 50 26), 22
$g.DrawEllipse($outerRim, 100, 100, 824, 824)
$goldRim = New-Object System.Drawing.Pen (Col 255 201 164 94), 14
$g.DrawEllipse($goldRim, 112, 112, 800, 800)
$innerRim = New-Object System.Drawing.Pen (Col 220 240 215 154), 4
$g.DrawEllipse($innerRim, 132, 132, 760, 760)
$outerRim.Dispose(); $goldRim.Dispose(); $innerRim.Dispose()

# d20 silhouette: hexagon with a raised central triangle.
$top = P 512 208
$ur = P 766 360
$lr = P 766 664
$bottom = P 512 816
$ll = P 258 664
$ul = P 258 360
$hex = [System.Drawing.PointF[]]@($top, $ur, $lr, $bottom, $ll, $ul)

$dieBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
  (New-Object System.Drawing.Rectangle 220, 190, 580, 650),
  (Col 255 244 214 158),
  (Col 255 142 96 34),
  62
)
$g.FillPolygon($dieBrush, $hex)

# Facet shading.
$warm = New-Object System.Drawing.SolidBrush (Col 46 255 236 190)
$g.FillPolygon($warm, [System.Drawing.PointF[]]@($top, (P 512 332), $ur))
$shade = New-Object System.Drawing.SolidBrush (Col 42 60 30 0)
$g.FillPolygon($shade, [System.Drawing.PointF[]]@($bottom, $ll, $lr))
$warm.Dispose(); $shade.Dispose()

$tt = P 512 330
$tr = P 682 642
$tl = P 342 642
$tri = [System.Drawing.PointF[]]@($tt, $tr, $tl)
$triBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
  (New-Object System.Drawing.Rectangle 330, 320, 360, 340),
  (Col 255 255 231 178),
  (Col 255 190 138 62),
  70
)
$g.FillPolygon($triBrush, $tri)

# Engraved facet lines.
$engrave = New-Object System.Drawing.Pen (Col 210 92 56 18), 9
$engrave.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
foreach ($segment in @(
  @($top, $tt), @($ul, $tl), @($ur, $tr), @($ll, $tl), @($lr, $tr),
  @($bottom, (P 512 642)), @($tt, $tl), @($tl, $tr), @($tr, $tt)
)) {
  $g.DrawLine($engrave, $segment[0], $segment[1])
}

# Gold highlight on top edges.
$shine = New-Object System.Drawing.Pen (Col 190 255 246 213), 3
foreach ($segment in @(@($top, $ur), @($top, $ul), @($tt, $tl), @($tt, $tr))) {
  $g.DrawLine($shine, $segment[0], $segment[1])
}
$engrave.Dispose(); $shine.Dispose(); $triBrush.Dispose(); $dieBrush.Dispose()

# No loose embers: at 32x32 they read as dust on the icon.

$g.Dispose()
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "Wrote $out"
