//! 随程序带的字体：磁贴名用朱雀仿宋（SIL OFL 1.1，许可证见 THIRD_PARTY_NOTICES.txt）。
//! 字体文件在 exe 旁的 `fonts` 文件夹（安装版），开发时在仓库的 `assets/fonts`（scripts/fonts.ps1 下载）。
//! 文件缺失或损坏不报错，调用方退回系统的微软雅黑 UI。
//! DirectWrite 用字体集（IDWriteFontSetBuilder）装载；GDI 的 FR_PRIVATE 字体 DirectWrite 看不见，两边各装各的。

use std::path::PathBuf;

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;

/// 磁贴名字体文件名
pub const DISPLAY_FILE: &str = "ZhuqueFangsong-Regular.ttf";

/// 找字体文件：exe 旁的 fonts，再往上几级找 assets/fonts（target/release 下运行时在仓库根）
pub fn find(file: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    let first = dir.join("fonts").join(file);
    if first.is_file() {
        return Some(first);
    }
    for _ in 0..4 {
        let p = dir.join("assets").join("fonts").join(file);
        if p.is_file() {
            return Some(p);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// 装载好的磁贴名字体：字体集合和族名
pub struct Display {
    pub collection: IDWriteFontCollection,
    pub family: String,
}

/// 把文件装成只含它的字体集合，读出族名；任何一步失败返回 None
pub fn load_display(dw: &IDWriteFactory) -> Option<Display> {
    unsafe {
        let path = find(DISPLAY_FILE)?;
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        let file = dw.CreateFontFileReference(PCWSTR(wide.as_ptr()), None).ok()?;
        let f3: IDWriteFactory3 = dw.cast().ok()?;
        let builder: IDWriteFontSetBuilder1 = f3.CreateFontSetBuilder().ok()?.cast().ok()?;
        builder.AddFontFile(&file).ok()?;
        let set = builder.CreateFontSet().ok()?;
        let coll: IDWriteFontCollection = f3.CreateFontCollectionFromFontSet(&set).ok()?.cast().ok()?;
        if coll.GetFontFamilyCount() == 0 {
            return None;
        }
        let names = coll.GetFontFamily(0).ok()?.GetFamilyNames().ok()?;
        let len = names.GetStringLength(0).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        names.GetString(0, &mut buf).ok()?;
        Some(Display { collection: coll, family: String::from_utf16_lossy(&buf[..len]) })
    }
}

/// 字体里没有的字（用户随手打的生僻字、符号）先回退到 `family`（微软雅黑 UI），再走系统回退链，不出方框
pub fn fallback(dw: &IDWriteFactory, family: PCWSTR) -> Option<IDWriteFontFallback> {
    unsafe {
        let f2: IDWriteFactory2 = dw.cast().ok()?;
        let b = f2.CreateFontFallbackBuilder().ok()?;
        let all = [DWRITE_UNICODE_RANGE { first: 0, last: 0x10FFFF }];
        b.AddMapping(&all, &[family.0], None::<&IDWriteFontCollection>, w!("zh-CN"), None::<&PCWSTR>, 1.0).ok()?;
        b.AddMappings(&f2.GetSystemFontFallback().ok()?).ok()?;
        b.CreateFontFallback().ok()
    }
}

/// GDI 里的字体名最长 31 个字符，英文族名 "Zhuque Fangsong (technical preview)" 放不下；
/// 字体里的简体中文族名可以（GDI 按名字表里的任一语言匹配）
pub const DISPLAY_GDI_FACE: &str = "朱雀仿宋（预览测试版）";

/// 给 GDI 私有装载字体文件（只在本进程可见，退出即卸）；成功返回 true
pub fn register_gdi(file: &str) -> bool {
    use windows::Win32::Graphics::Gdi::{AddFontResourceExW, FR_PRIVATE};
    let Some(path) = find(file) else { return false };
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    unsafe { AddFontResourceExW(PCWSTR(wide.as_ptr()), FR_PRIVATE, None) > 0 }
}

/// GDI 找不到字体名时会悄悄换一个别的字体：选进 DC 读回实际用的字体名，对不上就不用
pub fn gdi_face_is(font: windows::Win32::Graphics::Gdi::HFONT, want: &str) -> bool {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, font.into());
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, Some(&mut buf)) as usize;
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        n > 1 && String::from_utf16_lossy(&buf[..n - 1]).eq_ignore_ascii_case(want)
    }
}
