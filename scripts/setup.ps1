# 造一个假桌面：各类散文件 + 快捷方式 + 下载中的临时文件 + 被占用的文件
param([int]$Bulk = 0)
$d = 'C:\WorkFile\Temp\fakedesk'
if (Test-Path $d) { Remove-Item $d -Recurse -Force }
New-Item -ItemType Directory $d | Out-Null
Add-Type -AssemblyName System.Drawing
foreach ($n in '风景.png', '截图 2026-10-17.png') {
    $b = New-Object System.Drawing.Bitmap 256, 160
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 40, 120, 200)); $g.FillEllipse([System.Drawing.Brushes]::Orange, 40, 30, 120, 100)
    $b.Save("$d\$n"); $g.Dispose(); $b.Dispose()
}
'周报内容' | Set-Content "$d\周报.docx"
'x' | Set-Content "$d\季度预算表.xlsx"
'x' | Set-Content "$d\一个名字特别特别长的项目需求说明文档最终版.pdf"
'x' | Set-Content "$d\笔记.txt"
'x' | Set-Content "$d\素材.zip"
'x' | Set-Content "$d\setup.exe"
'x' | Set-Content "$d\未知格式.xyz"
'x' | Set-Content "$d\电影.mp4.crdownload"
'[InternetShortcut]' | Set-Content "$d\官网.url"
$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut("$d\Notepad.lnk"); $s.TargetPath = 'C:\Windows\notepad.exe'; $s.Save()
New-Item -ItemType Directory "$d\项目资料", "$d\照片备份" | Out-Null
for ($i = 1; $i -le $Bulk; $i++) { 'x' | Set-Content ("$d\批量文件{0:D3}.{1}" -f $i, @('txt', 'docx', 'png', 'zip', 'mp3')[$i % 5]) }
