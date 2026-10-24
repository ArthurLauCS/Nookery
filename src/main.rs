#![windows_subsystem = "windows"]
//! Nookery：桌面磁贴。桌面上的东西按类型显示在磁贴里，文件留在桌面目录不动；
//! 运行时关掉资源管理器自己的桌面图标，退出时打开。
//!
//! 参数：--root <目录> 用指定目录代替桌面（测试用，配置放进该目录下的 .Nookery，不碰真实桌面图标）；--top 磁贴置顶（截图走查用）；
//! --new-tile 在鼠标处新建磁贴（桌面右键菜单调用，已在运行时转给正在运行的实例）；
//! --uninstall 显示回桌面图标、删掉开机自启和右键菜单后退出（卸载程序调用）。

mod drop;
mod gfx;
mod lang;
mod rules;
mod shell;
mod tile;
mod toast;

use std::collections::HashMap;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use lang::tr;
use rules::{Config, TileCfg};
use tile::{CELL_H, Entry, PAD, TITLE_H, Tile};

pub const TILE_CLASS: PCWSTR = w!("NookeryTile");
const MAIN_CLASS: PCWSTR = w!("NookeryMain");
const WM_APP_TRAY: u32 = WM_APP + 2;
/// 另一个进程（桌面右键菜单）让正在运行的实例在 lparam 指的屏幕坐标处新建磁贴
const WM_APP_NEWTILE: u32 = WM_APP + 4;
/// 旧版把归进磁贴的文件放在桌面下的这个目录，启动时搬回桌面；--root 测试模式的配置也放这里
const DATA_DIR: &str = ".Nookery";
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// 桌面空白处右键菜单里的“新建磁贴”（Win11 下在“显示更多选项”里）
const MENU_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew");
const MENU_CMD_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew\\command");
const T_SCAN: usize = 1;
const OPACITY_LEVELS: [u8; 5] = [100, 75, 50, 25, 0];

pub struct Theme {
    pub dark: bool,
    pub tint: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub muted: D2D1_COLOR_F,
    pub hover: D2D1_COLOR_F,
    pub press: D2D1_COLOR_F,
    pub sel_fill: D2D1_COLOR_F,
    pub sel_line: D2D1_COLOR_F,
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
    pub edit_bg: COLORREF,
    pub edit_fg: COLORREF,
}

pub struct App {
    pub root: PathBuf,
    /// 所有用户共用的桌面；测试模式没有
    public: Option<PathBuf>,
    /// 桌面上要显示的项和所在磁贴，每次桌面变化重扫
    entries: Vec<(Entry, String)>,
    cfg_path: PathBuf,
    pub cfg: Config,
    main: HWND,
    pub owner: Option<HWND>,
    pub tiles: Vec<Box<Tile>>,
    d2d: ID2D1Factory1,
    pub gfx: gfx::Gfx,
    pub dw: IDWriteFactory,
    pub fmt_title: IDWriteTextFormat,
    pub fmt_label: IDWriteTextFormat,
    pub fmt_hint: IDWriteTextFormat,
    /// 标题栏按钮的系统图标字体
    pub fmt_icon: IDWriteTextFormat,
    pub fmt_toast: IDWriteTextFormat,
    /// 改名框字体：文件名 12、磁贴名 14 半粗，和 D2D 里的字号一致
    pub edit_font: HFONT,
    pub title_font: HFONT,
    pub edit_brush: HBRUSH,
    pub theme: Theme,
    pub toast: Option<toast::Toast>,
    pub hide_ext: bool,
    /// 托盘里点了“隐藏磁贴”
    hidden: bool,
    pub topmost: bool,
    /// 系统右键菜单弹出期间，用来转发子菜单的绘制消息
    pub menu2: Option<IContextMenu2>,
    /// 拖进磁贴时显示系统拖拽缩略图
    pub drop_helper: Option<IDropTargetHelper>,
    /// 双击桌面监听线程
    dbl_thread: u32,
    taskbar_created: u32,
    /// --light / --dark：截图走查时强制主题，不读系统设置
    force_dark: Option<bool>,
    tray_icon: HICON,
    /// --root 测试模式：不写桌面右键菜单
    test: bool,
}

static mut APP: *mut App = std::ptr::null_mut();

/// 单线程 UI，全局状态只在主线程上访问
pub fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn reg_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let (mut v, mut n) = (0u32, 4u32);
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut n)) };
    (r == ERROR_SUCCESS).then_some(v)
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// 窗口矩形整个挪进最近显示器的工作区，返回新的左上角
pub fn fit_work_area(r: &RECT) -> (i32, i32) {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(MonitorFromRect(r, MONITOR_DEFAULTTONEAREST), &mut mi);
    }
    let wa = mi.rcWork;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (r.left.clamp(wa.left, (wa.right - w).max(wa.left)), r.top.clamp(wa.top, (wa.bottom - h).max(wa.top)))
}

fn xywh(r: &RECT) -> [i32; 4] {
    [r.left, r.top, r.right - r.left, r.bottom - r.top]
}

/// 显示器的设备名（如 \\.\DISPLAY1）和工作区
fn mon_info(h: HMONITOR) -> (String, RECT) {
    let mut mi = MONITORINFOEXW { monitorInfo: MONITORINFO { cbSize: size_of::<MONITORINFOEXW>() as u32, ..Default::default() }, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO);
    }
    (String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').into(), mi.monitorInfo.rcWork)
}

/// 磁贴配置里的比例换算成像素矩形；它那块显示器不在了（比如拔掉了副屏）就按主屏算，插回来自动回原位
fn tile_px(c: &TileCfg) -> [i32; 4] {
    unsafe extern "system" fn add(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe { (*(lp.0 as *mut Vec<HMONITOR>)).push(h) };
        TRUE
    }
    let mut all = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut all as *mut _ as isize));
    }
    let wa = all.into_iter().map(mon_info).find(|(n, _)| *n == c.mon).map_or_else(|| mon_info(unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }).1, |(_, wa)| wa);
    rules::to_px(c.rect, xywh(&wa))
}

