//! 和资源管理器打交道：图标、右键菜单、拖放、文件移动、桌面监听。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicU32};
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

pub const WM_APP_FS: u32 = WM_APP + 1;
pub const WM_APP_DBLCLK: u32 = WM_APP + 3;
pub const WM_APP_ICON: u32 = WM_APP + 5;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn wpath(p: &Path) -> Vec<u16> {
    wide(&p.to_string_lossy())
}

pub fn desktop_dir() -> PathBuf {
    known_dir(&FOLDERID_Desktop).expect("找不到桌面目录")
}

/// 所有用户共用的桌面（C:\Users\Public\Desktop），安装程序常把快捷方式放这里
pub fn public_desktop_dir() -> Option<PathBuf> {
    known_dir(&FOLDERID_PublicDesktop)
}

fn known_dir(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        Some(PathBuf::from(s))
    }
}

/// 桌面图标所在的列表控件
fn desktop_listview() -> Option<HWND> {
    unsafe {
        let def = FindWindowW(w!("Progman"), PCWSTR::null())
            .ok()
            .and_then(|p| FindWindowExW(Some(p), None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok())
            .or_else(|| {
                // 开了壁纸轮换时 DefView 挂在某个 WorkerW 下
                let mut w = None;
                loop {
                    w = Some(FindWindowExW(None, w, w!("WorkerW"), PCWSTR::null()).ok()?);
                    if let Ok(d) = FindWindowExW(w, None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
                        return Some(d);
                    }
                }
            });
        FindWindowExW(def, None, w!("SysListView32"), PCWSTR::null()).ok()
    }
}

/// “显示桌面图标”的设置是关的：资源管理器启动时按它建桌面，切换时写回
fn icons_off() -> bool {
    crate::reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"), w!("HideIcons")) == Some(1)
}

/// 资源管理器的桌面建好了：图标列表在，并且已按设置显示出来（设置是关的就不会显示）
pub fn desktop_ready() -> bool {
    desktop_listview().is_some_and(|lv| unsafe { IsWindowVisible(lv) }.as_bool() || icons_off())
}

/// 资源管理器的桌面图标整体显示 / 隐藏，和桌面右键“查看 → 显示桌面图标”是同一个开关。
/// 按设置判断要不要切换，不看列表窗口当下显没显示：登录时资源管理器正在按设置建桌面，窗口显隐还会变
pub fn show_icons(show: bool) {
    // 桌面窗口里“显示桌面图标”菜单项的命令号，发一次切换一次
    const TOGGLE_ICONS: usize = 0x7402;
    let Some(lv) = desktop_listview() else { return };
    unsafe {
        if icons_off() == show {
            if let Ok(def) = GetParent(lv) {
                let _ = PostMessageW(Some(def), WM_COMMAND, WPARAM(TOGGLE_ICONS), LPARAM(0));
            }
        }
    }
}

/// 这一点是不是桌面空白处：桌面图标隐藏时点到的是图标列表背后的桌面窗口；
/// 图标显示时要点在列表上，且没有选中任何图标（点空白会清空选中）
pub fn desktop_blank_at(pt: POINT) -> bool {
    const LVM_GETSELECTEDCOUNT: u32 = 0x1032;
    let Some(lv) = desktop_listview() else { return false };
    let mut n = 1usize;
    unsafe {
        let w = WindowFromPoint(pt);
        if !IsWindowVisible(lv).as_bool() {
            return GetParent(lv).is_ok_and(|def| w == def);
        }
        w == lv && SendMessageTimeoutW(lv, LVM_GETSELECTEDCOUNT, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 200, Some(&mut n)).0 != 0 && n == 0
    }
}

/// 移进 dir，重名时依次改成“名字 (2)”“名字 (3)”…；不覆盖已有文件。返回移到的位置
pub fn move_unique(src: &Path, dir: &Path) -> Option<PathBuf> {
    let name = src.file_name().unwrap_or_default().to_string_lossy().to_string();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !src.is_dir() => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let from = wpath(src);
    for n in 1..100 {
        let dst = if n == 1 { dir.join(&name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        let to = wpath(&dst);
        match unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVE_FILE_FLAGS(0)) } {
            Ok(()) => return Some(dst),
            Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() || e.code() == ERROR_FILE_EXISTS.to_hresult() => {}
            Err(_) => return None,
        }
    }
    None
}

/// 拖放数据或剪贴板里的文件路径
pub fn hdrop_files(d: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut m) = d.GetData(&fmt) else { return Vec::new() };
        let files = hdrop_paths(HDROP(m.u.hGlobal.0));
        ReleaseStgMedium(&mut m);
        files
    }
}

