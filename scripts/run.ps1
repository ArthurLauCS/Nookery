# 用假桌面启动 Nookery，检查文件不被挪动、旧版 .Nookery 布局搬回桌面、内存和 CPU，并截屏
param([string]$Shot = 'C:\WorkFile\Temp\nookery-test\shot.png', [int]$Wait = 6, [switch]$Keep)
$d = 'C:\WorkFile\Temp\fakedesk'
New-Item -ItemType Directory -Force (Split-Path $Shot) | Out-Null
$exe = "$PSScriptRoot\..\target\release\nookery.exe"
# 旧版布局：归进磁贴的文件在 .Nookery\<磁贴名>\ 里，启动后应搬回桌面根目录、留在原磁贴
New-Item -ItemType Directory -Force "$d\.Nookery\文档" | Out-Null
'x' | Set-Content "$d\.Nookery\文档\旧版周报.docx"
"--- 启动前根目录：" + ((Get-ChildItem $d | ForEach-Object Name) -join ' | ')
$p = Start-Process $exe -ArgumentList '--root', $d, '--top' -PassThru
Start-Sleep $Wait
"--- $Wait 秒后根目录：" + ((Get-ChildItem $d | ForEach-Object Name) -join ' | ')
"--- .Nookery 里剩下：" + ((Get-ChildItem "$d\.Nookery" -Force | ForEach-Object Name) -join ' | ')
"--- 配置里的归属：" + ((Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^map\|') -join ' ; ')
$p.Refresh(); $c0 = $p.TotalProcessorTime.TotalMilliseconds; Start-Sleep 5; $p.Refresh()
"--- 私有内存 {0:N1} MB，工作集 {1:N1} MB，线程 {2}，静置 5 秒 CPU {3:N1} ms" -f ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB), $p.Threads.Count, ($p.TotalProcessorTime.TotalMilliseconds - $c0)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save($Shot); $g.Dispose(); $bmp.Dispose()
"--- 截图 $($b.Width)x$($b.Height) -> $Shot"
if (-not $Keep) { $p.Kill() }