/// 像素矩形所在（离它最近）的显示器，和它在那块显示器工作区里的比例
fn locate(r: [i32; 4]) -> (String, [f32; 4]) {
    let rc = RECT { left: r[0], top: r[1], right: r[0] + r[2], bottom: r[1] + r[3] };
    let (mon, wa) = mon_info(unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST) });
    (mon, rules::to_frac(r, xywh(&wa)))
}

fn is_hidden(md: &std::fs::Metadata) -> bool {
    md.file_attributes() & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0
}

/// 托盘图标用 exe 自带的应用图标（资源号 1，见 build.rs），按系统小图标尺寸取。
/// 不用 LoadIconMetric：它只在 6.0 版通用控件库里有，程序没声明 6.0 时 exe 直接启动失败（2026-10-17 踩过）
fn desktop_icon() -> HICON {
    use windows::Win32::System::LibraryLoader::*;
    unsafe {
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        GetModuleHandleW(None)
            .ok()
            .and_then(|m| LoadImageW(Some(HINSTANCE(m.0)), PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR).ok())
            .map(|h| HICON(h.0))
            .unwrap_or_else(|| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
    }
}

/// 销毁磁贴窗口；之后它的窗口过程不再碰这块磁贴
fn destroy(t: &Tile) {
    unsafe {
        let _ = windows::Win32::System::Ole::RevokeDragDrop(t.hwnd);
        SetWindowLongPtrW(t.hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(t.hwnd);
    }
}

/// 建菜单：id 为 0 的项是分隔线，文字以 ✓ 开头的项打勾
fn menu_from(items: &[(u32, &str)]) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu().unwrap_or_default();
        for (id, text) in items {
            let t = shell::wide(text.trim_start_matches('✓'));
            let flags = if *id == 0 {
                MF_SEPARATOR
            } else if text.starts_with('✓') {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(menu, flags, *id as usize, PCWSTR(t.as_ptr()));
        }
        menu
    }
}

/// 弹出简单菜单，返回选中的 id，没选返回 0
pub fn popup(hwnd: HWND, pt: POINT, items: &[(u32, &str)]) -> u32 {
    track(hwnd, pt, menu_from(items))
}

fn track(hwnd: HWND, pt: POINT, menu: HMENU) -> u32 {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as u32;
        let _ = DestroyMenu(menu);
        id
    }
}

impl App {
    fn load_theme(&mut self) {
        let dark = self.force_dark.unwrap_or_else(|| reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme")) != Some(1));
        // AccentColor 是 0xAABBGGRR
        let c = reg_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor")).unwrap_or(0xFFD47800);
        let (r, g, b) = ((c & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, ((c >> 16) & 0xFF) as f32 / 255.0);
        // 透明度 ≤50% 时底色太薄，深色字在深色壁纸上看不清：不管系统主题，都改成桌面图标那样的白字加阴影
        let o = self.cfg.opacity as f32 / 100.0;
        let desk = o <= 0.5;
        let dark = dark || desk;
        // 底色和文字取值见 scripts/contrast.ps1：不透明度 100% 时，亚克力背后是纯白（深色）或纯黑（浅色），正文和次要文字仍 ≥4.5:1
        let (bg, fg) = if dark { (0x20 as f32 / 255.0, 0xF3 as f32 / 255.0) } else { (0xF3 as f32 / 255.0, 0x1A as f32 / 255.0) };
        let gdi = |v: f32| (v * 255.0) as u32;
        let (ebg, efg) = if dark { (0x2B, gdi(fg)) } else { (0xFF, gdi(fg)) };
        self.theme = Theme {
            dark,
            tint: rgba(bg, bg, bg, o * if dark { 0.78 } else { 0.88 }),
            text: rgba(fg, fg, fg, 1.0),
            muted: rgba(fg, fg, fg, if desk { 0.86 } else { 0.74 }),
            hover: rgba(fg, fg, fg, 0.10),
            press: rgba(fg, fg, fg, 0.18),
            sel_fill: rgba(r, g, b, 0.32),
            sel_line: rgba(r, g, b, 0.85),
            blur: o >= 0.5,
            shadow: desk,
            shade: rgba(0.0, 0.0, 0.0, 0.5),
            edge: (o == 0.0).then(|| rgba(1.0, 1.0, 1.0, 0.28)),
            edge_in: rgba(0.0, 0.0, 0.0, 0.18),
            peek: rgba(bg, bg, bg, 0.30),
            edit_bg: COLORREF(ebg | ebg << 8 | ebg << 16),
            edit_fg: COLORREF(efg | efg << 8 | efg << 16),
        };
        unsafe {
            if !self.edit_brush.is_invalid() {
                let _ = DeleteObject(self.edit_brush.into());
            }
            self.edit_brush = CreateSolidBrush(self.theme.edit_bg);
        }
        self.hide_ext = reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"), w!("HideFileExt")) != Some(0);
    }

    /// 设备丢失（驱动更新、显卡重置）后整套重建
    pub fn reset_gfx(&mut self) {
        let Ok(g) = gfx::Gfx::new(&self.d2d) else { return };
        self.gfx = g;
        for t in &mut self.tiles {
            t.drop_device();
            t.invalidate();
        }
    }

    pub fn save(&self) {
        if let Some(dir) = self.cfg_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.cfg_path, self.cfg.to_text());
    }

    /// 主题或透明度变了：重算配色，所有磁贴换背景并重画
    fn restyle(&mut self) {
        self.load_theme();
        for t in &mut app().tiles {
            t.apply_style(self);
            t.drop_device();
        }
    }

    fn tile_cfg(&mut self, name: &str) -> Option<&mut TileCfg> {
        self.cfg.tiles.iter_mut().find(|t| t.name == name)
    }

    /// 重扫桌面（自己的、公共的、系统图标），按归属和规则分到磁贴，再刷新所有磁贴
    pub fn rescan(&mut self) {
        let mut entries: Vec<(Entry, String)> = Vec::new();
        for dir in [Some(&self.root), self.public.as_ref()].into_iter().flatten() {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(md) = e.metadata() else { continue };
                // 公共桌面上和自己桌面同名的项，按名字认不出是哪个，只显示自己的
                if name == DATA_DIR || entries.iter().any(|(x, _)| x.name == name) {
                    continue;
                }
                let Some(tile) = rules::classify(&self.cfg, &name, md.is_dir(), is_hidden(&md)) else { continue };
                let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                entries.push((Entry { name, path: e.path(), is_dir: md.is_dir(), mtime }, tile.to_string()));
            }
        }
        if !self.test {
            for id in shell::desktop_icons() {
                let name = format!("::{id}");
                if let Some(tile) = rules::classify(&self.cfg, &name, false, false) {
                    entries.push((Entry { path: PathBuf::from(&name), name, is_dir: false, mtime: SystemTime::UNIX_EPOCH }, tile.to_string()));
                }
            }
        }
        // 规则或归属指向的磁贴还没有配置：补上
        for (_, tile) in &entries {
            if self.cfg.tiles.iter().all(|t| t.name != *tile) {
                self.cfg.tiles.push(TileCfg { name: tile.clone(), mon: String::new(), rect: [0.0; 4], rolled: false, keep: false, locked: false, order: Vec::new() });
            }
        }
        self.entries = entries;
        self.sync_tiles();
    }

