# 走查用的假壁纸：在主屏右侧铺一张高饱和渐变加色块的无边框窗口，垫在磁贴（--top）下面，
# 用来看透明度和白字阴影在花哨背景上的效果。不动真实壁纸。
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$w, $h = 1100, $b.Height
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, [System.Drawing.Color]::FromArgb(255, 255, 170, 60), [System.Drawing.Color]::FromArgb(255, 40, 110, 230), 60.0)
$g.FillRectangle($grad, $rect)
# 一块近白、一块高饱和，覆盖最坏的两种背景
$g.FillEllipse((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 250, 250, 245))), 600, 120, 520, 420)
$g.FillEllipse((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 230, 40, 120))), 380, 700, 460, 380)
$g.Dispose()
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = 'None'; $f.ShowInTaskbar = $false; $f.StartPosition = 'Manual'
$f.Location = New-Object System.Drawing.Point ($b.Right - $w), $b.Top
$f.Size = New-Object System.Drawing.Size $w, $h
$f.BackgroundImage = $bmp
[System.Windows.Forms.Application]::Run($f)
