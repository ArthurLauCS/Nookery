# 算磁贴配色在最坏背景下的 WCAG 对比度：底色 tint 以 alpha 盖在“透过亚克力看到的背景”上，
# 文字再以 alpha 盖在底色上。合成按 sRGB 值线性混合（和 D2D/DWM 的 UNORM 合成一致），最后转亮度。
function Lin([double]$c) { if ($c -le 0.04045) { $c / 12.92 } else { [math]::Pow(($c + 0.055) / 1.055, 2.4) } }
function Lum($rgb) { 0.2126 * (Lin $rgb[0]) + 0.7152 * (Lin $rgb[1]) + 0.0722 * (Lin $rgb[2]) }
function Over($fg, [double]$a, $bg) { @(0..2 | ForEach-Object { $a * $fg[$_] + (1 - $a) * $bg[$_] }) }
function Ratio($x, $y) { $l1 = Lum $x; $l2 = Lum $y; ([math]::Max($l1, $l2) + 0.05) / ([math]::Min($l1, $l2) + 0.05) }
function Hex([string]$h) { @(0, 2, 4 | ForEach-Object { [Convert]::ToInt32($h.Substring($_, 2), 16) / 255 }) }

$themes = @(
    @{ Name = '深色'; Tint = '1C1D1B'; TintA = 0.9; Behind = @('FFFFFF', '808080', '000000'); Text = 'DCDAD3'; Muted = 'DCDAD3'; MutedA = 0.86 },
    @{ Name = '浅色'; Tint = 'FAFAF8'; TintA = 0.9; Behind = @('000000', '808080', 'FFFFFF'); Text = '3F4650'; Muted = '3F4650'; MutedA = 0.86 }
)
foreach ($t in $themes) {
    foreach ($b in $t.Behind) {
        $bg = Over (Hex $t.Tint) $t.TintA (Hex $b)
        $text = Hex $t.Text
        $muted = Over (Hex $t.Muted) $t.MutedA $bg
        "{0} 背后 #{1}：正文 {2:N2}:1，次要文字 {3:N2}:1" -f $t.Name, $b, (Ratio $text $bg), (Ratio $muted $bg)
    }
}