    /// 文件在自己或公共的桌面目录里（换磁贴只改归属，不动文件）
    pub fn on_desk(&self, f: &Path) -> bool {
        f.parent().is_some_and(|p| same_path(p, &self.root) || self.public.as_deref().is_some_and(|q| same_path(p, q)))
    }

    /// 按配置和扫到的桌面项建窗口、刷新列表、决定显隐
    fn sync_tiles(&mut self) {
        let (mut changed, mut shown) = (false, false);
        for i in 0..self.cfg.tiles.len() {
            let name = self.cfg.tiles[i].name.clone();
            let list = self.listing(i);
            let visible = !self.hidden && (!list.is_empty() || self.cfg.tiles[i].keep);
            let idx = match self.tiles.iter().position(|t| t.name == name) {
                Some(k) => k,
                None if !visible => continue,
                None => {
                    if self.cfg.tiles[i].rect[2] == 0.0 {
                        self.place(i, list.len());
                        changed = true;
                    }
                    let c = &self.cfg.tiles[i];
                    let t = Tile::create(self, &name, c.rolled, c.locked);
                    self.tiles.push(t);
                    self.tiles.len() - 1
                }
            };
            let t = &mut self.tiles[idx];
            if t.painting {
                // 正在取图标时不换列表，稍后再刷
                unsafe { SetTimer(Some(self.main), T_SCAN, 250, None) };
            } else {
                t.set_items(list);
            }
            unsafe {
                shown |= IsWindowVisible(t.hwnd).as_bool() != visible;
                let _ = ShowWindow(t.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        // 新建了磁贴，或有磁贴显示 / 隐藏了：位置要摆，折叠标题栏压住谁也跟着变
        if shown {
            self.layout();
        }
        if changed {
            self.save();
        }
    }

    /// 第 i 块磁贴的内容：拖过的项按配置里的顺序在前，其余文件夹在前、名字按资源管理器的自然顺序跟在后面
    fn listing(&self, i: usize) -> Vec<Entry> {
        let c = &self.cfg.tiles[i];
        let mut list: Vec<Entry> = self.entries.iter().filter(|(_, t)| *t == c.name).map(|(e, _)| e.clone()).collect();
        list.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| {
                let (x, y) = (shell::wide(&a.name), shell::wide(&b.name));
                unsafe { StrCmpLogicalW(PCWSTR(x.as_ptr()), PCWSTR(y.as_ptr())) }.cmp(&0)
            })
        });
        if !c.order.is_empty() {
            let pos: HashMap<&str, usize> = c.order.iter().enumerate().map(|(k, o)| (o.as_str(), k)).collect();
            list.sort_by_key(|e| pos.get(e.name.as_str()).copied().unwrap_or(usize::MAX));
        }
        list
    }

    /// 磁贴内拖动排序：moved 挪到第 at 项前面，整个顺序存进配置
    pub fn reorder(&mut self, t: &mut Tile, moved: &[String], at: usize) {
        let Some(i) = self.cfg.tiles.iter().position(|c| c.name == t.name) else { return };
        let names: Vec<String> = t.items.iter().map(|it| it.name.clone()).collect();
        self.cfg.tiles[i].order = rules::reorder(&names, moved, at);
        self.save();
        t.set_items(self.listing(i));
    }

    /// 新磁贴从主屏工作区右上角起，按列往下堆，一列放不下换左边一列；高度按文件数取 1~3 行
    fn place(&mut self, i: usize, n: usize) {
        let mut wa = RECT::default();
        let s = unsafe {
            let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            GetDpiForSystem() as f32 / 96.0
        };
        // 340 宽一行放 3 个
        let rows = n.div_ceil(3).clamp(1, 3) as f32;
        let (w, g) = ((340.0 * s) as i32, (12.0 * s) as i32);
        let h = ((TITLE_H + 2.0 * PAD + rows * CELL_H) * s) as i32;
        let placed: Vec<[i32; 4]> = self.cfg.tiles.iter().filter(|t| t.rect[2] > 0.0).map(tile_px).collect();
        for c in 0..20 {
            let x = wa.right - (c + 1) * (w + g);
            let bottom = placed.iter().filter(|r| r[0] < x + w && x < r[0] + r[2]).map(|r| r[1] + r[3]).max();
            let y = bottom.unwrap_or(wa.top) + g;
            if y + h <= wa.bottom || c == 19 {
                (self.cfg.tiles[i].mon, self.cfg.tiles[i].rect) = locate([x, y, w, h]);
                return;
            }
        }
    }

