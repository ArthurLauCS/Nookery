//! 拖进磁贴：OLE 拖放目标。拖过时高亮磁贴、显示系统拖拽缩略图，松手后移动或复制进磁贴目录。
//! 规则和资源管理器一样：同盘移动、跨盘复制，按住 Ctrl 复制、按住 Shift 移动。
//! 在磁贴内拖动是排序。

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

    /// 拖的全是本磁贴里的项：排序
    fn inside(&self) -> bool {
        let files = self.files.borrow();
        let dir = self.tile().dir(app());
        !files.is_empty() && files.iter().all(|f| f.parent().is_some_and(|p| same_path(p, &dir)))
    }

    fn effect(&self, keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
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
        *self.files.borrow_mut() = hdrop_files(obj);
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
        if inside {
            let moved: Vec<String> = files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()).collect();
            app().reorder(t, &moved, t.slot_at(pt.x, pt.y));
        } else if e != DROPEFFECT_NONE {
            app().drop_into(t, files, e == DROPEFFECT_COPY);
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
