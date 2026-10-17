# 用假桌面启动 DeskTiles，检查归类结果、内存和 CPU，并截屏
param([string]$Shot = 'C:\WorkFile\Temp\desktiles-test\shot.png', [int]$Wait = 6, [switch]$Keep)
$d = 'C:\WorkFile\Temp\fakedesk'
New-Item -ItemType Directory -Force (Split-Path $Shot) | Out-Null
$exe = 'C:\WorkFile\CodeBase\DeskTiles\target\release\desktiles.exe'
# 占住一个文件，验证“正在使用的文件不搬”
$lock = [IO.File]::Open("$d\笔记.txt", 'Open', 'Read', 'None')
$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process $exe -ArgumentList '--root', $d, '--top' -PassThru
Start-Sleep $Wait
"--- $Wait 秒后桌面根目录剩下："
Get-ChildItem $d -Force | ForEach-Object { $_.Name }
$lock.Close()
Start-Sleep 3
"--- 解除占用 3 秒后根目录："
Get-ChildItem $d | ForEach-Object { $_.Name }
"--- 磁贴目录："
Get-ChildItem "$d\.DeskTiles" -Directory | ForEach-Object { "{0}: {1}" -f $_.Name, ((Get-ChildItem $_.FullName | ForEach-Object Name) -join ' | ') }
$p.Refresh(); $c0 = $p.TotalProcessorTime.TotalMilliseconds; Start-Sleep 5; $p.Refresh()
"--- 私有内存 {0:N1} MB，工作集 {1:N1} MB，线程 {2}，静置 5 秒 CPU {3:N1} ms" -f ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB), $p.Threads.Count, ($p.TotalProcessorTime.TotalMilliseconds - $c0)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save($Shot); $g.Dispose(); $bmp.Dispose()
"--- 截图 $($b.Width)x$($b.Height) -> $Shot"
if (-not $Keep) { $p.Kill() }
