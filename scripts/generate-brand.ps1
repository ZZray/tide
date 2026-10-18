# Rebuild raster/icon derivatives of assets/brand/tide-icon.svg. Windows only.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$brandDirectory = Join-Path (Split-Path $PSScriptRoot -Parent) 'assets\brand'
function New-TideImage([int]$Size, [bool]$Tile) {
    $bitmap = [Drawing.Bitmap]::new($Size,$Size,[Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.Clear([Drawing.Color]::Transparent)
        $graphics.ScaleTransform($Size/256.0,$Size/256.0)
        if ($Tile) {
            $background = [Drawing.Drawing2D.GraphicsPath]::new()
            $brush = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#101E25'))
            try {
                foreach ($arc in @(@(0,0,180),@(144,0,270),@(144,144,0),@(0,144,90))) { $background.AddArc($arc[0],$arc[1],112,112,$arc[2],90) }
                $background.CloseFigure()
                $graphics.FillPath($brush,$background)
            } finally { $background.Dispose();$brush.Dispose() }
        }
        $pen = [Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml('#9BE0E7'),22)
        $pen.StartCap = [Drawing.Drawing2D.LineCap]::Round
        $pen.EndCap = [Drawing.Drawing2D.LineCap]::Round
        try {
            foreach ($offset in @(0,52)) {
                $wave = [Drawing.Drawing2D.GraphicsPath]::new()
                try {
                    $wave.AddBezier(52,103+$offset,78,75+$offset,103,75+$offset,128,101+$offset)
                    $wave.AddBezier(128,101+$offset,153,127+$offset,178,128+$offset,204,101+$offset)
                    $graphics.DrawPath($pen,$wave)
                } finally { $wave.Dispose() }
            }
        } finally { $pen.Dispose() }
    } finally { $graphics.Dispose() }
    return $bitmap
}
$frames = @()
foreach ($size in @(16,24,32,48,64,128,256)) {
    $bitmap = New-TideImage $size $true
    $memory = [IO.MemoryStream]::new()
    try {
        $bitmap.Save($memory,[Drawing.Imaging.ImageFormat]::Png)
        $frames += ,$memory.ToArray()
        if ($size -eq 256) { [IO.File]::WriteAllBytes((Join-Path $brandDirectory 'tide-256.png'),$memory.ToArray()) }
    } finally { $bitmap.Dispose(); $memory.Dispose() }
}
$tray = New-TideImage 64 $false
try { $tray.Save((Join-Path $brandDirectory 'tide-tray.png'),[Drawing.Imaging.ImageFormat]::Png) } finally { $tray.Dispose() }
$icon = [IO.File]::Create((Join-Path $brandDirectory 'tide.ico'))
$writer = [IO.BinaryWriter]::new($icon)
try {
    $writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]$frames.Count)
    $offset = 6 + 16*$frames.Count
    $sizes = @(16,24,32,48,64,128,256)
    for ($index=0;$index -lt $frames.Count;$index++) {
        $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
        $writer.Write([byte]$dimension);$writer.Write([byte]$dimension)
        $writer.Write([byte]0);$writer.Write([byte]0);$writer.Write([uint16]1);$writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$index].Length);$writer.Write([uint32]$offset)
        $offset += $frames[$index].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose();$icon.Dispose() }
Write-Output "Generated PNG and multi-resolution ICO assets in $brandDirectory"
