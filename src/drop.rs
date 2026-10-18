//! 拖进磁贴：OLE 拖放目标。拖过时高亮磁贴、显示系统拖拽缩略图，松手后移动或复制进磁贴目录。
//! 规则和资源管理器一样：同盘移动、跨盘复制，按住 Ctrl 复制、按住 Shift 移动。
//! 在磁贴内拖动是排序；此电脑、回收站这类没有文件路径的系统项建快捷方式。

use std::cell::RefCell;
use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::*;
use windows::core::*;

use crate::tile::Tile;
use crate::{app, same_path};

#[implement(IDropTarget)]
struct Target {
    tile: *mut Tile,
    files: RefCell<Vec<PathBuf>>,
    /// 拖来的系统项（没有文件路径）
    links: RefCell<Vec<IShellItem>>,
}

fn hdrop_files(d: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut m) = d.GetData(&fmt) else { return Vec::new() };
        let files = crate::shell::hdrop_paths(HDROP(m.u.hGlobal.0));
        ReleaseStgMedium(&mut m);
        files
    }
}

/// 数据里没有文件路径的 Shell 项：从桌面拖来的此电脑、回收站等不带 CF_HDROP，只有 Shell 项列表
fn virtual_items(d: &IDataObject) -> Vec<IShellItem> {
    unsafe {
        let Ok(arr) = SHCreateShellItemArrayFromDataObject::<_, IShellItemArray>(d) else { return Vec::new() };
        (0..arr.GetCount().unwrap_or(0))
            .filter_map(|i| arr.GetItemAt(i).ok())
            .filter(|it| match it.GetDisplayName(SIGDN_FILESYSPATH) {
                Ok(p) => {
                    CoTaskMemFree(Some(p.0 as _));
                    false
                }
                Err(_) => true,
            })
            .collect()
    }
}

impl Target {
    fn tile(&self) -> &mut Tile {
        unsafe { &mut *self.tile }
    }

    /// 拖的全是本磁贴里的项：排序
    fn inside(&self) -> bool {
        let files = self.files.borrow();
        let dir = self.tile().dir(app());
        !files.is_empty() && files.iter().all(|f| f.parent().is_some_and(|p| same_path(p, &dir)))
    }

    fn effect(&self, keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
        if !self.links.borrow().is_empty() {
            return DROPEFFECT_LINK;
        }
        let files = self.files.borrow();
        let dir = self.tile().dir(app());
        if files.is_empty() {
            return DROPEFFECT_NONE;
        }
        if self.inside() {
            return DROPEFFECT_MOVE;
        }
        let same_volume = files.iter().all(|f| f.components().next() == dir.components().next());
        if keys.0 & MK_CONTROL.0 != 0 {
            DROPEFFECT_COPY
        } else if keys.0 & MK_SHIFT.0 != 0 || same_volume {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_COPY
        }
    }

    /// 外面拖进来高亮整块磁贴；磁贴内排序只画插入线
    fn hover(&self, e: DROPEFFECT, pt: &POINTL) {
        let t = self.tile();
        let (hot, slot) = match (e != DROPEFFECT_NONE, self.inside()) {
            (true, true) => (false, Some(t.slot_at(pt.x, pt.y))),
            (on, _) => (on, None),
        };
        if t.drop_hot != hot || t.insert != slot {
            (t.drop_hot, t.insert) = (hot, slot);
            t.invalidate();
        }
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, obj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let obj = obj.ok()?;
        let files = hdrop_files(obj);
        *self.links.borrow_mut() = if files.is_empty() { virtual_items(obj) } else { Vec::new() };
        *self.files.borrow_mut() = files;
        let e = self.effect(keys);
        unsafe {
            *effect = e;
            if let Some(h) = &app().drop_helper {
                let _ = h.DragEnter(self.tile().hwnd, obj, &POINT { x: pt.x, y: pt.y }, e);
            }
        }
        self.hover(e, pt);
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let e = self.effect(keys);
        unsafe {
            *effect = e;
            if let Some(h) = &app().drop_helper {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, e);
            }
        }
        self.hover(e, pt);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(h) = &app().drop_helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        self.hover(DROPEFFECT_NONE, &POINTL::default());
        Ok(())
    }

    fn Drop(&self, obj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let e = self.effect(keys);
        let inside = self.inside();
        unsafe {
            // 排序不动文件，告诉拖动源什么都没做
            *effect = if inside { DROPEFFECT_NONE } else { e };
            if let (Some(h), Ok(obj)) = (&app().drop_helper, obj.ok()) {
                let _ = h.Drop(obj, &POINT { x: pt.x, y: pt.y }, e);
            }
        }
        self.hover(DROPEFFECT_NONE, pt);
        let t = self.tile();
        let files = std::mem::take(&mut *self.files.borrow_mut());
        let links = std::mem::take(&mut *self.links.borrow_mut());
        if inside {
            let moved: Vec<String> = files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()).collect();
            app().reorder(t, &moved, t.slot_at(pt.x, pt.y));
        } else if !links.is_empty() {
            let dir = t.dir(app());
            let _ = std::fs::create_dir_all(&dir);
            links.iter().for_each(|it| crate::shell::make_link(it, &dir));
        } else if e != DROPEFFECT_NONE {
            app().drop_into(t, files, e == DROPEFFECT_COPY);
        }
        Ok(())
    }
}

pub fn register(t: &mut Tile) {
    let target: IDropTarget = Target { tile: t, files: RefCell::new(Vec::new()), links: RefCell::new(Vec::new()) }.into();
    unsafe {
        let _ = RegisterDragDrop(t.hwnd, &target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 此电脑没有文件路径：从它的数据对象里认得出来，并能在目录里建出快捷方式
    #[test]
    fn this_pc_becomes_link() {
        unsafe {
            let _ = OleInitialize(None);
            let pc: IShellItem = SHCreateItemFromParsingName(w!("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), None).unwrap();
            let d: IDataObject = pc.BindToHandler(None, &BHID_DataObject).unwrap();
            assert!(hdrop_files(&d).is_empty());
            let items = virtual_items(&d);
            assert_eq!(items.len(), 1);
            let dir = std::env::temp_dir().join("nookery-link-test");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            crate::shell::make_link(&items[0], &dir);
            crate::shell::make_link(&items[0], &dir);
            let links: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "lnk")).collect();
            let ids: Vec<_> = links.iter().map(|p| crate::shell::link_clsid(p)).collect();
            let _ = std::fs::remove_dir_all(&dir);
            let pc = Some("{20D04FE0-3AEA-1069-A2D8-08002B30309D}".to_string());
            assert_eq!(ids, [pc.clone(), pc]);
            // 普通文件夹不算系统图标
            let win: IShellItem = SHCreateItemFromParsingName(w!("C:\\Windows"), None).unwrap();
            assert_eq!(crate::shell::desktop_clsid(&win), None);
        }
    }
}
