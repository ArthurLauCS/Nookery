#![windows_subsystem = "windows"]
//! Nookery：桌面磁贴。桌面上的散文件按类型自动收进磁贴（桌面下的隐藏目录 .Nookery\<磁贴名>），快捷方式不动。
//!
//! 参数：--root <目录> 用指定目录代替桌面（测试用，配置也放进该目录）；--top 磁贴置顶（截图走查用）；
//! --new-tile 在鼠标处新建磁贴（桌面右键菜单调用，已在运行时转给正在运行的实例）；
//! --uninstall 文件全部放回桌面、删掉开机自启和右键菜单后退出（卸载程序调用）。

mod drop;
mod gfx;
mod lang;
mod rules;
mod shell;
mod tile;
mod toast;

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
use tile::{CELL_H, PAD, TITLE_H, Tile};

pub const TILE_CLASS: PCWSTR = w!("NookeryTile");
const MAIN_CLASS: PCWSTR = w!("NookeryMain");
const WM_APP_TRAY: u32 = WM_APP + 2;
/// 另一个进程（桌面右键菜单）让正在运行的实例在 lparam 指的屏幕坐标处新建磁贴
const WM_APP_NEWTILE: u32 = WM_APP + 4;
const DATA_DIR: &str = ".Nookery";
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// 桌面空白处右键菜单里的“新建磁贴”（Win11 下在“显示更多选项”里）
const MENU_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew");
const MENU_CMD_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\NookeryNew\\command");
const T_SCAN: usize = 1;
const T_RECHECK: usize = 2;
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
    pub data: PathBuf,
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

fn is_hidden(md: &std::fs::Metadata) -> bool {
    md.file_attributes() & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0
}