/// 剪贴板里复制 / 剪切的文件的名字
pub fn clipboard_names() -> Vec<String> {
    let files = unsafe { OleGetClipboard() }.map(|d| hdrop_files(&d)).unwrap_or_default();
    files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()).collect()
}

pub fn hdrop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        (0..DragQueryFileW(hdrop, u32::MAX, None))
            .map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(hdrop, i, None) as usize + 1];
                let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
                PathBuf::from(String::from_utf16_lossy(&buf[..n]))
            })
            .collect()
    }
}

/// 拖进来的文件移动或复制进 dir；重名时由系统弹窗询问
pub fn drop_files(hwnd: HWND, files: &[PathBuf], dir: &Path, copy: bool) {
    let mut from: Vec<u16> = files.iter().flat_map(|f| wpath(f)).collect();
    from.push(0);
    let to = wpath(dir);
    let mut op = SHFILEOPSTRUCTW {
        hwnd,
        wFunc: if copy { FO_COPY } else { FO_MOVE },
        pFrom: PCWSTR(from.as_ptr()),
        pTo: PCWSTR(to.as_ptr()),
        fFlags: FOF_ALLOWUNDO.0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) };
}

/// 打开文件、文件夹或系统图标（::{CLSID}）
pub fn open(path: &Path) {
    let p = wpath(path);
    unsafe {
        let mut pidl = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(p.as_ptr()), None, &mut pidl, 0, None).is_err() {
            return;
        }
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST,
            lpIDList: pidl as _,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let _ = ShellExecuteExW(&mut info);
        CoTaskMemFree(Some(pidl as _));
    }
}

/// 资源管理器里显示的名字（系统图标用）
pub fn display_name(path: &Path) -> Option<String> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wpath(path).as_ptr()), None).ok()?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// 取图标的请求，取到的像素填进 pixels 发回；磁贴窗口和（名字，修改时间）用来认回是哪一项
pub struct IconJob {
    pub tile: usize,
    pub name: String,
    pub mtime: SystemTime,
    pub path: PathBuf,
    pub px: i32,
    pub pixels: Option<(Vec<u8>, u32, u32)>,
}

/// 后台取图标的线程。Shell 取一个图标要几到几十毫秒，开机冷缓存时慢得多，还会抽消息，放在界面线程上磁贴就无响应
/// （磁贴以桌面为所有者，两边输入队列连着，桌面也跟着卡）。线程用 STA（实测比 MTA 快约 40%），所以自己抽消息；
/// 请求走通道，线程消息只用来叫醒：被 Shell 的模态循环吃掉时，取完手上这个照样接着取
pub struct Icons {
    tx: std::sync::mpsc::Sender<IconJob>,
    tid: u32,
}

