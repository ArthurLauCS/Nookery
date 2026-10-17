//! 磁贴窗口：绘制和交互。尺寸常量都是 DIP，绘制时由渲染目标按窗口 DPI 缩放。

use std::path::PathBuf;
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::gfx::Surface;
use crate::shell::{self, Items, Picked};
use crate::{App, app, popup};

pub const TITLE_H: f32 = 34.0;
pub const PAD: f32 = 8.0;
const CELL_W: f32 = 84.0;
pub const CELL_H: f32 = 86.0;
const ICON: f32 = 40.0;
const LABEL_H: f32 = 32.0;
const EDGE: f32 = 6.0;

pub struct Item {
    pub name: String,
    pub is_dir: bool,
    pub mtime: SystemTime,
    icon: Option<ID2D1Bitmap>,
    icon_tried: bool,
    layout: Option<IDWriteTextLayout>,
}

pub struct Tile {
    pub name: String,
    pub hwnd: HWND,
    pub rolled: bool,
    pub items: Vec<Item>,
    surf: Option<Surface>,
    pub painting: bool,
    repaint: bool,
    scroll: f32,
    hover: Option<usize>,
    tracking: bool,
    pub sel: Vec<String>,
    anchor: usize,
    /// 按下左键的位置和项，移动超过拖拽阈值就开始拖出
    press: Option<(POINT, usize)>,
    /// 改名编辑框；Some(文件名) 改文件，None 改磁贴名
    edit: Option<(HWND, Option<String>)>,
    /// 有文件正拖在上面
    pub drop_hot: bool,
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

fn lparam_pt(lp: LPARAM) -> POINT {
    POINT { x: lp.0 as i16 as i32, y: (lp.0 >> 16) as i16 as i32 }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// 每行几格、每格多宽
fn grid(w: f32) -> (usize, f32) {
    let inner = w - 2.0 * PAD;
    let n = ((inner / CELL_W) as usize).max(1);
    (n, inner / n as f32)
}

impl Tile {
    pub fn create(a: &App, name: &str, r: [i32; 4], rolled: bool) -> Box<Tile> {
        let mut t = Box::new(Tile {
            name: name.into(),
            hwnd: HWND::default(),
            rolled,
            items: Vec::new(),
            surf: None,
            painting: false,
            repaint: false,
            scroll: 0.0,
            hover: None,
            tracking: false,
            sel: Vec::new(),
            anchor: 0,
            press: None,
            edit: None,
            drop_hot: false,
        });
        unsafe {
            let title = shell::wide(name);
            t.hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                crate::TILE_CLASS,
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
                r[0],
                r[1],
                r[2],
                r[3],
                a.owner,
                None,
                None,
                None,
            )
            .expect("创建磁贴窗口失败");
            SetWindowLongPtrW(t.hwnd, GWLP_USERDATA, &mut *t as *mut Tile as isize);
            crate::drop::register(&mut t);
            if rolled {
                let _ = SetWindowPos(t.hwnd, None, 0, 0, r[2], (TITLE_H * scale(t.hwnd)) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            t.apply_style(a);
        }
        t
    }

    /// 亚克力背景、圆角、深浅色
    pub fn apply_style(&self, a: &App) {
        unsafe {
            use windows::Win32::Graphics::Dwm::*;
            let dark = BOOL::from(a.theme.dark);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
            // 系统亚克力（DWMSBT_TRANSIENTWINDOW）在窗口未激活时退成纯色，磁贴几乎总是未激活，
            // 所以用 SetWindowCompositionAttribute 的亚克力模糊：未公开，但从 Win10 起一直可用
            #[repr(C)]
            struct Accent {
                state: u32,
                flags: u32,
                tint: u32,
                anim: u32,
            }
            #[repr(C)]
            struct Data {
                attr: u32,
                data: *mut Accent,
                size: usize,
            }
            type Swca = unsafe extern "system" fn(HWND, *mut Data) -> BOOL;
            let user32 = windows::Win32::System::LibraryLoader::GetModuleHandleW(w!("user32.dll")).unwrap_or_default();
            if let Some(f) = windows::Win32::System::LibraryLoader::GetProcAddress(user32, s!("SetWindowCompositionAttribute")) {
                let f: Swca = std::mem::transmute(f);
                // 4 = ACCENT_ENABLE_ACRYLICBLURBEHIND；tint 是 0xAABBGGRR，染色交给 D2D 画的底色
                let mut accent = Accent { state: 4, flags: 0, tint: 0x01000000, anim: 0 };
                let mut d = Data { attr: 19, data: &mut accent, size: size_of::<Accent>() };
                let _ = f(self.hwnd, &mut d);
            }
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn dir(&self, a: &App) -> PathBuf {
        a.data.join(&self.name)
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// 内容变了才重建列表，图标按（名字，修改时间）复用
    pub fn set_items(&mut self, list: Vec<(String, bool, SystemTime)>) {
        let same = list.len() == self.items.len()
            && list.iter().zip(&self.items).all(|((n, _, m), it)| *n == it.name && *m == it.mtime);
        if same {
            return;
        }
        let mut old: Vec<Item> = std::mem::take(&mut self.items);
        self.items = list
            .into_iter()
            .map(|(name, is_dir, mtime)| match old.iter().position(|o| o.name == name && o.mtime == mtime) {
                Some(i) => old.swap_remove(i),
                None => Item { name, is_dir, mtime, icon: None, icon_tried: false, layout: None },
            })
            .collect();
        self.sel.retain(|s| self.items.iter().any(|it| it.name == *s));
        self.hover = None;
        self.clamp_scroll();
        self.invalidate();
    }

    fn size_dip(&self) -> (f32, f32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let s = scale(self.hwnd);
        (r.right as f32 / s, r.bottom as f32 / s)
    }

    fn item_rect(&self, i: usize, w: f32) -> D2D_RECT_F {
        let (n, cw) = grid(w);
        let x = PAD + (i % n) as f32 * cw;
        let y = TITLE_H + PAD + (i / n) as f32 * CELL_H - self.scroll;
        rect(x, y, x + cw, y + CELL_H)
    }

    fn content_h(&self, w: f32) -> f32 {
        let (n, _) = grid(w);
        self.items.len().div_ceil(n) as f32 * CELL_H + 2.0 * PAD
    }

    fn clamp_scroll(&mut self) {
        let (w, h) = self.size_dip();
        let max = (self.content_h(w) - (h - TITLE_H)).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let (w, _) = self.size_dip();
        let (n, cw) = grid(w);
        if y < TITLE_H || x < PAD {
            return None;
        }
        let col = ((x - PAD) / cw) as usize;
        let row = ((y - TITLE_H - PAD + self.scroll) / CELL_H).floor();
        if col >= n || row < 0.0 {
            return None;
        }
        let i = row as usize * n + col;
        (i < self.items.len()).then_some(i)
    }

    fn display_name<'n>(a: &App, it: &'n Item) -> &'n str {
        match it.name.rfind('.') {
            Some(i) if a.hide_ext && !it.is_dir && i > 0 => &it.name[..i],
            _ => &it.name,
        }
    }

    pub fn drop_device(&mut self) {
        self.surf = None;
        for it in &mut self.items {
            it.icon = None;
            it.icon_tried = false;
        }
    }

    fn paint(&mut self, a: &App) {
        unsafe {
            let _ = ValidateRect(Some(self.hwnd), None);
        }
        // 取图标时 Shell 会抽消息，同一个磁贴的 WM_PAINT 可能重入：记下来，画完再补一次
        if self.painting {
            self.repaint = true;
            return;
        }
        self.painting = true;
        let (w, h) = self.size_dip();
        let (s, dir) = (scale(self.hwnd), self.dir(a));
        for i in 0..self.items.len() {
            let r = self.item_rect(i, w);
            let it = &mut self.items[i];
            if !it.icon_tried && r.bottom >= TITLE_H && r.top <= h {
                it.icon_tried = true;
                it.icon = shell::load_icon(&a.gfx.dc, &dir.join(&it.name), (ICON * s) as i32);
            }
        }
        self.draw(a);
        self.painting = false;
        if std::mem::take(&mut self.repaint) {
            self.invalidate();
        }
    }

    fn draw(&mut self, a: &App) {
        unsafe {
            if self.surf.is_none() {
                let mut r = RECT::default();
                let _ = GetClientRect(self.hwnd, &mut r);
                self.surf = a.gfx.surface(self.hwnd, r.right as u32, r.bottom as u32).ok();
            }
            let Some(surf) = self.surf.clone() else { return };
            let (rt, br) = (&a.gfx.dc, &a.gfx.brush);
            if a.gfx.bind(&surf, GetDpiForWindow(self.hwnd) as f32).is_err() {
                return app().reset_gfx();
            }
            let th = &a.theme;
            let (w, h) = self.size_dip();
            rt.BeginDraw();
            rt.Clear(Some(&th.tint));

            br.SetColor(&th.text);
            rt.DrawText(&u16s(&self.name), &a.fmt_title, &rect(14.0, 0.0, w - 48.0, TITLE_H), br, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            br.SetColor(&th.muted);
            let count = self.items.len().to_string();
            rt.DrawText(&u16s(&count), &a.fmt_count, &rect(w - 48.0, 0.0, w - 14.0, TITLE_H), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);

            if !self.rolled {
                br.SetColor(&th.line);
                rt.FillRectangle(&rect(10.0, TITLE_H - 1.0, w - 10.0, TITLE_H), br);
                if self.items.is_empty() {
                    br.SetColor(&th.muted);
                    rt.DrawText(&u16s("拖放文件到这里"), &a.fmt_hint, &rect(0.0, TITLE_H, w, h), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
                rt.PushAxisAlignedClip(&rect(0.0, TITLE_H, w, h), D2D1_ANTIALIAS_MODE_ALIASED);
                let s = scale(self.hwnd);
                let (_, cw) = grid(w);
                for i in 0..self.items.len() {
                    let r = self.item_rect(i, w);
                    if r.bottom < TITLE_H || r.top > h {
                        continue;
                    }
                    let cell = D2D1_ROUNDED_RECT { rect: rect(r.left + 2.0, r.top + 2.0, r.right - 2.0, r.bottom - 2.0), radiusX: 6.0, radiusY: 6.0 };
                    let selected = self.sel.contains(&self.items[i].name);
                    if selected {
                        br.SetColor(&th.sel_fill);
                        rt.FillRoundedRectangle(&cell, br);
                        br.SetColor(&th.sel_line);
                        rt.DrawRoundedRectangle(&cell, br, 1.0, None);
                    } else if self.hover == Some(i) {
                        br.SetColor(&th.hover);
                        rt.FillRoundedRectangle(&cell, br);
                    }
                    let it = &mut self.items[i];
                    if let Some(bmp) = &it.icon {
                        let sz = bmp.GetSize();
                        let (bw, bh) = (sz.width / s, sz.height / s);
                        let x = (r.left + r.right - bw) / 2.0;
                        let y = r.top + 8.0 + (ICON - bh) / 2.0;
                        rt.DrawBitmap(bmp, Some(&rect(x, y, x + bw, y + bh)), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                    }
                    if it.layout.is_none() {
                        let text = u16s(Self::display_name(a, it));
                        it.layout = a.dw.CreateTextLayout(&text, &a.fmt_label, cw - 8.0, LABEL_H).ok();
                    }
                    if let Some(l) = &it.layout {
                        br.SetColor(&th.text);
                        rt.DrawTextLayout(Vector2 { X: r.left + 4.0, Y: r.top + 14.0 + ICON }, l, br, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                    }
                }
                rt.PopAxisAlignedClip();
                let total = self.content_h(w);
                let view = h - TITLE_H;
                if total > view {
                    let bar = (view * view / total).max(24.0);
                    let top = TITLE_H + (view - bar) * self.scroll / (total - view);
                    br.SetColor(&th.muted);
                    rt.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT { rect: rect(w - 5.0, top + 2.0, w - 2.0, top + bar - 2.0), radiusX: 1.5, radiusY: 1.5 },
                        br,
                    );
                }
            }
            if self.drop_hot {
                let edge = D2D1_ROUNDED_RECT { rect: rect(1.0, 1.0, w - 1.0, h - 1.0), radiusX: 7.0, radiusY: 7.0 };
                br.SetColor(&D2D1_COLOR_F { a: 0.12, ..th.sel_line });
                rt.FillRoundedRectangle(&edge, br);
                br.SetColor(&th.sel_line);
                rt.DrawRoundedRectangle(&edge, br, 2.0, None);
            }
            let ok = rt.EndDraw(None, None).is_ok();
            if !(a.gfx.present(&surf).is_ok() && ok) {
                // 显卡驱动重置等：整套设备重建
                app().reset_gfx();
            }
        }
    }

    fn nc_hit(&self, lp: LPARAM) -> LRESULT {
        let p = lparam_pt(lp);
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        let s = scale(self.hwnd);
        let e = (EDGE * s) as i32;
        let (l, rt) = (p.x < r.left + e, p.x >= r.right - e);
        let (t, b) = (!self.rolled && p.y < r.top + e, !self.rolled && p.y >= r.bottom - e);
        let code = match (l, rt, t, b) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, ..) => HTLEFT,
            (_, true, ..) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ if p.y < r.top + (TITLE_H * s) as i32 => HTCAPTION,
            _ => HTCLIENT,
        };
        LRESULT(code as isize)
    }

    fn selected_idx(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|i| self.sel.contains(&self.items[*i].name)).collect()
    }

    fn sel_items(&self, a: &App) -> Option<Items> {
        Items::new(self.hwnd, &self.dir(a), &self.sel)
    }

    fn on_down(&mut self, lp: LPARAM) {
        let p = lparam_pt(lp);
        let s = scale(self.hwnd);
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        match self.hit(p.x as f32 / s, p.y as f32 / s) {
            Some(i) => {
                let name = self.items[i].name.clone();
                if key_down(VK_CONTROL) {
                    if let Some(k) = self.sel.iter().position(|n| *n == name) {
                        self.sel.remove(k);
                    } else {
                        self.sel.push(name);
                    }
                } else if key_down(VK_SHIFT) {
                    let (a, b) = (self.anchor.min(i), self.anchor.max(i).min(self.items.len() - 1));
                    self.sel = self.items[a..=b].iter().map(|it| it.name.clone()).collect();
                } else if !self.sel.contains(&name) {
                    self.sel = vec![name];
                }
                if !key_down(VK_SHIFT) {
                    self.anchor = i;
                }
                self.press = Some((p, i));
            }
            None => self.sel.clear(),
        }
        self.invalidate();
    }

    fn on_move(&mut self, a: &App, wp: WPARAM, lp: LPARAM) {
        let p = lparam_pt(lp);
        let s = scale(self.hwnd);
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            self.tracking = unsafe { TrackMouseEvent(&mut tme).is_ok() };
        }
        let hover = self.hit(p.x as f32 / s, p.y as f32 / s);
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
        if let Some((p0, _)) = self.press {
            let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if wp.0 as u32 & MK_LBUTTON.0 == 0 {
                self.press = None;
            } else if (p.x - p0.x).abs() > dx || (p.y - p0.y).abs() > dy {
                self.press = None;
                if let Some(items) = self.sel_items(a) {
                    items.drag(self.hwnd);
                }
            }
        }
    }

    fn on_up(&mut self) {
        // 在多选中的某项上单击（没拖动），松开后只选它
        if let Some((_, i)) = self.press.take() {
            if !key_down(VK_CONTROL) && !key_down(VK_SHIFT) {
                self.sel = vec![self.items[i].name.clone()];
                self.invalidate();
            }
        }
    }

    fn open_selected(&self, a: &App) {
        let dir = self.dir(a);
        for i in self.selected_idx() {
            shell::open(&dir.join(&self.items[i].name));
        }
    }

    fn verb(&self, a: &App, verb: &str) {
        if let Some(cm) = self.sel_items(a).and_then(|it| it.menu(self.hwnd)) {
            shell::invoke_verb(&cm, self.hwnd, verb, key_down(VK_SHIFT));
        }
    }

    fn paste(&self, a: &App) {
        let dir = self.dir(a);
        let _ = std::fs::create_dir_all(&dir);
        if let Some(cm) = shell::background_menu(self.hwnd, &dir) {
            shell::invoke_verb(&cm, self.hwnd, "paste", false);
        }
    }

    fn on_key(&mut self, a: &mut App, vk: u16) {
        let ctrl = key_down(VK_CONTROL);
        match VIRTUAL_KEY(vk) {
            VK_F2 => {
                if let Some(&i) = self.selected_idx().first() {
                    self.start_rename(a, Some(i));
                }
            }
            VK_DELETE => self.verb(a, "delete"),
            VK_RETURN => self.open_selected(a),
            VK_F5 => a.rescan(),
            VK_A if ctrl => {
                self.sel = self.items.iter().map(|it| it.name.clone()).collect();
                self.invalidate();
            }
            VK_C if ctrl => self.verb(a, "copy"),
            VK_X if ctrl => self.verb(a, "cut"),
            VK_V if ctrl => self.paste(a),
            _ => {}
        }
    }

    fn on_rclick(&mut self, a: &mut App, lp: LPARAM) {
        let p = lparam_pt(lp);
        let s = scale(self.hwnd);
        let mut screen = p;
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut screen);
        }
        let Some(i) = self.hit(p.x as f32 / s, p.y as f32 / s) else {
            return self.tile_menu(a, screen);
        };
        if !self.sel.contains(&self.items[i].name) {
            self.sel = vec![self.items[i].name.clone()];
            self.invalidate();
        }
        let Some(cm) = self.sel_items(a).and_then(|it| it.menu(self.hwnd)) else { return };
        if let Picked::Rename = shell::track_menu(&cm, self.hwnd, screen, &mut a.menu2) {
            self.start_rename(a, Some(i));
        }
    }

    pub fn tile_menu(&mut self, a: &mut App, pt: POINT) {
        let mut items: Vec<(u32, &str)> = vec![(1, "重命名磁贴"), (2, if self.rolled { "展开" } else { "折叠" }), (3, "在资源管理器中打开")];
        if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() } {
            items.push((6, "粘贴"));
        }
        items.extend([(4, "新建磁贴"), (0, ""), (5, "删除磁贴")]);
        match popup(self.hwnd, pt, &items) {
            1 => self.start_rename(a, None),
            2 => a.toggle_roll(self),
            3 => {
                let dir = self.dir(a);
                let _ = std::fs::create_dir_all(&dir);
                shell::open(&dir);
            }
            4 => a.new_tile(),
            5 => {
                let msg = shell::wide(&format!("删除磁贴「{}」？里面的文件会放回桌面，再按规则归类。", self.name));
                let ok = unsafe { MessageBoxW(Some(self.hwnd), PCWSTR(msg.as_ptr()), w!("DeskTiles"), MB_YESNO | MB_ICONQUESTION) } == IDYES;
                if ok {
                    // 磁贴会被销毁，之后不能再碰 self
                    let name = self.name.clone();
                    return a.delete_tile(&name);
                }
            }
            6 => self.paste(a),
            _ => {}
        }
    }

    pub fn start_rename(&mut self, a: &App, item: Option<usize>) {
        self.finish_rename(false);
        let (w, _) = self.size_dip();
        let s = scale(self.hwnd);
        let (r, text) = match item {
            Some(i) => {
                let r = self.item_rect(i, w);
                (rect(r.left, r.top + 10.0 + ICON, r.right, r.top + 10.0 + ICON + 24.0), self.items[i].name.clone())
            }
            None => (rect(8.0, 5.0, w - 8.0, TITLE_H - 5.0), self.name.clone()),
        };
        let mut pt = POINT { x: (r.left * s) as i32, y: (r.top * s) as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let t = shell::wide(&text);
            // 置顶：磁贴可能在所有者链上被系统调到它上面
            let Ok(e) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("EDIT"),
                PCWSTR(t.as_ptr()),
                WS_POPUP | WS_BORDER | WS_VISIBLE | WINDOW_STYLE((ES_AUTOHSCROLL | ES_CENTER) as u32),
                pt.x,
                pt.y,
                ((r.right - r.left) * s) as i32,
                ((r.bottom - r.top) * s) as i32,
                Some(self.hwnd),
                None,
                None,
                None,
            ) else {
                return;
            };
            SendMessageW(e, WM_SETFONT, Some(WPARAM(a.edit_font.0 as usize)), Some(LPARAM(1)));
            // 和资源管理器一样只选中主文件名
            let stem = match text.rfind('.') {
                Some(i) if item.is_some() && i > 0 && !self.items[item.unwrap()].is_dir => text[..i].encode_utf16().count(),
                _ => text.encode_utf16().count(),
            };
            SendMessageW(e, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(stem as isize)));
            let _ = SetWindowSubclass(e, Some(edit_proc), 1, self as *mut Tile as usize);
            let _ = SetForegroundWindow(e);
            let _ = SetFocus(Some(e));
            self.edit = Some((e, item.map(|i| self.items[i].name.clone())));
        }
    }

    fn finish_rename(&mut self, commit: bool) {
        let Some((e, target)) = self.edit.take() else { return };
        let mut buf = [0u16; 260];
        let n = unsafe { GetWindowTextW(e, &mut buf) } as usize;
        unsafe {
            let _ = DestroyWindow(e);
        }
        let text = String::from_utf16_lossy(&buf[..n]).trim().to_string();
        if commit && !text.is_empty() {
            app().rename(self, target, &text);
        }
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        let t = &mut *(data as *mut Tile);
        match msg {
            WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize || wp.0 == VK_ESCAPE.0 as usize => {
                t.finish_rename(wp.0 == VK_RETURN.0 as usize);
                LRESULT(0)
            }
            WM_CHAR if wp.0 == 13 || wp.0 == 27 => LRESULT(0),
            WM_KILLFOCUS => {
                t.finish_rename(true);
                LRESULT(0)
            }
            _ => DefSubclassProc(hwnd, msg, wp, lp),
        }
    }
}

