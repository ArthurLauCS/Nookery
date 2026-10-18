# 不动真实鼠标：用 drive.exe 给磁贴窗口发消息，测悬停、选中、F2 改名、折叠
$d = 'C:\WorkFile\Temp\fakedesk'
$t = 'C:\WorkFile\Temp\nookery-test'; New-Item -ItemType Directory -Force $t | Out-Null
$drive = "$PSScriptRoot\..\target\release\examples\drive.exe"
$p = Start-Process "$PSScriptRoot\..\target\release\nookery.exe" -ArgumentList '--root', $d, '--top' -PassThru
Start-Sleep 5
& $drive click 文档 60 80
& $drive hover 文档 170 80
Start-Sleep 1
& "$PSScriptRoot\shot.ps1" "$t\hover.png"
& $drive key 文档 113
Start-Sleep 1
& "$PSScriptRoot\shot.ps1" "$t\rename.png"
& $drive rename 文档 需求说明.pdf
Start-Sleep 2
"--- 文档目录：" + ((Get-ChildItem "$d\.Nookery\文档" | ForEach-Object Name) -join ' | ')
# 标题栏右上角的折叠按钮（340 宽时在 294, 18）
& $drive click 图片 294 18
Start-Sleep 1
"--- 配置里的图片磁贴：" + (Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^tile\|图片')
& "$PSScriptRoot\shot.ps1" "$t\rolled.png"
# 从磁贴拖回桌面的文件应留在桌面；删掉后“留在桌面”的记录也清掉
Move-Item "$d\.Nookery\文档\周报.docx" "$d\周报.docx"
Start-Sleep 5
"--- 拖回桌面 5 秒后：根目录有周报=" + (Test-Path "$d\周报.docx") + "，配置 keep=" + ((Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^keep\|') -join ',')
Remove-Item "$d\周报.docx"
Start-Sleep 2
"--- 删掉后配置 keep=" + ((Get-Content "$d\.Nookery\config.txt" -Encoding UTF8 | Select-String '^keep\|') -join ',')
$p.Kill()