impl Icons {
    /// 取到的图标以 WM_APP_ICON 发给 notify，lparam 是 Box<IconJob>
    pub fn start(notify: HWND) -> Icons {
        let notify = notify.0 as usize;
        let (tx, rx) = std::sync::mpsc::channel::<IconJob>();
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut msg = MSG::default();
            // 先建好消息队列再报线程 id，叫醒消息才不会丢
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let _ = tid_tx.send(windows::Win32::System::Threading::GetCurrentThreadId());
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
                while let Ok(mut job) = rx.try_recv() {
                    job.pixels = icon_pixels(&job.path, job.px);
                    let p = Box::into_raw(Box::new(job));
                    if PostMessageW(Some(HWND(notify as _)), WM_APP_ICON, WPARAM(0), LPARAM(p as isize)).is_err() {
                        drop(Box::from_raw(p));
                    }
                }
            }
        });
        Icons { tx, tid: tid_rx.recv().unwrap_or(0) }
    }

    pub fn load(&self, job: IconJob) {
        let _ = self.tx.send(job);
        unsafe {
            let _ = PostThreadMessageW(self.tid, WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}

/// 系统图标或缩略图的像素：BGRA、预乘 alpha、自上而下
fn icon_pixels(path: &Path, px: i32) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wpath(path).as_ptr()), None).ok()?;
        let hbm = f.GetImage(SIZE { cx: px, cy: px }, SIIGBF_RESIZETOFIT).ok()?;
        let mut ds = DIBSECTION::default();
        let got = GetObjectW(hbm.into(), size_of::<DIBSECTION>() as i32, Some(&mut ds as *mut _ as _));
        let bm = ds.dsBm;
        let result = if got == 0 || bm.bmBitsPixel != 32 || bm.bmBits.is_null() {
            None
        } else {
            let (w, h, stride) = (bm.bmWidth as usize, bm.bmHeight as usize, bm.bmWidthBytes as usize);
            let src = std::slice::from_raw_parts(bm.bmBits as *const u8, stride * h);
            let mut buf = Vec::with_capacity(w * 4 * h);
            for y in 0..h {
                // biHeight > 0 是自下而上存储
                let row = if ds.dsBmih.biHeight > 0 { h - 1 - y } else { y };
                buf.extend_from_slice(&src[row * stride..row * stride + w * 4]);
            }
            // 缩略图不带 alpha 时整张 alpha 为 0，补成不透明
            if buf.chunks_exact(4).all(|c| c[3] == 0) {
                buf.chunks_exact_mut(4).for_each(|c| c[3] = 255);
            } else if buf.chunks_exact(4).any(|c| c[0].max(c[1]).max(c[2]) > c[3]) {
                // 图标是没预乘的 alpha（2026-10-18 实测 notepad.exe 48px 有 242 个像素颜色值大于 alpha），
                // 当成预乘画，半透明的边缘会发亮、发毛：先乘上 alpha
                for c in buf.chunks_exact_mut(4) {
                    let a = c[3] as u32;
                    for v in &mut c[..3] {
                        *v = ((*v as u32 * a + 127) / 255) as u8;
                    }
                }
            }
            Some((buf, w as u32, h as u32))
        };
        let _ = DeleteObject(hbm.into());
        result
    }
}

/// 取回的像素建成 D2D 位图；在界面线程调用（D2D 工厂是单线程的）
pub fn to_bitmap(rt: &ID2D1RenderTarget, (buf, w, h): &(Vec<u8>, u32, u32)) -> Option<ID2D1Bitmap> {
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    unsafe { rt.CreateBitmap(D2D_SIZE_U { width: *w, height: *h }, Some(buf.as_ptr() as _), w * 4, &props).ok() }
}

/// 没有文件路径的系统项（控制面板里的某一项等）在 dir 里建一个指向它的快捷方式，重名时加序号；返回快捷方式的文件名
pub fn make_link(item: &IShellItem, dir: &Path) -> Option<String> {
    unsafe {
        let link = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pidl = SHGetIDListFromObject(item).ok()?;
        let _ = link.SetIDList(pidl);
        CoTaskMemFree(Some(pidl as _));
        let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let mut stem = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as _));
        stem.retain(|c| !r#"\/:*?"<>|"#.contains(c));
        let file = link.cast::<IPersistFile>().ok()?;
        for n in 1..100 {
            let f = if n == 1 { format!("{stem}.lnk") } else { format!("{stem} ({n}).lnk") };
            let dst = dir.join(&f);
            if !dst.exists() {
                return file.Save(PCWSTR(wpath(&dst).as_ptr()), true).ok().map(|_| f);
            }
        }
        None
    }
}