/// 拖动磁贴时吸附到工作区边缘和其他磁贴的边
fn snap(a: &App, me: HWND, r: &mut RECT) {
    unsafe {
        let s = scale(me);
        let (d, gap) = ((12.0 * s) as i32, (8.0 * s) as i32);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromRect(r, MONITOR_DEFAULTTONEAREST), &mut mi);
        let wa = mi.rcWork;
        let (mut xl, mut xr, mut yt, mut yb) = (vec![wa.left], vec![wa.right], vec![wa.top], vec![wa.bottom]);
        for t in a.tiles.iter().filter(|t| t.hwnd != me && IsWindowVisible(t.hwnd).as_bool()) {
            let mut o = RECT::default();
            let _ = GetWindowRect(t.hwnd, &mut o);
            xl.extend([o.right + gap, o.left]);
            xr.extend([o.left - gap, o.right]);
            yt.extend([o.bottom + gap, o.top]);
            yb.extend([o.top - gap, o.bottom]);
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if let Some(&x) = xl.iter().find(|x| (r.left - **x).abs() <= d) {
            (r.left, r.right) = (x, x + w);
        } else if let Some(&x) = xr.iter().find(|x| (r.right - **x).abs() <= d) {
            (r.left, r.right) = (x - w, x);
        }
        if let Some(&y) = yt.iter().find(|y| (r.top - **y).abs() <= d) {
            (r.top, r.bottom) = (y, y + h);
        } else if let Some(&y) = yb.iter().find(|y| (r.bottom - **y).abs() <= d) {
            (r.top, r.bottom) = (y - h, y);
        }
    }
}

