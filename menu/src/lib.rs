//! Win11 桌面右键菜单第一级的“新建磁贴”（IExplorerCommand）。
//! 由安装包注册的稀疏包（installer/AppxManifest.xml）声明，资源管理器在 dllhost 里加载本 DLL；
//! 点击后启动同目录的 nookery.exe --new-tile。老式菜单（显示更多选项）里的同名项由 nookery.exe 自己写注册表。
// COM 导出函数按惯例应标 PRIVATE，Rust 没有对应写法，链接器会报 LNK4104，不影响使用
#![allow(linker_messages)]

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::*;

#[path = "../../src/lang.rs"]
mod lang;

/// 和 AppxManifest.xml 里的 Clsid 一致
const CLSID: GUID = GUID::from_u128(0xced55fd0_06a3_4902_8881_fe3eabd415a5);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// 本 DLL 所在目录（安装目录）
fn dir() -> PathBuf {
    let mut m = HMODULE::default();
    let mut buf = [0u16; 1024];
    unsafe {
        let _ = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(dir as *const u16),
            &mut m,
        );
        let n = GetModuleFileNameW(Some(m), &mut buf) as usize;
        PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(PathBuf::from).unwrap_or_default()
    }
}

/// 取出系统分配的字符串并释放
fn take(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// 右键的是不是桌面背景：按 Directory\Background 注册，普通文件夹窗口的空白处也会问到这里
fn on_desktop(items: Ref<IShellItemArray>) -> bool {
    let here = items.as_ref().and_then(|a| unsafe { a.GetItemAt(0) }.ok()).and_then(|i| unsafe { i.GetDisplayName(SIGDN_FILESYSPATH) }.ok()).map(take);
    let Some(here) = here else { return true };
    unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) }.ok().map(take).is_some_and(|d| d.eq_ignore_ascii_case(&here))
}

#[implement(IExplorerCommand)]
struct NewTile;

impl IExplorerCommand_Impl for NewTile_Impl {
    fn GetTitle(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        lang::load();
        unsafe { SHStrDupW(PCWSTR(wide(lang::tr("新建磁贴", "New tile")).as_ptr())) }
    }

    fn GetIcon(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        unsafe { SHStrDupW(w!("imageres.dll,-183")) }
    }

    fn GetToolTip(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID)
    }

    fn GetState(&self, items: Ref<IShellItemArray>, _: BOOL) -> Result<u32> {
        Ok(if on_desktop(items) { ECS_ENABLED } else { ECS_HIDDEN }.0 as u32)
    }

    fn Invoke(&self, _: Ref<IShellItemArray>, _: Ref<IBindCtx>) -> Result<()> {
        let child = std::process::Command::new(dir().join("nookery.exe")).arg("--new-tile").spawn().map_err(|_| Error::from(E_FAIL))?;
        // 资源管理器把前台权限给了本进程，转给 nookery，改名框才拿得到焦点
        unsafe {
            let _ = AllowSetForegroundWindow(child.id());
        }
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        unsafe { *out = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let cmd: IExplorerCommand = NewTile.into();
        unsafe { cmd.query(iid, out).ok() }
    }

    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        *out = std::ptr::null_mut();
        if *clsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let f: IClassFactory = Factory.into();
        f.query(iid, out)
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