/// 桌面上系统图标（此电脑、回收站等）的 CLSID，如 "{20D04FE0-3AEA-1069-A2D8-08002B30309D}"；别的项返回 None
pub fn desktop_clsid(item: &IShellItem) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        let id = s.strip_prefix("::")?;
        (id.starts_with('{') && !id.contains('\\')).then(|| id.to_string())
    }
}

/// 快捷方式指向的桌面系统图标的 CLSID
pub fn link_clsid(path: &Path) -> Option<String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(PCWSTR(wpath(path).as_ptr()), STGM_READ).ok()?;
        let pidl = link.GetIDList().ok()?;
        let item = SHCreateItemFromIDList::<IShellItem>(pidl);
        CoTaskMemFree(Some(pidl as _));
        desktop_clsid(&item.ok()?)
    }
}

/// 显示或隐藏桌面上的系统图标，和“桌面图标设置”里的勾选是同一个开关
pub fn show_desktop_icon(clsid: &str, show: bool) {
    let (name, v) = (wide(clsid), (!show) as u32);
    unsafe {
        let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\HideDesktopIcons\\NewStartPanel");
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, PCWSTR(name.as_ptr()), REG_DWORD.0, Some(&v as *const u32 as _), 4);
        // 让桌面马上刷新
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// 桌面上显示的系统图标（“桌面图标设置”里勾选的）的 CLSID；没设置过时回收站默认显示
pub fn desktop_icons() -> Vec<String> {
    const RECYCLE_BIN: &str = "{645FF040-5081-101B-9F08-00AA002F954E}";
    let mut ids = Vec::new();
    let mut seen_bin = false;
    unsafe {
        let mut key = HKEY::default();
        let path = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\HideDesktopIcons\\NewStartPanel");
        if RegOpenKeyExW(HKEY_CURRENT_USER, path, None, KEY_READ, &mut key) == ERROR_SUCCESS {
            for i in 0.. {
                let (mut name, mut n) = ([0u16; 64], 64u32);
                let (mut v, mut vn) = (1u32, 4u32);
                if RegEnumValueW(key, i, Some(PWSTR(name.as_mut_ptr())), &mut n, None, None, Some(&mut v as *mut u32 as _), Some(&mut vn)) != ERROR_SUCCESS {
                    break;
                }
                let id = String::from_utf16_lossy(&name[..n as usize]);
                seen_bin |= id.eq_ignore_ascii_case(RECYCLE_BIN);
                if v == 0 {
                    ids.push(id);
                }
            }
            let _ = RegCloseKey(key);
        }
    }
    if !seen_bin {
        ids.push(RECYCLE_BIN.into());
    }
    // 只留解析得出来的（聚焦壁纸的“了解此图片”等不一定在）
    ids.retain(|id| display_name(Path::new(&format!("::{id}"))).is_some());
    ids
}

/// 若干项的 Shell 对象：右键菜单、拖拽数据都从这里取
pub struct Items {
    folder: IShellFolder,
    pidls: Vec<*mut ITEMIDLIST>,
}

impl Drop for Items {
    fn drop(&mut self) {
        for p in &self.pidls {
            unsafe { CoTaskMemFree(Some(*p as _)) };
        }
    }
}

pub fn folder(dir: &Path) -> Option<IShellFolder> {
    unsafe {
        let p = wpath(dir);
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(p.as_ptr()), None, &mut pidl, 0, None).ok()?;
        let f = SHGetDesktopFolder().ok()?.BindToObject(pidl, None);
        CoTaskMemFree(Some(pidl as _));
        f.ok()
    }
}