pub unsafe extern "system" fn tile_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // 整个窗口都是客户区，边框和标题栏自己画
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_ERASEBKGND => return LRESULT(1),
            _ => {}
        }
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Tile;
        if p.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let t = &mut *p;
        let a = app();
        if let Some(cm) = &a.menu2 {
            if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM) {
                if cm.HandleMenuMsg(msg, wp, lp).is_ok() {
                    return LRESULT(0);
                }
            }
        }
        match msg {
            WM_PAINT => t.paint(a),
            WM_SIZE => {
                if let Some(s) = &t.surf {
                    a.gfx.resize(s, (lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                }
                t.items.iter_mut().for_each(|it| it.layout = None);
                t.clamp_scroll();
                t.invalidate();
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                t.drop_device();
                a.save_rect(t);
            }
            WM_NCHITTEST => return t.nc_hit(lp),
            WM_NCLBUTTONDBLCLK if wp.0 as u32 == HTCAPTION => a.toggle_roll(t),
            WM_NCRBUTTONDOWN if wp.0 as u32 == HTCAPTION => {}
            WM_NCRBUTTONUP if wp.0 as u32 == HTCAPTION => t.tile_menu(a, lparam_pt(lp)),
            WM_WINDOWPOSCHANGING => {
                // 始终贴在桌面这一层，不盖住其他程序
                let pos = &mut *(lp.0 as *mut WINDOWPOS);
                if pos.flags & SWP_NOZORDER == SET_WINDOW_POS_FLAGS(0) {
                    pos.hwndInsertAfter = if a.topmost { HWND_TOPMOST } else { HWND_BOTTOM };
                }
            }
            WM_MOVING => {
                snap(a, hwnd, &mut *(lp.0 as *mut RECT));
                return LRESULT(1);
            }
            WM_EXITSIZEMOVE => a.save_rect(t),
            WM_MOUSEMOVE => t.on_move(a, wp, lp),
            WM_MOUSELEAVE => {
                t.tracking = false;
                t.hover = None;
                t.invalidate();
            }
            WM_LBUTTONDOWN => t.on_down(lp),
            WM_LBUTTONUP => t.on_up(),
            WM_LBUTTONDBLCLK => {
                t.press = None;
                t.open_selected(a);
            }
            WM_RBUTTONUP => t.on_rclick(a, lp),
            WM_MOUSEWHEEL => {
                t.scroll -= (wp.0 >> 16) as i16 as f32 / 120.0 * 48.0;
                t.clamp_scroll();
                t.invalidate();
            }
            WM_KEYDOWN => t.on_key(a, wp.0 as u16),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}