    /// 用户拖动或改了尺寸：按磁贴现在所在的显示器记下比例
    pub fn save_rect(&mut self, t: &Tile) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(t.hwnd, &mut r);
        }
        let (mon, f) = locate(xywh(&r));
        let rolled = t.rolled;
        if let Some(c) = self.tile_cfg(&t.name) {
            // 折叠时窗口只剩标题栏，展开的高度沿用配置里的
            c.rect = [f[0], f[1], f[2], if rolled { f[1] + c.rect[3] - c.rect[1] } else { f[3] }];
            c.mon = mon;
        }
        self.save();
    }

    pub fn toggle_roll(&mut self, t: &mut Tile) {
        t.rolled = !t.rolled;
        let Some(c) = self.tile_cfg(&t.name) else { return };
        c.rolled = t.rolled;
        self.layout();
        self.save();
    }

    pub fn toggle_lock(&mut self, t: &mut Tile) {
        t.locked = !t.locked;
        if let Some(c) = self.tile_cfg(&t.name) {
            c.locked = t.locked;
        }
        t.invalidate();
        self.save();
    }

    /// target 为 Some(文件名) 时改文件名，None 时改磁贴名（连同规则和归属）
    pub fn rename(&mut self, t: &mut Tile, target: Option<String>, new: &str) {
        match target {
            Some(old) if old != new => {
                let Some(path) = t.items.iter().find(|it| it.name == old).map(|it| it.path.clone()) else { return };
                let (from, to) = (shell::wpath(&path), shell::wpath(&path.with_file_name(new)));
                match unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVE_FILE_FLAGS(0)) } {
                    Ok(()) => {
                        t.sel = vec![new.to_string()];
                        // 改了扩展名也留在这块磁贴；排过序的项改名后留在原位
                        self.cfg.map.retain(|(n, _)| *n != old);
                        self.cfg.set_map(new, &t.name);
                        if let Some(o) = self.tile_cfg(&t.name).and_then(|c| c.order.iter_mut().find(|o| **o == old)) {
                            *o = new.to_string();
                        }
                        self.save();
                    }
                    Err(e) => self.warn(t, &format!("{}{}", tr("改名失败：", "Rename failed: "), e.message())),
                }
            }
            None if t.name != new => {
                // 没内容、正隐藏着的分类（没设常驻）不算重名：名字让给这块磁贴，分类的规则也归它
                if self.tile_cfg(new).is_some_and(|c| c.keep) || self.entries.iter().any(|(_, x)| x == new) {
                    let msg = if lang::en() { format!("A tile named \"{new}\" already exists") } else { format!("已有名为“{new}”的磁贴") };
                    return self.warn(t, &msg);
                }
                self.cfg.tiles.retain(|c| c.name != new);
                if let Some(k) = self.tiles.iter().position(|x| x.name == new) {
                    destroy(&self.tiles.remove(k));
                }
                let old = std::mem::replace(&mut t.name, new.to_string());
                self.cfg.rename_tile(&old, new);
                self.entries.iter_mut().filter(|(_, x)| *x == old).for_each(|(_, x)| *x = new.to_string());
                let title = shell::wide(new);
                unsafe {
                    let _ = SetWindowTextW(t.hwnd, PCWSTR(title.as_ptr()));
                }
                t.invalidate();
                self.save();
            }
            _ => {}
        }
    }

    /// 操作失败：警告音，并在磁贴上方弹提示说明原因
    fn warn(&mut self, t: &Tile, text: &str) {
        let mut r = RECT::default();
        unsafe {
            MessageBeep(MB_ICONWARNING).ok();
            let _ = GetWindowRect(t.hwnd, &mut r);
        }
        toast::show(self, text, POINT { x: (r.left + r.right) / 2, y: r.top });
    }

    /// 新建一块空磁贴并进入改名；at 是桌面右键菜单点下的位置，None 时按默认规则摆放
    pub fn new_tile(&mut self, at: Option<POINT>) {
        let base = tr("新磁贴", "New tile");
        let mut name = base.to_string();
        for n in 2.. {
            if self.tile_cfg(&name).is_none() {
                break;
            }
            name = format!("{base} {n}");
        }
        self.cfg.tiles.push(TileCfg { name: name.clone(), mon: String::new(), rect: [0.0; 4], rolled: false, keep: true, locked: false, order: Vec::new() });
        let i = self.cfg.tiles.len() - 1;
        self.place(i, 0);
        if let Some(pt) = at {
            // 左上角放在右键的位置，超出工作区就往回挪
            let [_, _, w, h] = tile_px(&self.cfg.tiles[i]);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            unsafe {
                let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
            }
            let wa = mi.rcWork;
            (self.cfg.tiles[i].mon, self.cfg.tiles[i].rect) = locate([pt.x.min(wa.right - w).max(wa.left), pt.y.min(wa.bottom - h).max(wa.top), w, h]);
        }
        self.save();
        self.sync_tiles();
        let a = app();
        if let Some(t) = self.tiles.iter_mut().find(|t| t.name == name) {
            t.start_rename(a, None);
        }
    }

    /// 里面的项按剩下的规则分到别的磁贴
    pub fn delete_tile(&mut self, name: &str) {
        self.cfg.rules.retain(|(_, t)| t != name);
        self.cfg.map.retain(|(_, t)| t != name);
        self.cfg.tiles.retain(|t| t.name != name);
        if let Some(k) = self.tiles.iter().position(|t| t.name == name) {
            destroy(&self.tiles.remove(k));
        }
        self.save();
        self.rescan();
    }

    /// 资源管理器重启后桌面窗口换了一个，磁贴（以旧桌面为所有者）会被系统连带销毁：全部重建
    fn rebuild_tiles(&mut self) {
        self.owner = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }.ok();
        if !self.test {
            shell::show_icons(false);
        }
        for t in self.tiles.drain(..) {
            destroy(&t);
        }
        self.sync_tiles();
    }

    fn set_dblclick(&mut self, on: bool) {
        if self.dbl_thread != 0 {
            shell::stop_thread(self.dbl_thread);
            self.dbl_thread = 0;
        }
        if on {
            self.dbl_thread = shell::watch_dblclick(self.main);
        }
    }

    /// 桌面上的项只记归属；别处拖来的文件移动或复制到桌面目录，再记归属
    pub fn drop_into(&mut self, t: &mut Tile, files: Vec<PathBuf>, copy: bool) {
        if files.is_empty() {
            return;
        }
        // 全是桌面上（别的磁贴里）的同一种文件时，问要不要改规则
        let from_tiles = files.iter().all(|f| !f.is_dir() && self.on_desk(f));
        let ext = rules::ext_of(&files[0].to_string_lossy());
        let one_ext = !ext.is_empty() && files.iter().all(|f| rules::ext_of(&f.to_string_lossy()) == ext);
        let other: Vec<PathBuf> = files.iter().filter(|f| !self.on_desk(f)).cloned().collect();
        if !other.is_empty() {
            shell::drop_files(t.hwnd, &other, &self.root, copy);
        }
        for f in files.iter().filter_map(|f| f.file_name()) {
            self.cfg.set_map(&f.to_string_lossy(), &t.name);
        }
        self.save();
        self.rescan();
        if from_tiles && one_ext && self.cfg.tile_for(&ext) != Some(t.name.as_str()) {
            let msg = shell::wide(&if lang::en() {
                format!("Always put .{ext} files in \"{}\"?", t.name)
            } else {
                format!("以后 .{ext} 文件都自动放进「{}」？", t.name)
            });
            if unsafe { MessageBoxW(Some(t.hwnd), PCWSTR(msg.as_ptr()), w!("Nookery"), MB_YESNO | MB_ICONQUESTION) } == IDYES {
                self.cfg.set_rule(&ext, &t.name);
                self.save();
            }
        }
    }

    /// 拖进来的系统项：此电脑、回收站这类桌面系统图标在“桌面图标设置”里打开、由磁贴显示；
    /// 别的（控制面板里的某一项等）在桌面目录建快捷方式
    pub fn link_into(&mut self, t: &Tile, links: &[windows::Win32::UI::Shell::IShellItem]) {
        for it in links {
            let name = match shell::desktop_clsid(it) {
                Some(id) => {
                    shell::show_desktop_icon(&id, true);
                    format!("::{id}")
                }
                None => match shell::make_link(it, &self.root) {
                    Some(n) => n,
                    None => continue,
                },
            };
            self.cfg.set_map(&name, &t.name);
        }
        self.save();
        self.rescan();
    }

    fn tray(&self, msg: NOTIFY_ICON_MESSAGE) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.main,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        for (d, s) in nid.szTip.iter_mut().zip(tr("Nookery 桌面磁贴", "Nookery desktop tiles").encode_utf16()) {
            *d = s;
        }
        unsafe {
            let _ = Shell_NotifyIconW(msg, &nid);
        }
    }

    fn autostart(&self) -> bool {
        unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"), RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
    }

    fn set_autostart(&self, on: bool) {
        unsafe {
            if on {
                let mut buf = [0u16; 1024];
                let n = GetModuleFileNameW(None, &mut buf) as usize;
                let cmd = shell::wide(&format!("\"{}\"", String::from_utf16_lossy(&buf[..n])));
                let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"), REG_SZ.0, Some(cmd.as_ptr() as _), (cmd.len() * 2) as u32);
            } else {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"));
            }
        }
    }

    /// 默认磁贴名换成当前语言（配置、窗口标题一起改）；用户改过名的、或新名字已被占用的不动。
    /// 托盘提示和桌面右键菜单的文字也跟着换
    fn relang(&mut self) {
        let en = lang::en();
        for (from, to) in self.cfg.lang_renames(en) {
            if self.tile_cfg(to).is_some() {
                continue;
            }
            self.cfg.rename_tile(from, to);
            self.entries.iter_mut().filter(|(_, x)| x == from).for_each(|(_, x)| *x = to.to_string());
            if let Some(t) = self.tiles.iter_mut().find(|t| t.name == from) {
                t.name = to.into();
                let title = shell::wide(to);
                unsafe {
                    let _ = SetWindowTextW(t.hwnd, PCWSTR(title.as_ptr()));
                }
                t.invalidate();
            }
        }
        self.cfg.en = en;
        self.save();
        self.tray(NIM_MODIFY);
        if !self.test {
            register_desktop_menu();
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        self.sync_tiles();
    }

    fn tray_menu(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let check = |on: bool, s: &'static str| if on { format!("✓{s}") } else { s.to_string() };
        let (auto, hidden) = (check(self.autostart(), tr("开机自启", "Start with Windows")), check(self.hidden, tr("隐藏磁贴", "Hide tiles")));
        let dbl = check(self.cfg.dblclick, tr("双击桌面空白处隐藏磁贴", "Double-click desktop to hide tiles"));
        let items = [
            (1, tr("新建磁贴", "New tile")),
            (6, hidden.as_str()),
            (7, dbl.as_str()),
            (0, ""),
            (4, auto.as_str()),
            (0, ""),
            (5, tr("退出", "Exit")),
        ];
        let levels: Vec<String> = OPACITY_LEVELS
            .iter()
            .map(|&v| format!("{}{v}%{}", if v == self.cfg.opacity { "✓" } else { "" }, if v == 0 { tr("（只留边框）", " (border only)") } else { "" }))
            .collect();
        let sub: Vec<(u32, &str)> = levels.iter().enumerate().map(|(i, s)| (100 + i as u32, s.as_str())).collect();
        // 语言入口和选项都带英文，切到看不懂的语言后也能找回来
        let langs = [(200, if lang::en() { "中文/Chinese" } else { "✓中文/Chinese" }), (201, if lang::en() { "✓English" } else { "English" })];
        let menu = menu_from(&items);
        let (opacity, language) = (shell::wide(tr("磁贴透明度", "Tile opacity")), shell::wide(tr("语言/Language", "Language")));
        unsafe {
            // 透明度子菜单插在“双击桌面空白处隐藏磁贴”后面，语言子菜单插在“开机自启”后面
            let _ = InsertMenuW(menu, 3, MF_BYPOSITION | MF_POPUP, menu_from(&sub).0 as usize, PCWSTR(opacity.as_ptr()));
            let _ = InsertMenuW(menu, 6, MF_BYPOSITION | MF_POPUP, menu_from(&langs).0 as usize, PCWSTR(language.as_ptr()));
        }
        match track(self.main, pt, menu) {
            id @ 200..=201 => {
                lang::save(id == 201);
                self.relang();
            }
            id @ 100..=104 => {
                self.cfg.opacity = OPACITY_LEVELS[(id - 100) as usize];
                self.save();
                self.restyle();
            }
            1 => self.new_tile(None),
            4 =>self.set_autostart(!self.autostart()),
            5 => unsafe {
                self.tray(NIM_DELETE);
                let _ = DestroyWindow(self.main);
            },
            6 => self.set_hidden(!self.hidden),
            7 => {
                self.cfg.dblclick = !self.cfg.dblclick;
                self.set_dblclick(self.cfg.dblclick);
                self.save();
            }
            _ => {}
        }
    }

    /// 所有磁贴按配置里的比例摆到各自显示器的工作区上。完全按比例、不套最小尺寸，屏幕变小时磁贴不互相压住，
    /// 装不下的内容在磁贴里滚动。折叠的磁贴只剩标题栏，标题栏高度不随分辨率缩小：从上往下摆，被上方折叠标题栏压住的
    /// 磁贴上边让到标题栏下方（隔吸附间距），展开的下边不动、变矮，折叠的整条往下挪、接着让它下面的。
    /// 不存盘，换回原来的分辨率时原样复原
    pub fn layout(&self) {
        let mut v: Vec<(&Tile, [i32; 4])> = self.tiles.iter().filter_map(|t| Some((&**t, tile_px(self.cfg.tiles.iter().find(|c| c.name == t.name)?)))).collect();
        v.sort_by_key(|(_, r)| r[1]);
        for i in 0..v.len() {
            let (t, mut r) = v[i];
            let s = unsafe { GetDpiForWindow(t.hwnd) } as f32 / 96.0;
            let bar = (TITLE_H * s) as i32;
            let under = v[..i]
                .iter()
                .filter(|(u, o)| u.rolled && unsafe { IsWindowVisible(u.hwnd) }.as_bool() && o[0] < r[0] + r[2] && r[0] < o[0] + o[2] && o[1] + o[3] > r[1])
                .map(|(_, o)| o[1] + o[3] + (8.0 * s) as i32)
                .max();
            if let Some(y) = under {
                if !t.rolled {
                    r[3] = (r[3] - (y - r[1])).max(bar);
                }
                r[1] = y;
            }
            if t.rolled {
                r[3] = bar;
            }
            v[i].1 = r;
            unsafe {
                let _ = SetWindowPos(t.hwnd, None, r[0], r[1], r[2], r[3], SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if APP.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let a = app();
        match msg {
            // 桌面变化先去抖，一批变化只扫一次
            shell::WM_APP_FS => {
                SetTimer(Some(hwnd), T_SCAN, 250, None);
            }
            WM_TIMER if wp.0 == T_SCAN => {
                let _ = KillTimer(Some(hwnd), wp.0);
                a.rescan();
            }
            WM_APP_TRAY => match (lp.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP => a.set_hidden(!a.hidden),
                WM_RBUTTONUP => a.tray_menu(),
                _ => {}
            },
            WM_SETTINGCHANGE => {
                // 任务栏挪了位置、改了大小，或分辨率变了之后任务栏重新摆好，工作区跟着变
                if wp.0 == SPI_SETWORKAREA.0 as usize {
                    a.layout();
                }
                a.restyle();
            }
            WM_APP_NEWTILE => a.new_tile(Some(POINT { x: lp.0 as i32, y: (lp.0 >> 32) as i32 })),
            WM_DISPLAYCHANGE => a.layout(),
            WM_DESTROY => {
                if !a.test {
                    shell::show_icons(true);
                }
                PostQuitMessage(0);
            }
            shell::WM_APP_DBLCLK => {
                let pt = POINT { x: lp.0 as i32, y: (lp.0 >> 32) as i32 };
                // wparam 为 1 是测试驱动发的，跳过“点在桌面空白处”的判断（examples/drive.rs 的 dblclk）
                if wp.0 == 1 || shell::desktop_blank_at(pt) {
                    a.set_hidden(!a.hidden);
                    let text = if a.hidden {
                        tr("磁贴已隐藏 · 再双击桌面空白处恢复", "Tiles hidden · Double-click the desktop again to show")
                    } else {
                        tr("磁贴已显示", "Tiles shown")
                    };
                    toast::show(a, text, pt);
                }
            }
            m if m == a.taskbar_created => {
                a.tray(NIM_ADD);
                a.rebuild_tiles();
            }
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}

fn text_format(dw: &IDWriteFactory, family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> IDWriteTextFormat {
    unsafe {
        let f = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("zh-CN")).expect("创建字体失败");
        let _ = f.SetTextAlignment(align);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        f
    }
}

/// Win11 的图标字体是 Segoe Fluent Icons，Win10 只有 Segoe MDL2 Assets，两者用到的码位相同
fn icon_family(dw: &IDWriteFactory) -> PCWSTR {
    unsafe {
        let mut coll = None;
        let _ = dw.GetSystemFontCollection(&mut coll, false);
        let (mut idx, mut found) = (0u32, BOOL(0));
        if let Some(c) = coll {
            let _ = c.FindFamilyName(w!("Segoe Fluent Icons"), &mut idx, &mut found);
        }
        if found.as_bool() { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") }
    }
}

fn gdi_font(size: f32, weight: i32) -> HFONT {
    // 负数高度是字符高度（像素）：按系统 DPI 把 DIP 字号换算成像素，和 D2D 里同字号的字一样大
    unsafe {
        CreateFontW(
            -((size * GetDpiForSystem() as f32 / 96.0) as i32),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        )
    }
}

/// 桌面空白处右键菜单里加“新建磁贴”，点了以 --new-tile 启动本程序；只写当前用户的注册表
fn register_desktop_menu() {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(None, &mut buf) as usize;
        let exe = String::from_utf16_lossy(&buf[..n]);
        for (key, name, value) in [
            (MENU_KEY, PCWSTR::null(), tr("新建磁贴", "New tile").to_string()),
            (MENU_KEY, w!("Icon"), "imageres.dll,-183".to_string()),
            (MENU_CMD_KEY, PCWSTR::null(), format!("\"{exe}\" --new-tile")),
        ] {
            let v = shell::wide(&value);
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32);
        }
    }
}

/// 旧版把归进磁贴的文件移到 .Nookery\<磁贴名>\：搬回桌面，记下原来在哪块磁贴。
/// 旧版指向此电脑、回收站等的快捷方式删掉，把原图标在“桌面图标设置”里打开，改由磁贴显示。
/// 旧版配置里磁贴位置是像素坐标：按当时所在的显示器换算成比例
fn migrate(root: &Path, cfg: &mut Config) {
    let mut old = false;
    for d in std::fs::read_dir(root.join(DATA_DIR)).into_iter().flatten().flatten().filter(|d| d.path().is_dir()) {
        old = true;
        let tile = d.file_name().to_string_lossy().to_string();
        for e in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
            let path = e.path();
            let name = match (rules::ext_of(&path.to_string_lossy()) == "lnk").then(|| shell::link_clsid(&path)).flatten() {
                Some(id) => {
                    shell::show_desktop_icon(&id, true);
                    let _ = std::fs::remove_file(&path);
                    format!("::{id}")
                }
                None => match shell::move_unique(&path, root).and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())) {
                    Some(n) => n,
                    None => continue,
                },
            };
            cfg.set_map(&name, &tile);
        }
        // 用户在 .Nookery 里手动建的磁贴目录
        if cfg.tiles.iter().all(|t| t.name != tile) {
            cfg.tiles.push(TileCfg { name: tile, mon: String::new(), rect: [0.0; 4], rolled: false, keep: true, locked: false, order: Vec::new() });
        }
        let _ = std::fs::remove_dir(d.path());
    }
    // 测试模式里配置还在这个目录下，删不掉
    let _ = std::fs::remove_dir(root.join(DATA_DIR));
    if old {
        cfg.add_shortcuts();
    }
    for c in cfg.tiles.iter_mut().filter(|c| c.mon.is_empty() && c.rect[2] > 0.0) {
        (c.mon, c.rect) = locate(c.rect.map(|v| v as i32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧版 .Nookery 里的文件搬回桌面（重名加序号），归属记到原磁贴；手建的磁贴目录留成常驻磁贴
    #[test]
    fn migrate_old_layout() {
        let root = std::env::temp_dir().join("nookery-migrate-test");
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join(DATA_DIR);
        for d in ["文档", "手建"] {
            std::fs::create_dir_all(data.join(d)).unwrap();
        }
        for f in ["文档\\a.docx", "文档\\x.txt", "手建\\b.png"] {
            std::fs::write(data.join(f), "x").unwrap();
        }
        std::fs::write(root.join("x.txt"), "x").unwrap();
        let mut cfg = Config::parse("");
        cfg.rules.retain(|(k, _)| k != "lnk");
        migrate(&root, &mut cfg);
        let mut names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(names, ["a.docx", "b.png", "x (2).txt", "x.txt"]);
        for (n, t) in [("a.docx", "文档"), ("x (2).txt", "文档"), ("b.png", "手建")] {
            assert_eq!(rules::classify(&cfg, n, false, false), Some(t));
        }
        assert!(cfg.tiles.iter().any(|t| t.name == "手建" && t.keep));
        assert_eq!(rules::classify(&cfg, "微信.lnk", false, false), Some("快捷方式"));
    }
}

fn ellipsis(dw: &IDWriteFactory, f: &IDWriteTextFormat) {
    unsafe {
        let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        if let Ok(sign) = dw.CreateEllipsisTrimmingSign(f) {
            let _ = f.SetTrimming(&trim, &sign);
        }
    }
}

fn main() {
    // 没有控制台，崩溃信息写到 %TEMP%\nookery-crash.txt
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(std::env::temp_dir().join("nookery-crash.txt"), format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    let args: Vec<String> = std::env::args().collect();
    let test_root = args.iter().position(|a| a == "--root").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let root = test_root.clone().unwrap_or_else(shell::desktop_dir);
    lang::load();
    if args.iter().any(|a| a == "--uninstall") {
        // 卸载程序调用（调用前已强制结束正在运行的实例，桌面图标还关着）：显示回桌面图标，删开机自启和右键菜单；
        // 顺带把旧版留下的文件搬回桌面（读快捷方式认出系统图标要先初始化 COM）
        unsafe {
            let _ = OleInitialize(None);
            migrate(&root, &mut Config::parse(""));
            shell::show_icons(true);
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, MENU_KEY);
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"));
        }
        return;
    }
    unsafe {
        let new_tile = args.iter().any(|a| a == "--new-tile");
        let mutex = shell::wide(&format!("Nookery-{}", root.to_string_lossy().replace('\\', "/").to_lowercase()));
        let _guard = CreateMutexW(None, true, PCWSTR(mutex.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // 已在运行：把“新建磁贴”转给它；这个进程由右键菜单启动，带着前台权限，顺手让给它，好让改名框拿到焦点
            if let (true, Ok(w)) = (new_tile, FindWindowW(MAIN_CLASS, PCWSTR::null())) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = PostMessageW(Some(w), WM_APP_NEWTILE, WPARAM(0), LPARAM(((pt.y as isize) << 32) | (pt.x as u32 as isize)));
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);

        let cfg_path = match &test_root {
            Some(_) => {
                let data = root.join(DATA_DIR);
                let _ = std::fs::create_dir_all(&data);
                let d = shell::wpath(&data);
                let _ = SetFileAttributesW(PCWSTR(d.as_ptr()), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
                data.join("config.txt")
            }
            None => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Nookery").join("config.txt"),
        };
        let mut cfg = Config::parse(&std::fs::read_to_string(&cfg_path).unwrap_or_default());
        migrate(&root, &mut cfg);

        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("D2D 初始化失败");
        let gfx = gfx::Gfx::new(&d2d).expect("D3D 初始化失败");
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DirectWrite 初始化失败");
        let ui = w!("Segoe UI Variable Text");
        let fmt_title = text_format(&dw, ui, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = fmt_title.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        ellipsis(&dw, &fmt_title);
        let fmt_hint = text_format(&dw, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER);
        let fmt_icon = text_format(&dw, icon_family(&dw), 12.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER);
        let fmt_toast = text_format(&dw, ui, 14.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER);
        let fmt_label = text_format(&dw, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = fmt_label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        // 长英文单词没有断行点，普通 WRAP 会在第一行就截断，EMERGENCY_BREAK 允许在词中换行，两行都用上
        let _ = fmt_label.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK);
        ellipsis(&dw, &fmt_label);
        let (edit_font, title_font) = (gdi_font(12.0, FW_NORMAL.0 as i32), gdi_font(14.0, FW_SEMIBOLD.0 as i32));

        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(main_proc), lpszClassName: MAIN_CLASS, ..Default::default() };
        RegisterClassW(&wc);
        let wc = WNDCLASSW { style: CS_DBLCLKS, lpfnWndProc: Some(tile::tile_proc), lpszClassName: TILE_CLASS, hCursor: cursor, ..Default::default() };
        RegisterClassW(&wc);
        let wc = WNDCLASSW { lpfnWndProc: Some(toast::toast_proc), lpszClassName: toast::TOAST_CLASS, hCursor: cursor, ..Default::default() };
        RegisterClassW(&wc);
        // 普通隐藏窗口（不是 message-only），才能收到主题变化、任务栏重建等广播
        let main = CreateWindowExW(WINDOW_EX_STYLE(0), MAIN_CLASS, w!("Nookery"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, None, None).expect("创建主窗口失败");

        let mut a = Box::new(App {
            root: root.clone(),
            public: if test_root.is_some() { None } else { shell::public_desktop_dir() },
            entries: Vec::new(),
            cfg_path,
            cfg,
            main,
            // 以桌面为所有者：磁贴始终在桌面之上、其他窗口之下
            owner: FindWindowW(w!("Progman"), PCWSTR::null()).ok(),
            tiles: Vec::new(),
            d2d,
            gfx,
            dw,
            fmt_title,
            fmt_label,
            fmt_hint,
            fmt_icon,
            fmt_toast,
            edit_font,
            title_font,
            edit_brush: HBRUSH::default(),
            // 占位，马上由 load_theme 算出真正的值
            theme: Theme {
                dark: true,
                tint: rgba(0.0, 0.0, 0.0, 0.0),
                text: rgba(1.0, 1.0, 1.0, 1.0),
                muted: rgba(1.0, 1.0, 1.0, 0.5),
                hover: rgba(0.0, 0.0, 0.0, 0.0),
                press: rgba(0.0, 0.0, 0.0, 0.0),
                sel_fill: rgba(0.0, 0.0, 0.0, 0.0),
                sel_line: rgba(0.0, 0.0, 0.0, 0.0),
                blur: true,
                shadow: false,
                shade: rgba(0.0, 0.0, 0.0, 0.0),
                edge: None,
                edge_in: rgba(0.0, 0.0, 0.0, 0.0),
                peek: rgba(0.0, 0.0, 0.0, 0.0),
                edit_bg: COLORREF(0),
                edit_fg: COLORREF(0),
            },
            toast: None,
            hide_ext: true,
            hidden: false,
            topmost: args.iter().any(|a| a == "--top"),
            menu2: None,
            drop_helper: CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok(),
            dbl_thread: 0,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            force_dark: if args.iter().any(|a| a == "--light") { Some(false) } else if args.iter().any(|a| a == "--dark") { Some(true) } else { None },
            tray_icon: desktop_icon(),
            test: test_root.is_some(),
        });
        a.load_theme();
        APP = &mut *a;
        a.tray(NIM_ADD);
        if test_root.is_none() {
            shell::show_icons(false);
        }
        if a.cfg.dblclick {
            a.set_dblclick(true);
        }
        // 默认磁贴名对齐当前语言（安装包可能刚改过），顺带写桌面右键菜单
        a.relang();
        shell::watch(root, main);
        a.rescan();
        // 归属记录只在启动时清理：运行中文件可能被程序“删了再写”，一时不在
        let n = a.cfg.map.len();
        a.cfg.map.retain(|(name, _)| a.entries.iter().any(|(e, _)| e.name == *name));
        if a.cfg.map.len() != n {
            a.save();
        }
        if new_tile {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            a.new_tile(Some(pt));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
