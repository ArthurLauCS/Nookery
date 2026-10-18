# 设计走查取证：深色 100% / 75% / 50% / 25% / 0%、浅色 100% 六种样式 × 锁定、折叠、选中状态，外加改名框、提示浮层、去色版。
# 截图存到 C:\WorkFile\Temp\nookery-test\round-N\
param([int]$Round = 1)
$d = 'C:\WorkFile\Temp\fakedesk'
$out = "C:\WorkFile\Temp\nookery-test\round-$Round"
$drive = "$PSScriptRoot\..\target\release\examples\drive.exe"
$exe = "$PSScriptRoot\..\target\release\nookery.exe"
New-Item -ItemType Directory -Force $out | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
# 先铺假壁纸，磁贴用 --top 盖在它上面
$bg = Start-Process powershell -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "$PSScriptRoot\backdrop.ps1" -PassThru
Start-Sleep 3

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

foreach ($v in @(@('dark', 100), @('dark', 75), @('dark', 50), @('dark', 25), @('dark', 0), @('light', 100))) {
    $theme, $opacity = $v
    & "$PSScriptRoot\setup.ps1" -Bulk 40
    'x' | Set-Content "$d\Quarterly_Financial_Report_第三季度财务报告_最终版_v12_带批注.xlsx"
    New-Item -ItemType Directory -Force "$d\.Nookery\空磁贴" | Out-Null
    # 只写透明度：没有磁贴和规则时按默认配置补齐
    "opacity|$opacity" | Set-Content "$d\.Nookery\config.txt" -Encoding UTF8
    $p = Start-Process $exe -ArgumentList '--root', $d, '--top', "--$theme" -PassThru
    Start-Sleep 7
    # 标题栏 340 宽：锁定按钮在 (322, 18)，折叠按钮在 (294, 18)
    & $drive click 文档 322 18
    & $drive click 图片 294 18
    & $drive click 文档 60 80
    Start-Sleep 1
    Shot "$theme-$opacity"
    if ($theme -eq 'dark' -and $opacity -eq 100) {
        # 锁定的磁贴单击名字直接进入改名
        & $drive click 文档 170 18
        Start-Sleep 1
        Shot "$theme-$opacity-title-edit"
        & $drive rename 文档 工作文档
        Start-Sleep 1
        "--- 改名后配置：" + ((Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^tile\|(工作文档|图片)\|') -join ' ; ')
        & $drive dblclk 2900 760
        Start-Sleep -Milliseconds 500
        Shot "$theme-$opacity-toast"
        # 第二个进程带 --new-tile：应转给正在运行的实例并立即退出
        $p2 = Start-Process $exe -ArgumentList '--root', $d, '--new-tile' -PassThru
        Start-Sleep 2
        "--- 第二个进程已退出=" + $p2.HasExited + "；新磁贴：" + ((Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^tile\|新磁贴') -join ' ; ')
    }
    $p.Kill()
    Start-Sleep 1
}
$bg.Kill()
"截图在 $out"