impl Items {
    /// 系统图标（::{CLSID}）挂在桌面根下，文件挂在各自目录下。Shell 对象要求同一父级，
    /// 选中的项不在同一处时（如回收站加一个文件）只取和第一项同处的
    pub fn new(hwnd: HWND, paths: &[&Path]) -> Option<Items> {
        let parent = |p: &Path| if p.to_string_lossy().starts_with("::") { None } else { p.parent().map(Path::to_path_buf) };
        let base = parent(paths.first()?);
        let folder = match &base {
            Some(d) => folder(d)?,
            None => unsafe { SHGetDesktopFolder().ok()? },
        };
        let mut it = Items { folder, pidls: Vec::new() };
        for p in paths.iter().filter(|p| parent(p) == base) {
            let n = if base.is_some() { p.file_name()?.to_string_lossy() } else { p.to_string_lossy() };
            let w = wide(&n);
            let mut pidl = std::ptr::null_mut();
            unsafe {
                it.folder.ParseDisplayName(hwnd, None, PCWSTR(w.as_ptr()), None, &mut pidl, std::ptr::null_mut()).ok()?;
            }
            it.pidls.push(pidl);
        }
        (!it.pidls.is_empty()).then_some(it)
    }

    fn ptrs(&self) -> Vec<*const ITEMIDLIST> {
        self.pidls.iter().map(|p| *p as *const _).collect()
    }

    pub fn menu(&self, hwnd: HWND) -> Option<IContextMenu> {
        unsafe { self.folder.GetUIObjectOf(hwnd, &self.ptrs(), None).ok() }
    }

    pub fn drag(&self, hwnd: HWND) {
        unsafe {
            if let Ok(data) = self.folder.GetUIObjectOf::<IDataObject>(hwnd, &self.ptrs(), None) {
                let _ = SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK);
            }
        }
    }
}

/// 目录空白处的菜单（用来“粘贴”）
pub fn background_menu(hwnd: HWND, dir: &Path) -> Option<IContextMenu> {
    unsafe { folder(dir)?.CreateViewObject(hwnd).ok() }
}

/// 按动词执行菜单命令：copy / cut / delete / paste
pub fn invoke_verb(cm: &IContextMenu, hwnd: HWND, verb: &str, shift: bool) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL);
        let v = std::ffi::CString::new(verb).unwrap();
        let info = CMINVOKECOMMANDINFO {
            cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
            fMask: if shift { CMIC_MASK_SHIFT_DOWN } else { 0 },
            hwnd,
            lpVerb: PCSTR(v.as_ptr() as _),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let _ = cm.InvokeCommand(&info);
        let _ = DestroyMenu(menu);
    }
}

pub enum Picked {
    None,
    Rename,
}

/// 弹出系统右键菜单；菜单期间 menu2 用来转发子菜单消息（“发送到”“打开方式”）
pub fn track_menu(cm: &IContextMenu, hwnd: HWND, pt: POINT, menu2: &mut Option<IContextMenu2>) -> Picked {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return Picked::None };
        let _ = cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL | CMF_CANRENAME);
        *menu2 = cm.cast().ok();
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as u32;
        *menu2 = None;
        let _ = DestroyMenu(menu);
        if id == 0 {
            return Picked::None;
        }
        let mut verb = [0u16; 64];
        let is_rename = cm
            .GetCommandString((id - 1) as usize, GCS_VERBW, None, PSTR(verb.as_mut_ptr() as _), verb.len() as u32)
            .is_ok()
            && String::from_utf16_lossy(&verb).trim_end_matches('\0') == "rename";
        if is_rename {
            return Picked::Rename;
        }
        let info = CMINVOKECOMMANDINFO {
            cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd,
            lpVerb: PCSTR((id - 1) as usize as *const u8),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let _ = cm.InvokeCommand(&info);
        Picked::None
    }
}

