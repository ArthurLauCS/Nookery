# 下载磁贴名用的朱雀仿宋（TrionesType/zhuque，SIL OFL 1.1）到 assets/fonts（不进 git，约 8.4 MB）。
# 开发运行和打安装包前各跑一次；文件已在且哈希对得上就跳过。
# 先校验压缩包的 SHA-256，再校验解出来的 ttf。
$dir = "$PSScriptRoot\..\assets\fonts"
New-Item -ItemType Directory -Force $dir | Out-Null
$fonts = @(
    @{
        Name = 'ZhuqueFangsong-Regular.ttf'
        Sha256 = '558c62730844fe54ba220146ed62f859d4e2880188d92d985f8921c6e3743bc4'
        Url = 'https://github.com/TrionesType/zhuque/releases/download/v0.212/ZhuqueFangsong-v0.212.zip'
        ZipSha256 = 'bb8b661a7643d2296a72d9d10530a00949419c4e527fb61783f73c2ba1a8c062'
    }
)
Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($f in $fonts) {
    $path = Join-Path $dir $f.Name
    if ((Test-Path $path) -and (Get-FileHash $path -Algorithm SHA256).Hash -eq $f.Sha256) { continue }
    $zip = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName() + '.zip')
    try {
        # Windows PowerShell 5.1 的 Invoke-WebRequest 连 GitHub 发布文件常报 unexpected EOF，curl.exe 走代理环境变量，稳定
        curl.exe -fsSL -o $zip $f.Url
        if ($LASTEXITCODE) { throw "$($f.Url) 下载失败" }
        $zh = (Get-FileHash $zip -Algorithm SHA256).Hash
        if ($zh -ne $f.ZipSha256) { throw "$($f.Url) SHA-256 mismatch: $zh" }
        $za = [IO.Compression.ZipFile]::OpenRead($zip)
        try {
            $e = $za.Entries | Where-Object { $_.Name -eq $f.Name } | Select-Object -First 1
            if (-not $e) { throw "$($f.Name) 不在压缩包里" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($e, $path, $true)
        } finally { $za.Dispose() }
    } finally { Remove-Item $zip -ErrorAction SilentlyContinue }
    $hash = (Get-FileHash $path -Algorithm SHA256).Hash
    if ($hash -ne $f.Sha256) { throw "$($f.Name) SHA-256 mismatch: $hash" }
}
