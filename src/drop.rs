//! 拖进磁贴：OLE 拖放目标。拖过时高亮磁贴、显示系统拖拽缩略图，松手后移动或复制进磁贴目录。
//! 规则和资源管理器一样：同盘移动、跨盘复制，按住 Ctrl 复制、按住 Shift 移动。

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

impl Target {
    fn tile(&self) -> &mut Tile {
        unsafe { &mut *self.tile }
    }

    fn effect(&self, keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
        let files = self.files.borrow();
        let dir = self.tile().dir(app());
        // 全是本磁贴里的文件（在磁贴内拖动）时不接
        if files.is_empty() || files.iter().all(|f| f.parent().is_some_and(|p| same_path(p, &dir))) {
            return DROPEFFECT_NONE;
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

    fn set_hot(&self, hot: bool) {
        let t = self.tile();
        if t.drop_hot != hot {
            t.drop_hot = hot;
            t.invalidate();
        }
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, obj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let obj = obj.ok()?;
        *self.files.borrow_mut() = hdrop_files(obj);
        let e = self.effect(keys);
        unsafe {
            *effect = e;
            if let Some(h) = &app().drop_helper {
                let _ = h.DragEnter(self.tile().hwnd, obj, &POINT { x: pt.x, y: pt.y }, e);
            }
        }
        self.set_hot(e != DROPEFFECT_NONE);
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
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(h) = &app().drop_helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        self.set_hot(false);
        Ok(())
    }

    fn Drop(&self, obj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let e = self.effect(keys);
        unsafe {
            *effect = e;
            if let (Some(h), Ok(obj)) = (&app().drop_helper, obj.ok()) {
                let _ = h.Drop(obj, &POINT { x: pt.x, y: pt.y }, e);
            }
        }
        self.set_hot(false);
        if e != DROPEFFECT_NONE {
            let files = std::mem::take(&mut *self.files.borrow_mut());
            app().drop_into(self.tile(), files, e == DROPEFFECT_COPY);
        }
        Ok(())
    }
}

pub fn register(t: &mut Tile) {
    let target: IDropTarget = Target { tile: t, files: RefCell::new(Vec::new()) }.into();
    unsafe {
        let _ = RegisterDragDrop(t.hwnd, &target);
    }
}
