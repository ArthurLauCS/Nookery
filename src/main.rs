#![windows_subsystem = "windows"]
//! Nookery：桌面磁贴。桌面上的东西按类型显示在磁贴里，文件留在桌面目录不动；
//! 运行时关掉资源管理器自己的桌面图标，退出时打开。
//!
//! 参数：--root <目录> 用指定目录代替桌面（测试用，配置放进该目录下的 .Nookery，不碰真实桌面图标）；--top 磁贴置顶（截图走查用）；
//! --new-tile 在鼠标处新建磁贴（桌面右键菜单调用，已在运行时转给正在运行的实例）；
//! --uninstall 显示回桌面图标、删掉开机自启和右键菜单后退出（卸载程序调用）。

mod drop;
mod fonts;
mod gfx;
mod lang;
mod rules;
mod shell;
mod tile;
mod theme;
mod toast;

use std::collections::HashMap;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Registry::*;
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use lang::tr;
use theme::Theme;
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
/// 开机自启的任务计划名
const TASK: &str = "Nookery";
/// 桌面空白处右键菜单里的“新建磁贴”（Win11 下在“显示更多选项”里）
const MENU_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew");
const MENU_CMD_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew\\command");
const T_SCAN: usize = 1;
const OPACITY_LEVELS: [u8; 5] = [100, 75, 50, 25, 0];

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
    pub d2d: ID2D1Factory1,
    pub gfx: gfx::Gfx,
    pub dw: IDWriteFactory,
    pub fmt_title: IDWriteTextFormat,
    pub fmt_label: IDWriteTextFormat,
    pub fmt_hint: IDWriteTextFormat,
    pub fmt_toast: IDWriteTextFormat,
    /// 改名框字体（GDI）：文件名用雅黑 UI，磁贴名用和 D2D 里同一个字体（朱雀仿宋；装不上退回雅黑 UI）
    pub edit_font: HFONT,
    pub title_font: HFONT,
    /// 磁贴名这一行字的上沿（基线减上伸）：磁贴名改名框的框顶，单行编辑框的字贴着框顶画，和标题重合
    pub title_top: f32,
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
    /// 后台取图标线程
    pub icons: shell::Icons,
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

