//! 和资源管理器打交道：图标、右键菜单、拖放、文件移动、桌面监听。

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicU32};
use std::time::{Duration, SystemTime};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

pub const WM_APP_FS: u32 = WM_APP + 1;
pub const WM_APP_DBLCLK: u32 = WM_APP + 3;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn wpath(p: &Path) -> Vec<u16> {
    wide(&p.to_string_lossy())
}

pub fn desktop_dir() -> PathBuf {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).expect("找不到桌面目录");
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        PathBuf::from(s)
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

/// 用户正在桌面上给图标改名（列表里有编辑框）时不归类，否则“新建文件夹”还没起好名字就被拿走了
pub fn label_editing() -> bool {
    desktop_listview().is_some_and(|lv| unsafe { FindWindowExW(Some(lv), None, w!("Edit"), PCWSTR::null()).is_ok() })
}

/// 这一点是不是桌面空白处：点在桌面图标列表上，且没有选中任何图标（点空白会清空选中）
pub fn desktop_blank_at(pt: POINT) -> bool {
    const LVM_GETSELECTEDCOUNT: u32 = 0x1032;
    let Some(lv) = desktop_listview() else { return false };
    let mut n = 1usize;
    unsafe {
        WindowFromPoint(pt) == lv
            && SendMessageTimeoutW(lv, LVM_GETSELECTEDCOUNT, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 200, Some(&mut n)).0 != 0
            && n == 0
    }
}

/// 3 秒内没改动、并且能独占打开，才算写完。
/// OneDrive 只在云端的占位文件不去打开（一打开就会触发下载），直接算稳定
pub fn is_stable(path: &Path, md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    let modified = md.modified().unwrap_or(SystemTime::now());
    let quiet = SystemTime::now().duration_since(modified).map_or(true, |d| d >= Duration::from_secs(3));
    let cloud = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0 | FILE_ATTRIBUTE_RECALL_ON_OPEN.0 | FILE_ATTRIBUTE_OFFLINE.0;
    quiet && (md.is_dir() || md.file_attributes() & cloud != 0 || std::fs::OpenOptions::new().read(true).share_mode(0).open(path).is_ok())
}

/// 移进 dir，重名时依次改成“名字 (2)”“名字 (3)”…；不覆盖已有文件
pub fn move_unique(src: &Path, dir: &Path) -> bool {
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
            Ok(()) => return true,
            Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() || e.code() == ERROR_FILE_EXISTS.to_hresult() => {}
            Err(_) => return false,
        }
    }
    false
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

pub fn open(path: &Path) {
    let p = wpath(path);
    unsafe { ShellExecuteW(None, PCWSTR::null(), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
}

/// 系统图标或缩略图，转成 D2D 位图
pub fn load_icon(rt: &ID2D1RenderTarget, path: &Path, px: i32) -> Option<ID2D1Bitmap> {
    unsafe {
        let p = wpath(path);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(p.as_ptr()), None).ok()?;
        to_bitmap(rt, &f, px, SIIGBF_RESIZETOFIT)
    }
}

/// 一类文件的原版图标（关联程序的图标），文件不需要存在：用只带路径的简单 PIDL，资源管理器按扩展名取图标
pub fn load_type_icon(rt: &ID2D1RenderTarget, name: &str, px: i32) -> Option<ID2D1Bitmap> {
    unsafe {
        let p = wide(&format!("C:\\{name}"));
        let pidl = SHSimpleIDListFromPath(PCWSTR(p.as_ptr()));
        if pidl.is_null() {
            return None;
        }
        let f = SHCreateItemFromIDList::<IShellItemImageFactory>(pidl);
        ILFree(Some(pidl));
        to_bitmap(rt, &f.ok()?, px, SIIGBF_ICONONLY)
    }
}

fn to_bitmap(rt: &ID2D1RenderTarget, f: &IShellItemImageFactory, px: i32, flags: SIIGBF) -> Option<ID2D1Bitmap> {
    unsafe {
        let hbm = f.GetImage(SIZE { cx: px, cy: px }, flags).ok()?;
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
            }
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            rt.CreateBitmap(D2D_SIZE_U { width: w as u32, height: h as u32 }, Some(buf.as_ptr() as _), (w * 4) as u32, &props)
                .ok()
        };
        let _ = DeleteObject(hbm.into());
        result
    }
}

/// 一个目录下若干项的 Shell 对象：右键菜单、拖拽数据都从这里取
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
    pub fn new(hwnd: HWND, dir: &Path, names: &[String]) -> Option<Items> {
        let folder = folder(dir)?;
        let mut it = Items { folder, pidls: Vec::new() };
        for n in names {
            let w = wide(n);
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
/// 钩子单独开线程：主线程取图标时会卡上百毫秒，钩子跑在主线程会拖慢全系统的鼠标
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