/// 后台线程阻塞等桌面目录（含子目录）的变化，有变化就通知主窗口；不轮询
pub fn watch(root: PathBuf, notify: HWND) {
    let notify = notify.0 as usize;
    std::thread::spawn(move || unsafe {
        let p = wpath(&root);
        let Ok(h) = CreateFileW(
            PCWSTR(p.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) else {
            return;
        };
        let mut buf = vec![0u32; 16 * 1024];
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
        loop {
            let mut n = 0;
            if ReadDirectoryChangesW(h, buf.as_mut_ptr() as _, (buf.len() * 4) as u32, true, filter, Some(&mut n), None, None).is_err() {
                return;
            }
            // 不解析具体变化：主线程去抖后整体重扫，桌面上的文件量很小
            let _ = PostMessageW(Some(HWND(notify as _)), WM_APP_FS, WPARAM(0), LPARAM(0));
        }
    });
}

static DBL_NOTIFY: AtomicIsize = AtomicIsize::new(0);
static DBL_TIME: AtomicU32 = AtomicU32::new(0);
static DBL_X: AtomicI32 = AtomicI32::new(i32::MIN);
static DBL_Y: AtomicI32 = AtomicI32::new(0);

/// 全局鼠标钩子只看左键抬起，两次抬起落在系统双击时间和范围内就通知主窗口。
/// 钩子单独开线程：主线程复制文件、调 Shell 时会卡，钩子跑在主线程会拖慢全系统的鼠标
unsafe extern "system" fn mouse_ll(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && wp.0 as u32 == WM_LBUTTONUP {
            let m = &*(lp.0 as *const MSLLHOOKSTRUCT);
            let (dx, dy) = (GetSystemMetrics(SM_CXDOUBLECLK) / 2, GetSystemMetrics(SM_CYDOUBLECLK) / 2);
            let quick = m.time.wrapping_sub(DBL_TIME.load(Relaxed)) <= windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime();
            let near = (m.pt.x - DBL_X.load(Relaxed)).abs() <= dx && (m.pt.y - DBL_Y.load(Relaxed)).abs() <= dy;
            if quick && near {
                DBL_X.store(i32::MIN, Relaxed);
                let lp = ((m.pt.y as isize) << 32) | (m.pt.x as u32 as isize);
                let _ = PostMessageW(Some(HWND(DBL_NOTIFY.load(Relaxed) as _)), WM_APP_DBLCLK, WPARAM(0), LPARAM(lp));
            } else {
                DBL_TIME.store(m.time, Relaxed);
                DBL_X.store(m.pt.x, Relaxed);
                DBL_Y.store(m.pt.y, Relaxed);
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

/// 开始监听双击，返回钩子线程 id（停用时传给 stop_thread）
pub fn watch_dblclick(notify: HWND) -> u32 {
    DBL_NOTIFY.store(notify.0 as isize, Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let _ = tx.send(windows::Win32::System::Threading::GetCurrentThreadId());
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_ll), None, 0);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        if let Ok(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
    });
    rx.recv().unwrap_or(0)
}

pub fn stop_thread(tid: u32) {
    unsafe {
        let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回收站和文件各自取得到右键菜单；混在一起选时只取和第一项同处的
    #[test]
    fn system_icon_and_file_items() {
        unsafe {
            let _ = OleInitialize(None);
        }
        let bin = Path::new("::{645FF040-5081-101B-9F08-00AA002F954E}");
        assert!(display_name(bin).is_some_and(|n| !n.is_empty()));
        let f = std::env::temp_dir().join("nookery-items-test.txt");
        std::fs::write(&f, "x").unwrap();
        let mixed = Items::new(HWND::default(), &[bin, &f]).unwrap();
        assert_eq!(mixed.pidls.len(), 1);
        assert!(mixed.menu(HWND::default()).is_some());
        let file = Items::new(HWND::default(), &[&f, &f]).unwrap();
        assert_eq!(file.pidls.len(), 2);
        assert!(file.menu(HWND::default()).is_some());
        let _ = std::fs::remove_file(&f);
    }
}