/// 磁贴目录里要显示的项：文件夹在前，名字按资源管理器的自然顺序
fn list_dir(dir: &Path) -> Vec<(String, bool, SystemTime)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            (!is_hidden(&md)).then(|| (e.file_name().to_string_lossy().to_string(), md.is_dir(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    v.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let (x, y) = (shell::wide(&a.0), shell::wide(&b.0));
            unsafe { StrCmpLogicalW(PCWSTR(x.as_ptr()), PCWSTR(y.as_ptr())) }.cmp(&0)
        })
    });
    v
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

    /// 把桌面上稳定下来的散文件归进磁贴，再刷新所有磁贴
    pub fn rescan(&mut self) {
        let mut wait = false;
        if !self.cfg.paused {
            if shell::label_editing() {
                wait = true;
            } else {
                let mut seen = Vec::new();
                for e in std::fs::read_dir(&self.root).into_iter().flatten().flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let Ok(md) = e.metadata() else { continue };
                    seen.push(name.clone());
                    if name == DATA_DIR || self.cfg.keep.contains(&name) {
                        continue;
                    }
                    let Some(tile) = rules::classify(&self.cfg, &name, md.is_dir(), is_hidden(&md)).map(str::to_string) else { continue };
                    // 刚从磁贴拖回桌面的（磁贴列表里还有它、目录里已经没了）：用户特意放的，留在桌面
                    if self.tiles.iter().any(|t| t.items.iter().any(|it| it.name == name) && !self.data.join(&t.name).join(&name).exists()) {
                        self.cfg.keep.push(name);
                        self.save();
                        continue;
                    }
                    let path = e.path();
                    if !shell::is_stable(&path, &md) {
                        wait = true;
                        continue;
                    }
                    if self.tile_cfg(&tile).is_none() {
                        self.cfg.tiles.push(TileCfg { name: tile.clone(), rect: [0; 4], rolled: false, keep: false, locked: false });
                    }
                    let dir = self.data.join(&tile);
                    let _ = std::fs::create_dir_all(&dir);
                    // 失败多半是文件正被占用，稍后再试
                    wait |= !shell::move_unique(&path, &dir);
                }
                // 留在桌面的项被删掉或改名后，记录跟着清掉
                let before = self.cfg.keep.len();
                self.cfg.keep.retain(|k| seen.contains(k));
                if self.cfg.keep.len() != before {
                    self.save();
                }
            }
        }
        if wait {
            unsafe { SetTimer(Some(self.main), T_RECHECK, 1000, None) };
        }
        self.sync_tiles();
    }

    /// 按配置和磁贴目录的内容建窗口、刷新列表、决定显隐
    fn sync_tiles(&mut self) {
        let mut changed = false;
        // 用户在 .Nookery 里手动建的文件夹也当成磁贴
        for e in std::fs::read_dir(&self.data).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.metadata().is_ok_and(|m| m.is_dir() && !is_hidden(&m)) && self.tile_cfg(&name).is_none() {
                self.cfg.tiles.push(TileCfg { name, rect: [0; 4], rolled: false, keep: true, locked: false });
                changed = true;
            }
        }
        for i in 0..self.cfg.tiles.len() {
            let name = self.cfg.tiles[i].name.clone();
            let list = list_dir(&self.data.join(&name));
            let visible = !self.hidden && (!list.is_empty() || self.cfg.tiles[i].keep);
            let idx = match self.tiles.iter().position(|t| t.name == name) {
                Some(k) => k,
                None if !visible => continue,
                None => {
                    if self.cfg.tiles[i].rect[2] == 0 {
                        self.place(i, list.len());
                        changed = true;
                    }
                    let c = &self.cfg.tiles[i];
                    let t = Tile::create(self, &name, c.rect, c.rolled, c.locked);
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
                let _ = ShowWindow(t.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        if changed {
            self.save();
        }
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
        for c in 0..20 {
            let x = wa.right - (c + 1) * (w + g);
            let bottom = self.cfg.tiles.iter().filter(|t| t.rect[2] > 0 && t.rect[0] < x + w && x < t.rect[0] + t.rect[2]).map(|t| t.rect[1] + t.rect[3]).max();
            let y = bottom.unwrap_or(wa.top) + g;
            if y + h <= wa.bottom || c == 19 {
                self.cfg.tiles[i].rect = [x, y, w, h];
                return;
            }
        }
    }
    pub fn save_rect(&mut self, t: &Tile) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(t.hwnd, &mut r);
        }
        let rolled = t.rolled;
        if let Some(c) = self.tile_cfg(&t.name) {
            c.rect = [r.left, r.top, r.right - r.left, if rolled { c.rect[3] } else { r.bottom - r.top }];
        }
        self.save();
    }

    pub fn toggle_roll(&mut self, t: &mut Tile) {
        t.rolled = !t.rolled;
        let Some(c) = self.tile_cfg(&t.name) else { return };
        c.rolled = t.rolled;
        let s = unsafe { GetDpiForWindow(t.hwnd) } as f32 / 96.0;
        let h = if t.rolled { (TITLE_H * s) as i32 } else { c.rect[3] };
        let w = c.rect[2];
        unsafe {
            let _ = SetWindowPos(t.hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
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

    /// target 为 Some(文件名) 时改文件名，None 时改磁贴名（连同目录和规则）
    pub fn rename(&mut self, t: &mut Tile, target: Option<String>, new: &str) {
        let dir = t.dir(self);
        match target {
            Some(old) if old != new => {
                let (from, to) = (shell::wpath(&dir.join(&old)), shell::wpath(&dir.join(new)));
                match unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVE_FILE_FLAGS(0)) } {
                    Ok(()) => t.sel = vec![new.to_string()],
                    Err(e) => self.warn(t, &format!("{}{}", tr("改名失败：", "Rename failed: "), e.message())),
                }
            }
            None if t.name != new => {
                let to = self.data.join(new);
                // 没文件、正隐藏着的分类（目录不存在、没设常驻）不算重名：名字让给这块磁贴，分类的规则也归它
                if to.exists() && !same_path(&to, &dir) || self.tile_cfg(new).is_some_and(|c| c.keep) {
                    let msg = if lang::en() { format!("A tile named \"{new}\" already exists") } else { format!("已有名为“{new}”的磁贴") };
                    return self.warn(t, &msg);
                }
                let moved = if dir.exists() {
                    let (f, n) = (shell::wpath(&dir), shell::wpath(&to));
                    unsafe { MoveFileExW(PCWSTR(f.as_ptr()), PCWSTR(n.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(|e| e.message())
                } else {
                    std::fs::create_dir_all(&to).map_err(|e| e.to_string())
                };
                if let Err(e) = moved {
                    return self.warn(t, &format!("{}{e}", tr("改名失败：", "Rename failed: ")));
                }
                self.cfg.tiles.retain(|c| c.name != new);
                if let Some(k) = self.tiles.iter().position(|x| x.name == new) {
                    destroy(&self.tiles.remove(k));
                }
                let old = std::mem::replace(&mut t.name, new.to_string());
                self.cfg.rename_tile(&old, new);
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
            if self.tile_cfg(&name).is_none() && !self.data.join(&name).exists() {
                break;
            }
            name = format!("{base} {n}");
        }
        let _ = std::fs::create_dir_all(self.data.join(&name));
        self.cfg.tiles.push(TileCfg { name: name.clone(), rect: [0; 4], rolled: false, keep: true, locked: false });
        let i = self.cfg.tiles.len() - 1;
        self.place(i, 0);
        if let Some(pt) = at {
            // 左上角放在右键的位置，超出工作区就往回挪
            let [_, _, w, h] = self.cfg.tiles[i].rect;
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            unsafe {
                let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
            }
            let wa = mi.rcWork;
            self.cfg.tiles[i].rect = [pt.x.min(wa.right - w).max(wa.left), pt.y.min(wa.bottom - h).max(wa.top), w, h];
        }
        self.save();
        self.sync_tiles();
        let a = app();
        if let Some(t) = self.tiles.iter_mut().find(|t| t.name == name) {
            t.start_rename(a, None);
        }
    }

    /// 文件放回桌面根目录，之后按剩下的规则重新归类
    pub fn delete_tile(&mut self, name: &str) {
        self.cfg.rules.retain(|(_, t)| t != name);
        self.cfg.tiles.retain(|t| t.name != name);
        let dir = self.data.join(name);
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            shell::move_unique(&e.path(), &self.root);
        }
        let _ = std::fs::remove_dir(&dir);
        if let Some(k) = self.tiles.iter().position(|t| t.name == name) {
            destroy(&self.tiles.remove(k));
        }
        self.save();
    }

    /// 资源管理器重启后桌面窗口换了一个，磁贴（以旧桌面为所有者）会被系统连带销毁：全部重建
    fn rebuild_tiles(&mut self) {
        self.owner = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }.ok();
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

    pub fn drop_into(&mut self, t: &mut Tile, mut files: Vec<PathBuf>, copy: bool) {
        let dir = t.dir(self);
        files.retain(|f| !f.parent().is_some_and(|p| same_path(p, &dir)));
        if files.is_empty() {
            return;
        }
        let _ = std::fs::create_dir_all(&dir);
        // 全是从别的磁贴拖来的同一种文件时，问要不要改规则
        let from_tiles = files.iter().all(|f| !f.is_dir() && f.parent().and_then(Path::parent).is_some_and(|p| same_path(p, &self.data)));
        let ext = rules::ext_of(&files[0].to_string_lossy());
        let one_ext = !ext.is_empty() && files.iter().all(|f| rules::ext_of(&f.to_string_lossy()) == ext);
        shell::drop_files(t.hwnd, &files, &dir, copy);
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

    /// 卸载前用：所有文件放回桌面，并暂停自动归类
    fn restore_all(&mut self) {
        self.cfg.paused = true;
        restore(&self.root);
        self.save();
        self.sync_tiles();
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

    /// 默认磁贴名换成当前语言（目录、配置、窗口标题一起改）；用户改过名的、或新名字已被占用的不动。
    /// 托盘提示和桌面右键菜单的文字也跟着换
    fn relang(&mut self) {
        let en = lang::en();
        for (from, to) in self.cfg.lang_renames(en) {
            let (src, dst) = (self.data.join(from), self.data.join(to));
            if self.tile_cfg(to).is_some() || dst.exists() || src.exists() && std::fs::rename(&src, &dst).is_err() {
                continue;
            }
            self.cfg.rename_tile(from, to);
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
        let (paused, auto, hidden) = (
            check(self.cfg.paused, tr("暂停自动归类", "Pause auto-sorting")),
            check(self.autostart(), tr("开机自启", "Start with Windows")),
            check(self.hidden, tr("隐藏磁贴", "Hide tiles")),
        );
        let dbl = check(self.cfg.dblclick, tr("双击桌面空白处隐藏磁贴", "Double-click desktop to hide tiles"));
        let items = [
            (1, tr("新建磁贴", "New tile")),
            (6, hidden.as_str()),
            (7, dbl.as_str()),
            (2, paused.as_str()),
            (0, ""),
            (3, tr("全部还原到桌面", "Restore all to desktop")),
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
            // 透明度子菜单插在“暂停自动归类”后面，语言子菜单插在“开机自启”后面
            let _ = InsertMenuW(menu, 4, MF_BYPOSITION | MF_POPUP, menu_from(&sub).0 as usize, PCWSTR(opacity.as_ptr()));
            let _ = InsertMenuW(menu, 8, MF_BYPOSITION | MF_POPUP, menu_from(&langs).0 as usize, PCWSTR(language.as_ptr()));
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
            2 => {
                self.cfg.paused = !self.cfg.paused;
                self.save();
                self.rescan();
            }
            3 => {
                let ok = unsafe {
                    let msg = shell::wide(tr("把所有磁贴里的文件放回桌面，并暂停自动归类？", "Move all files in tiles back to the desktop and pause auto-sorting?"));
                    MessageBoxW(Some(self.main), PCWSTR(msg.as_ptr()), w!("Nookery"), MB_YESNO | MB_ICONQUESTION)
                } == IDYES;
                if ok {
                    self.restore_all();
                }
            }
            4 => self.set_autostart(!self.autostart()),
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

    /// 分辨率或显示器变化后，把跑到屏幕外的磁贴拉回最近的工作区
    fn clamp_tiles(&mut self) {
        for t in &self.tiles {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(t.hwnd, &mut r);
                let (x, y) = fit_work_area(&r);
                if (x, y) != (r.left, r.top) {
                    let _ = SetWindowPos(t.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
        }
        let tiles: Vec<*const Tile> = self.tiles.iter().map(|t| &**t as *const Tile).collect();
        for t in tiles {
            self.save_rect(unsafe { &*t });
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
            WM_TIMER if wp.0 == T_SCAN || wp.0 == T_RECHECK => {
                let _ = KillTimer(Some(hwnd), wp.0);
                a.rescan();
            }
            WM_APP_TRAY => match (lp.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP => a.set_hidden(!a.hidden),
                WM_RBUTTONUP => a.tray_menu(),
                _ => {}
            },
            WM_SETTINGCHANGE => a.restyle(),
            WM_APP_NEWTILE => a.new_tile(Some(POINT { x: lp.0 as i32, y: (lp.0 >> 32) as i32 })),
            WM_DISPLAYCHANGE => a.clamp_tiles(),
            WM_DESTROY => PostQuitMessage(0),
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

/// .Nookery 下每个磁贴目录里的东西都放回桌面，并去掉桌面右键菜单
fn restore(root: &Path) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, MENU_KEY);
    }
    for d in std::fs::read_dir(root.join(DATA_DIR)).into_iter().flatten().flatten() {
        for e in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
            shell::move_unique(&e.path(), root);
        }
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
        // 卸载程序调用（调用前已结束正在运行的实例）：文件放回桌面，删开机自启和右键菜单，空的磁贴目录一并删掉
        restore(&root);
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Nookery"));
        }
        let data = root.join(DATA_DIR);
        for d in std::fs::read_dir(&data).into_iter().flatten().flatten() {
            let _ = std::fs::remove_dir(d.path());
        }
        let _ = std::fs::remove_dir(&data);
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

        let data = root.join(DATA_DIR);
        let _ = std::fs::create_dir_all(&data);
        let d = shell::wpath(&data);
        let _ = SetFileAttributesW(PCWSTR(d.as_ptr()), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
        let cfg_path = match &test_root {
            Some(_) => data.join("config.txt"),
            None => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Nookery").join("config.txt"),
        };
        let cfg = Config::parse(&std::fs::read_to_string(&cfg_path).unwrap_or_default());

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
            data,
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
        // 第一次在真实桌面上运行：先说清楚要动多少项，确认后再整理
        if test_root.is_none() && !a.cfg_path.exists() {
            let n = std::fs::read_dir(&a.root)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name() != DATA_DIR && e.metadata().is_ok_and(|m| rules::classify(&a.cfg, &e.file_name().to_string_lossy(), m.is_dir(), is_hidden(&m)).is_some()))
                .count();
            let msg = shell::wide(&if lang::en() {
                format!(
                    "Nookery will sort {n} loose files and folders on your desktop into tiles by type. Shortcuts stay put.\nYou can always choose \"Restore all to desktop\" in the tray menu.\n\nSort now? Choose \"No\" to pause auto-sorting for now."
                )
            } else {
                format!(
                    "Nookery 会把桌面上 {n} 个散文件和文件夹按类型收进磁贴，快捷方式不动。\n随时可以在托盘菜单里“全部还原到桌面”。\n\n现在整理吗？选“否”会先暂停自动归类。"
                )
            });
            if n > 0 && MessageBoxW(Some(main), PCWSTR(msg.as_ptr()), w!("Nookery"), MB_YESNO | MB_ICONQUESTION) != IDYES {
                a.cfg.paused = true;
            }
            a.save();
        }
        if a.cfg.dblclick {
            a.set_dblclick(true);
        }
        // 默认磁贴名对齐当前语言（安装包可能刚改过），顺带写桌面右键菜单
        a.relang();
        shell::watch(root, main);
        a.rescan();
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
