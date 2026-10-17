# 不动真实鼠标：用 drive.exe 给磁贴窗口发消息，测悬停、选中、F2 改名、折叠
$d = 'C:\WorkFile\Temp\fakedesk'
$t = 'C:\WorkFile\Temp\desktiles-test'; New-Item -ItemType Directory -Force $t | Out-Null
$drive = 'C:\WorkFile\CodeBase\DeskTiles\target\release\examples\drive.exe'
$p = Start-Process 'C:\WorkFile\CodeBase\DeskTiles\target\release\desktiles.exe' -ArgumentList '--root', $d, '--top' -PassThru
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
"--- 文档目录：" + ((Get-ChildItem "$d\.DeskTiles\文档" | ForEach-Object Name) -join ' | ')
& $drive roll 图片
Start-Sleep 1
"--- 配置里的图片磁贴：" + (Get-Content "$d\.DeskTiles\config.txt" -Encoding UTF8 | Select-String '^tile\|图片')
& "$PSScriptRoot\shot.ps1" "$t\rolled.png"
$p.Kill()