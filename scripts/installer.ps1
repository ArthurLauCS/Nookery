# 编译 release、给右键菜单的稀疏包打包签名，再打安装包，产物在 target\installer\Nookery-Setup-<版本>.exe。
# 需要 Inno Setup 6 和 Windows SDK（makeappx、signtool）。
# 签名用当前用户证书库里的自签证书 CN=Nookery，没有就新建。换电脑打包会新建另一张，装过旧包的机器升级时会再装一张，不影响使用。
$root = "$PSScriptRoot\.."
$out = "$root\target\installer"
# 只留本次版本的产物，旧版本的安装包不累计
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
# 字体文件不进 git，打包前确保 assets\fonts 里有
& "$PSScriptRoot\fonts.ps1"
cargo build --release --workspace --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

$cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq 'CN=Nookery' | Sort-Object NotAfter | Select-Object -Last 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type Custom -Subject 'CN=Nookery' -KeyUsage DigitalSignature -FriendlyName 'Nookery' -NotAfter (Get-Date).AddYears(10) `
        -CertStoreLocation Cert:\CurrentUser\My -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
}

$pkg = "$out\pkg"
New-Item -ItemType Directory -Force $pkg | Out-Null
(Get-Content "$root\installer\AppxManifest.xml" -Raw -Encoding UTF8).Replace('$VERSION$', "$version.0") | Set-Content "$pkg\AppxManifest.xml" -Encoding UTF8
Copy-Item "$root\assets\logo.png" $pkg
$sdk = (Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" | Sort-Object FullName | Select-Object -Last 1).DirectoryName
& "$sdk\makeappx.exe" pack /o /nv /d $pkg /p "$out\nookery.msix" | Out-Null
if ($LASTEXITCODE) { exit $LASTEXITCODE }
& "$sdk\signtool.exe" sign /q /fd SHA256 /sha1 $cert.Thumbprint "$out\nookery.msix"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Export-Certificate -Cert $cert -FilePath "$out\nookery.cer" | Out-Null

# Inno Setup 可能装在全机或当前用户目录下
$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
& $iscc /Q "/DVersion=$version" "/DCertThumb=$($cert.Thumbprint)" "$root\installer\nookery.iss"
exit $LASTEXITCODE
