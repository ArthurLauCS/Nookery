//! 配色和字体：ui-style 目录里的 washi（日系清新，宣纸、发丝线、轻字重）。
//! 所有色值、字号、圆角只在这个文件里；绘制代码只引用 Theme 的字段和这里的常量。
//! accent 取应用图标的蓝（App accents 表里 Nookery 一行）。

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

/// 0xRRGGBB
type Hex = u32;

/// 一种模式下的全部色值（washi 表）
struct Palette {
    bg: Hex,
    surface: Hex,
    raised: Hex,
    border: Hex,
    text: Hex,
    muted: Hex,
    accent: Hex,
    /// 选中底色 = accent 加这个透明度（0~255）
    selection_a: u8,
}

const LIGHT: Palette = Palette { bg: 0xFAFAF8, surface: 0xF3F2EE, raised: 0xEAE8E2, border: 0xDCDAD3, text: 0x3F4650, muted: 0x626B76, accent: 0x1F5FC8, selection_a: 0x24 };
const DARK: Palette = Palette { bg: 0x1C1D1B, surface: 0x222320, raised: 0x2A2B28, border: 0x3A3B37, text: 0xDCDAD3, muted: 0x9B998F, accent: 0x6EA2FF, selection_a: 0x42 };

// 字体按角色（ui-style 的 Type roles，覆盖 washi 的 Type 行）：
// DISPLAY 磁贴名 = 朱雀仿宋（随程序带，见 fonts.rs；只有 Regular 一个字重，缺文件时退回 UI 字体）；
// UI 文件名、提示 = 微软雅黑 UI（12 px 压在壁纸上也清楚；字重只有 300 / 400 / 700，不用粗体）。
// 没有数字或路径要显示，MONO（Cascadia Mono）不用。
pub const FONT_UI: &str = "Microsoft YaHei UI";
pub const WEIGHT_BODY: i32 = 400;
/// 磁贴名：朱雀仿宋只有 Regular
pub const WEIGHT_TITLE: i32 = 400;
/// 字号 12 / 13 / 15 / 18 / 24 里用到的三档
pub const SIZE_LABEL: f32 = 12.0;
pub const SIZE_TOAST: f32 = 13.0;
pub const SIZE_TITLE: f32 = 18.0;

// 圆角：控件 6，面板和浮层 8
pub const RADIUS_CONTROL: f32 = 6.0;
pub const RADIUS_PANEL: f32 = 8.0;
/// 线宽：发丝线 1
pub const HAIR: f32 = 1.0;

fn rgb(h: Hex, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: ((h >> 16) & 0xFF) as f32 / 255.0, g: ((h >> 8) & 0xFF) as f32 / 255.0, b: (h & 0xFF) as f32 / 255.0, a }
}

fn colorref(h: Hex) -> COLORREF {
    COLORREF(((h & 0xFF) << 16) | (h & 0xFF00) | ((h >> 16) & 0xFF))
}

pub struct Theme {
    pub dark: bool,
    /// 磁贴底色（bg 加透明度档位）；100% 时不透明，没有亚克力也是完整的纸色
    pub tint: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub muted: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub hover: D2D1_COLOR_F,
    pub press: D2D1_COLOR_F,
    pub sel_fill: D2D1_COLOR_F,
    pub sel_line: D2D1_COLOR_F,
    /// 键盘焦点外面那圈 3 px 的淡光环
    pub ring: D2D1_COLOR_F,
    /// 透明度 ≥50% 时开亚克力模糊（磨砂），25% 和 0% 不模糊
    pub blur: bool,
    /// 透明度 ≤50% 时文字压在壁纸上：白字加阴影，和桌面图标一致
    pub shadow: bool,
    pub shade: D2D1_COLOR_F,
    /// 全透明时自己画的淡边框；Some 也表示关掉 DWM 的边框和阴影
    pub edge: Option<D2D1_COLOR_F>,
    pub edge_in: D2D1_COLOR_F,
    /// 全透明时鼠标移进来显出的淡底色，方便看清磁贴范围
    pub peek: D2D1_COLOR_F,
    /// 浮层（提示）：底、字、发丝边
    pub pop_bg: D2D1_COLOR_F,
    pub pop_text: D2D1_COLOR_F,
    pub pop_line: D2D1_COLOR_F,
    pub edit_bg: COLORREF,
    pub edit_fg: COLORREF,
}

impl Theme {
    /// opacity 是 0~100 的透明度档位；透明度 ≤50% 时不管系统主题都用夜色那一套（文字压在壁纸上）
    pub fn new(system_dark: bool, opacity: u8) -> Theme {
        let o = opacity as f32 / 100.0;
        let desk = o <= 0.5;
        // 磨砂档（>50% 且 <100%）：底色至少 90% 不透明，最坏的壁纸下次要文字也 ≥4.5:1（scripts/contrast.ps1）
        let frosted = o > 0.5 && o < 1.0;
        let dark = system_dark || desk;
        let p = if dark { &DARK } else { &LIGHT };
        let (hover, press) = if desk {
            // 壁纸上不知道底是深是浅：用文字色的淡层，不用不透明的 raised
            (rgb(p.text, 0.14), rgb(p.text, 0.24))
        } else {
            (rgb(p.raised, 1.0), rgb(p.border, 1.0))
        };
        Theme {
            dark,
            tint: rgb(p.bg, if frosted { 0.9 } else { o }),
            text: rgb(p.text, 1.0),
            // 底色可能透出壁纸：次要文字用 text 的 86%；100% 时是纸色底，直接用 muted
            muted: if desk || frosted { rgb(p.text, 0.86) } else { rgb(p.muted, 1.0) },
            accent: rgb(p.accent, 1.0),
            hover,
            press,
            sel_fill: rgb(p.accent, p.selection_a as f32 / 255.0),
            sel_line: rgb(p.accent, 0.6),
            ring: rgb(p.accent, 0.12),
            blur: o >= 0.5,
            shadow: desk,
            shade: D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.5 },
            // 外圈淡亮、内圈淡暗：深浅壁纸上都看得出一圈不显眼的发丝边
            edge: (opacity == 0).then(|| rgb(p.text, 0.30)),
            edge_in: rgb(DARK.bg, 0.30),
            peek: rgb(p.bg, 0.30),
            pop_bg: rgb(p.bg, 1.0),
            pop_text: rgb(p.text, 1.0),
            pop_line: rgb(p.border, 1.0),
            edit_bg: colorref(p.surface),
            edit_fg: colorref(p.text),
        }
    }
}
