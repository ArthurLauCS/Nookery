# 设计走查取证：深浅两套主题 × 各种状态截图，外加去色版，存到 C:\WorkFile\Temp\desktiles-test\round-N\
param([int]$Round = 1)
$d = 'C:\WorkFile\Temp\fakedesk'
$out = "C:\WorkFile\Temp\desktiles-test\round-$Round"
$drive = 'C:\WorkFile\CodeBase\DeskTiles\target\release\examples\drive.exe'
$exe = 'C:\WorkFile\CodeBase\DeskTiles\target\release\desktiles.exe'
New-Item -ItemType Directory -Force $out | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

function Shot([string]$name) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap 1100, 1440
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Right - 1100, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save("$out\$name.png")
    # 去色版：按亮度系数灰度化，检查层级
    $gray = New-Object System.Drawing.Bitmap 1100, 1440
    $gg = [System.Drawing.Graphics]::FromImage($gray)
    $m = New-Object System.Drawing.Imaging.ColorMatrix(, [single[][]]@(
            [single[]]@(0.2126, 0.2126, 0.2126, 0, 0), [single[]]@(0.7152, 0.7152, 0.7152, 0, 0),
            [single[]]@(0.0722, 0.0722, 0.0722, 0, 0), [single[]]@(0, 0, 0, 1, 0), [single[]]@(0, 0, 0, 0, 1)))
    $ia = New-Object System.Drawing.Imaging.ImageAttributes; $ia.SetColorMatrix($m)
    $gg.DrawImage($bmp, (New-Object System.Drawing.Rectangle 0, 0, 1100, 1440), 0, 0, 1100, 1440, [System.Drawing.GraphicsUnit]::Pixel, $ia)
    $gray.Save("$out\$name-gray.png")
    $g.Dispose(); $bmp.Dispose(); $gg.Dispose(); $gray.Dispose()
}

foreach ($theme in 'dark', 'light') {
    & "$PSScriptRoot\setup.ps1" -Bulk 40
    # 极值内容：超长中英混排文件名；一个空的自建磁贴；一个放原版图标快捷方式的磁贴
    'x' | Set-Content "$d\Quarterly_Financial_Report_第三季度财务报告_最终版_v12_带批注.xlsx"
    New-Item -ItemType Directory -Force "$d\.DeskTiles\空磁贴", "$d\.DeskTiles\IconCheck" | Out-Null
    $ws = New-Object -ComObject WScript.Shell
    foreach ($i in 183, 105, 27) { $s = $ws.CreateShortcut("$d\.DeskTiles\IconCheck\imageres_$i.lnk"); $s.TargetPath = 'C:\Windows\notepad.exe'; $s.IconLocation = "C:\Windows\System32\imageres.dll,-$i"; $s.Save() }
    $p = Start-Process $exe -ArgumentList '--root', $d, '--top', "--$theme" -PassThru
    Start-Sleep 7
    Shot "$theme-1-default"
    & $drive click 文档 60 80
    & $drive down 文档 170 80
    Start-Sleep 1
    Shot "$theme-2-selected-pressed"
    & $drive up 文档 170 80
    & $drive key 文档 39
    & $drive key 文档 40
    Start-Sleep 1
    Shot "$theme-3-keyboard-focus"
    & $drive size 文档 150 400
    & $drive rect 其他 2350 1100 720 250
    & $drive roll 音频
    Start-Sleep 1
    Shot "$theme-4-narrow-wide-rolled"
    $p.Kill()
    Start-Sleep 1
}
"截图在 $out"
