//! 界面语言（中文 / 英文）。存在 HKCU\Software\Nookery 的 Language 值（zh / en）：
//! 安装包按安装时选的语言写入，托盘菜单切换时改写，桌面右键菜单扩展（menu 包）也读这里。

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::System::Registry::*;
use windows::core::*;

const KEY: PCWSTR = w!("Software\\Nookery");
static EN: AtomicBool = AtomicBool::new(false);

/// 读注册表；没设过时跟随系统界面语言，中文系统用中文，其他用英文
pub fn load() {
    let mut buf = [0u16; 8];
    let mut len = size_of_val(&buf) as u32;
    let ok = unsafe { RegGetValueW(HKEY_CURRENT_USER, KEY, w!("Language"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut len)) }.is_ok();
    // 主语言号在低 10 位，0x04 是中文
    let en = if ok { buf.starts_with(&[b'e' as u16, b'n' as u16]) } else { (unsafe { GetUserDefaultUILanguage() } & 0x3FF) != 0x04 };
    EN.store(en, Relaxed);
}

#[allow(dead_code)]
pub fn save(en: bool) {
    EN.store(en, Relaxed);
    let v: Vec<u16> = (if en { "en" } else { "zh" }).encode_utf16().chain([0]).collect();
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, w!("Language"), REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32);
    }
}

pub fn en() -> bool {
    EN.load(Relaxed)
}

pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if self::en() { en } else { zh }
}