/// 系统画的菜单（磁贴菜单、托盘菜单、文件右键菜单）跟磁贴的深浅色走。用 uxtheme 没公开的序号导出（Win10 1903 起）：
/// 135 SetPreferredAppMode（2 强制深色，3 强制浅色），136 FlushMenuThemes 让菜单重读主题
fn menu_theme(dark: bool) {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type SetMode = unsafe extern "system" fn(i32) -> i32;
    type Flush = unsafe extern "system" fn();
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let (Some(mode), Some(flush)) = (GetProcAddress(ux, PCSTR(135 as _)), GetProcAddress(ux, PCSTR(136 as _))) {
            std::mem::transmute::<_, SetMode>(mode)(if dark { 2 } else { 3 });
            std::mem::transmute::<_, Flush>(flush)();
        }
    }
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
        // 没有这个值时是系统默认的浅色
        let system_dark = self.force_dark.unwrap_or_else(|| reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme")) == Some(0));
        // 配色全在 theme.rs；透明度 ≤50% 时文字压在壁纸上，改用夜色那套加阴影（见 Theme::new）
        self.theme = Theme::new(system_dark, self.cfg.opacity);
        menu_theme(self.theme.dark);
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
            t.set_items(list);
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
        let (auto, hidden) = (check(autostart(), tr("开机自启", "Start with Windows")), check(self.hidden, tr("隐藏磁贴", "Hide tiles")));
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
            4 => set_autostart(!autostart()),
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
                // 开机时系统和各程序连着发很多次这个广播：深浅色真变了才换配色（换配色要重画、重取全部图标）
                let dark = a.theme.dark;
                a.load_theme();
                if a.theme.dark != dark {
                    a.restyle();
                }
            }
            shell::WM_APP_ICON => {
                let job = Box::from_raw(lp.0 as *mut shell::IconJob);
                if let Some(t) = app().tiles.iter_mut().find(|t| t.hwnd.0 as usize == job.tile) {
                    t.set_icon(a, *job);
                }
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

fn text_format(dw: &IDWriteFactory, family: PCWSTR, coll: Option<&IDWriteFontCollection>, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> IDWriteTextFormat {
    unsafe {
        let f = dw.CreateTextFormat(family, coll, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("zh-CN")).expect("创建字体失败");
        let _ = f.SetTextAlignment(align);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        f
    }
}

/// 字体的度量；字体不在 coll（或系统）里时取 Segoe UI 顶替
fn font_metrics(dw: &IDWriteFactory, family: PCWSTR, coll: Option<&IDWriteFontCollection>, weight: DWRITE_FONT_WEIGHT) -> DWRITE_FONT_METRICS {
    unsafe {
        let coll = match coll {
            Some(c) => c.clone(),
            None => {
                let mut c = None;
                let _ = dw.GetSystemFontCollection(&mut c, false);
                c.expect("读取系统字体失败")
            }
        };
        let find = |f: PCWSTR| {
            let (mut idx, mut found) = (0u32, BOOL(0));
            let _ = coll.FindFamilyName(f, &mut idx, &mut found);
            found.as_bool().then_some(idx)
        };
        let idx = find(family).or_else(|| find(w!("Segoe UI"))).or(Some(0)).expect("找不到界面字体");
        let font = coll.GetFontFamily(idx).and_then(|f| f.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)).expect("读取字体失败");
        let mut m = DWRITE_FONT_METRICS::default();
        font.GetMetrics(&mut m);
        m
    }
}

fn gdi_font(face: PCWSTR, size: f32, weight: i32) -> HFONT {
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
            face,
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

/// 开机自启是任务计划里当前用户“登录时”启动本程序的任务：注册表 Run 项由资源管理器在开机后逐个串行拉起，
/// 排在后面要等一分多钟（2026-10-29 本机实测 82 秒），任务计划登录即启动
fn task_folder() -> Option<ITaskFolder> {
    unsafe {
        let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).ok()?;
        let none = VARIANT::default();
        svc.Connect(&none, &none, &none, &none).ok()?;
        svc.GetFolder(&BSTR::from("\\")).ok()
    }
}

fn autostart() -> bool {
    task_folder().is_some_and(|f| unsafe { f.GetTask(&BSTR::from(TASK)) }.is_ok())
}

/// 旧版的开机自启写在 Run 项里，开关时一并删掉
fn set_autostart(on: bool) {
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"));
        let Some(f) = task_folder() else { return };
        if !on {
            let _ = f.DeleteTask(&BSTR::from(TASK), 0);
            return;
        }
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(None, &mut buf) as usize;
        let esc = |s: String| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let exe = esc(String::from_utf16_lossy(&buf[..n]));
        let user = esc(format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default()));
        // 触发限定为当前用户，普通用户才有权限建；默认设置会在任务跑满 3 天时结束程序、用电池时不启动、
        // 以低于正常的优先级运行，都改掉
        let xml = format!(
            "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\
             <Triggers><LogonTrigger><UserId>{user}</UserId></LogonTrigger></Triggers>\
             <Principals><Principal><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType></Principal></Principals>\
             <Settings><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\
             <ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>4</Priority></Settings>\
             <Actions><Exec><Command>{exe}</Command></Exec></Actions></Task>"
        );
        let none = VARIANT::default();
        let _ = f.RegisterTask(&BSTR::from(TASK), &BSTR::from(xml), TASK_CREATE_OR_UPDATE.0, &none, &none, TASK_LOGON_INTERACTIVE_TOKEN, &none);
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
        }
        set_autostart(false);
        return;
    }
    if args.iter().any(|a| a == "--autostart") {
        // 安装程序勾了“开机自启”时调用
        unsafe {
            let _ = OleInitialize(None);
        }
        set_autostart(true);
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
        // 开机自启在登录时就拉起本程序，资源管理器可能还没建好桌面：等它建好，才挂得上磁贴、关得掉桌面图标
        while !shell::desktop_ready() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);
        // 旧版的开机自启写在 Run 项里：换成任务计划
        if test_root.is_none() && RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"), RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS {
            set_autostart(true);
        }

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
        let ui_name = shell::wide(theme::FONT_UI);
        let ui = PCWSTR(ui_name.as_ptr());
        let (w_title, w_body) = (DWRITE_FONT_WEIGHT(theme::WEIGHT_TITLE), DWRITE_FONT_WEIGHT(theme::WEIGHT_BODY));
        // 磁贴名字体：随程序带的朱雀仿宋；文件缺失时退回界面字体，不报错
        let display = fonts::load_display(&dw);
        let disp_name = shell::wide(display.as_ref().map_or(theme::FONT_UI, |d| d.family.as_str()));
        let disp = PCWSTR(disp_name.as_ptr());
        let disp_coll = display.as_ref().map(|d| &d.collection);
        let fmt_title = text_format(&dw, disp, disp_coll, theme::SIZE_TITLE, w_title, DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = fmt_title.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        if display.is_some() {
            // 用户打的字朱雀仿宋里没有时，先回退微软雅黑 UI，再走系统回退链
            if let (Ok(f1), Some(fb)) = (fmt_title.cast::<IDWriteTextFormat1>(), fonts::fallback(&dw, ui)) {
                let _ = f1.SetFontFallback(&fb);
            }
        }
        // 按行框居中时行框下部是降部，字形偏下：行高取标题栏高，基线放到让大写字母居中的位置，和按钮图标同一条中线
        let m = font_metrics(&dw, disp, disp_coll, w_title);
        let em = |v: u16| v as f32 * fmt_title.GetFontSize() / m.designUnitsPerEm as f32;
        let baseline = (TITLE_H + em(m.capHeight)) / 2.0;
        let _ = fmt_title.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, TITLE_H, baseline);
        ellipsis(&dw, &fmt_title);
        let fmt_hint = text_format(&dw, ui, None, theme::SIZE_LABEL, w_body, DWRITE_TEXT_ALIGNMENT_CENTER);
        let fmt_toast = text_format(&dw, ui, None, theme::SIZE_TOAST, w_body, DWRITE_TEXT_ALIGNMENT_CENTER);
        let fmt_label = text_format(&dw, ui, None, theme::SIZE_LABEL, w_body, DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = fmt_label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        // 长英文单词没有断行点，普通 WRAP 会在第一行就截断，EMERGENCY_BREAK 允许在词中换行，两行都用上
        let _ = fmt_label.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK);
        ellipsis(&dw, &fmt_label);
        // 磁贴名改名框：同一字体的 GDI 版；装不上或 GDI 没认出就用雅黑 UI，不报错
        let edit_font = gdi_font(ui, theme::SIZE_LABEL, FW_NORMAL.0 as i32);
        let title_font = if display.is_some() && fonts::register_gdi(fonts::DISPLAY_FILE) {
            let face = shell::wide(fonts::DISPLAY_GDI_FACE);
            let f = gdi_font(PCWSTR(face.as_ptr()), theme::SIZE_TITLE, FW_NORMAL.0 as i32);
            if fonts::gdi_face_is(f, fonts::DISPLAY_GDI_FACE) { f } else { gdi_font(ui, theme::SIZE_TITLE, FW_NORMAL.0 as i32) }
        } else {
            gdi_font(ui, theme::SIZE_TITLE, FW_NORMAL.0 as i32)
        };
        // 单行编辑框的字按 GDI 的上伸贴着框顶画：框顶 = 基线减 GDI 上伸，字才落在标题的基线上
        let title_top = {
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, title_font.into());
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            let _ = DeleteDC(dc);
            baseline - tm.tmAscent as f32 * 96.0 / GetDpiForSystem() as f32
        };

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
            fmt_toast,
            edit_font,
            title_font,
            title_top,
            edit_brush: HBRUSH::default(),
            // 占位，马上由 load_theme 算出真正的值
            theme: Theme::new(true, 100),
            toast: None,
            hide_ext: true,
            hidden: false,
            topmost: args.iter().any(|a| a == "--top"),
            menu2: None,
            drop_helper: CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok(),
            icons: shell::Icons::start(main),
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
