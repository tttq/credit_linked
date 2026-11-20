Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

function New-Icon([int]$size, [string]$path) {
  $bmp = New-Object System.Drawing.Bitmap($size, $size)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'
  $g.TextRenderingHint = 'AntiAliasGridFit'
  $g.Clear([System.Drawing.Color]::Transparent)

  $rect = New-Object System.Drawing.Rectangle(2, 2, ($size - 4), ($size - 4))
  $r = [Math]::Max(4, [int]($size * 0.22))
  $pathObj = New-Object System.Drawing.Drawing2D.GraphicsPath
  $pathObj.AddArc($rect.X, $rect.Y, 2*$r, 2*$r, 180, 90)
  $pathObj.AddArc($rect.Right - 2*$r, $rect.Y, 2*$r, 2*$r, 270, 90)
  $pathObj.AddArc($rect.Right - 2*$r, $rect.Bottom - 2*$r, 2*$r, 2*$r, 0, 90)
  $pathObj.AddArc($rect.X, $rect.Bottom - 2*$r, 2*$r, 2*$r, 90, 90)
  $pathObj.CloseFigure()

  $c1 = [System.Drawing.Color]::FromArgb(255, 104, 98, 237)
  $c2 = [System.Drawing.Color]::FromArgb(255, 157, 123, 240)
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $c1, $c2, 45.0)
  $g.FillPath($brush, $pathObj)

  # white router glyph: two nodes + link, simplified
  $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, [Math]::Max(1.6, $size * 0.075))
  $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
  $cx = $size * 0.5
  # horizontal link
  $g.DrawLine($pen, $size*0.30, $size*0.52, $size*0.70, $size*0.52)
  # left node
  $nr = $size * 0.085
  $g.FillEllipse([System.Drawing.Brushes]::White, $size*0.30-$nr, $size*0.52-$nr, 2*$nr, 2*$nr)
  # right node
  $g.FillEllipse([System.Drawing.Brushes]::White, $size*0.70-$nr, $size*0.52-$nr, 2*$nr, 2*$nr)
  # up link from center
  $g.DrawLine($pen, $cx, $size*0.52, $cx, $size*0.30)
  $g.FillEllipse([System.Drawing.Brushes]::White, $cx-$nr, $size*0.30-$nr, 2*$nr, 2*$nr)
  # down link from center
  $g.DrawLine($pen, $cx, $size*0.52, $cx, $size*0.72)
  $g.FillEllipse([System.Drawing.Brushes]::White, $cx-$nr, $size*0.72-$nr, 2*$nr, 2*$nr)
  $g.Dispose()

  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}

$iconDir = 'E:\auto_change\credit-rs\src-tauri\icons'
New-Item -ItemType Directory -Force -Path $iconDir | Out-Null

$sizes = @(256, 128, 64, 48, 32, 24, 16)
$tmp = 'E:\auto_change\credit-rs\.agent-kernel\icon_tmp'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
foreach ($s in $sizes) { New-Icon $s ("$tmp\icon_$s.png") | Out-Null }

# ico with multiple sizes via IconWriter (manual binary)
function Png-ToIco([string[]]$pngPaths, [string]$icoPath) {
  $ms = New-Object System.IO.MemoryStream
  $bw = New-Object System.IO.BinaryWriter($ms)
  $bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$pngPaths.Count)
  $offset = 6 + 16 * $pngPaths.Count
  foreach ($p in $pngPaths) {
    $bytes = [IO.File]::ReadAllBytes($p)
    $img = [System.Drawing.Image]::FromFile($p)
    $w = $img.Width; $h = $img.Height
    $img.Dispose()
    $bw.Write([Byte]($(if ($w -ge 256) { 0 } else { $w })))
    $bw.Write([Byte]($(if ($h -ge 256) { 0 } else { $h })))
    $bw.Write([Byte]0); $bw.Write([Byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$bytes.Length)
    $bw.Write([UInt32]$offset)
    $offset += $bytes.Length
  }
  foreach ($p in $pngPaths) { $bw.Write([IO.File]::ReadAllBytes($p)) }
  $bw.Flush()
  [IO.File]::WriteAllBytes($icoPath, $ms.ToArray())
  $bw.Dispose(); $ms.Dispose()
}

$icoPngs = @("$tmp\icon_256.png","$tmp\icon_128.png","$tmp\icon_64.png","$tmp\icon_48.png","$tmp\icon_32.png","$tmp\icon_16.png")
Png-ToIco $icoPngs "$iconDir\icon.ico"
Copy-Item "$tmp\icon_256.png" "$iconDir\icon.png" -Force
"done: " + (Get-Item "$iconDir\icon.ico").Length + " / " + (Get-Item "$iconDir\icon.png").Length
