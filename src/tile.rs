//! 磁贴窗口：绘制和交互。尺寸常量都是 DIP，绘制时由渲染目标按窗口 DPI 缩放。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::gfx::Surface;
use crate::lang::tr;
use crate::shell::{self, IconJob, Items, Picked};
use crate::theme::{HAIR, RADIUS_CONTROL, RADIUS_PANEL};
use crate::{App, app, fit_work_area, popup};

// 间距都在 4 / 8 的格子上；字号只有两档（12 / 14，1.2 比例），在 main.rs 的字体里
pub const TITLE_H: f32 = 36.0;
pub const PAD: f32 = 8.0;
const CELL_W: f32 = 84.0;
/// 格子里图标上方和两行文件名下方留一样的空：选中框里上下对称；
/// 磁贴高度刚好放下几行时，最后一行下面的空也和标题栏到第一行图标的一样（都是 PAD + CELL_PAD）
const CELL_PAD: f32 = 4.0;
pub const CELL_H: f32 = CELL_PAD + ICON + GAP + LABEL_H + CELL_PAD;
/// 和桌面默认的“中等图标”一样大：图标文件里一般就有这一档，不用缩放
const ICON: f32 = 48.0;
/// 图标下沿到文件名的距离
const GAP: f32 = 8.0;
const LABEL_H: f32 = 32.0;
const RADIUS: f32 = RADIUS_CONTROL;
const EDGE: f32 = 6.0;
const MIN_W: f32 = 120.0;
/// 标题栏右上角的折叠、锁定按钮
const BTN: f32 = 28.0;
const BTN_PAD: f32 = 4.0;

#[derive(Clone, Copy)]
enum Glyph {
    Collapse,
    Expand,
    Lock,
    Unlock,
}

/// 1 px 线图标（圆头），画在按钮矩形正中，12 px 见方；坐标落在半像素上线才清晰
fn draw_glyph(a: &App, g: Glyph, r: &D2D_RECT_F, color: &D2D1_COLOR_F, dy: f32) {
    unsafe {
        let (rt, br) = (&a.gfx.dc, &a.gfx.brush);
        br.SetColor(color);
        let (cx, cy) = (((r.left + r.right) / 2.0).floor() + 0.5, ((r.top + r.bottom) / 2.0).floor() + 0.5 + dy);
        let p = |x: f32, y: f32| Vector2 { X: cx + x, Y: cy + y };
        let line = |a1: Vector2, b1: Vector2| rt.DrawLine(a1, b1, br, HAIR, &a.gfx.stroke);
        match g {
            Glyph::Collapse | Glyph::Expand => {
                let d = if matches!(g, Glyph::Collapse) { 1.0 } else { -1.0 };
                line(p(-4.0, 2.0 * d), p(0.0, -2.0 * d));
                line(p(0.0, -2.0 * d), p(4.0, 2.0 * d));
            }
            Glyph::Lock | Glyph::Unlock => {
                let body = D2D1_ROUNDED_RECT { rect: rect(cx - 5.0, cy - 1.0, cx + 5.0, cy + 5.0), radiusX: 1.5, radiusY: 1.5 };
                rt.DrawRoundedRectangle(&body, br, HAIR, &a.gfx.stroke);
                // 锁梁：左脚 → 半圆 → 右脚；打开时右脚抬起不落回锁身
                let Ok(geo) = a.d2d.CreatePathGeometry() else { return };
                let Ok(sink) = geo.Open() else { return };
                sink.BeginFigure(p(-3.0, -1.0), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLine(p(-3.0, -3.0));
                let arc = D2D1_ARC_SEGMENT {
                    point: p(3.0, -3.0),
                    size: D2D_SIZE_F { width: 3.0, height: 3.0 },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                };
                sink.AddArc(&arc);
                if matches!(g, Glyph::Lock) {
                    sink.AddLine(p(3.0, -1.0));
                } else {
                    sink.AddLine(p(3.0, -4.0));
                }
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                let _ = sink.Close();
                rt.DrawGeometry(&geo, br, HAIR, &a.gfx.stroke);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    None,
    Roll,
    Lock,
}

/// 桌面上的一项：name 是它在桌面上的名字（系统图标是 ::{CLSID}），path 用来取图标、打开、右键
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub mtime: SystemTime,
}

pub struct Item {
    pub name: String,
    pub path: PathBuf,
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
    pub locked: bool,
    pub items: Vec<Item>,
    surf: Option<Surface>,
    scroll: f32,
    hover: Option<usize>,
    /// 鼠标在磁贴上：显示标题栏按钮；全透明时显出淡底色
    inside: bool,
    hot: Btn,
    tracking: bool,
    pub sel: Vec<String>,
    anchor: usize,
    /// 按下左键的位置和项，移动超过拖拽阈值就开始拖出
    press: Option<(POINT, usize)>,
    /// 改名编辑框；Some(文件名) 改文件，None 改磁贴名
    edit: Option<(HWND, Option<String>)>,
    /// 有文件正拖在上面
    pub drop_hot: bool,
    /// 磁贴内拖动排序时的插入位置，画一条竖线
    pub insert: Option<usize>,
    /// 标题文字实际占的横向范围，单击这里进入改名
    name_span: (f32, f32),
    /// 键盘焦点所在项；kbd 为真时画焦点环（用键盘操作过、还没再点鼠标）
    focus: usize,
    kbd: bool,
    /// 按住标题栏拖动：按下时的鼠标屏幕坐标、窗口位置、是否已经拖过阈值、按下时是否在名字上
    moving: Option<(POINT, RECT, bool, bool)>,
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
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

/// 标题栏：右上角两个按钮，折叠在最右、锁定在它左边，任何状态下都不换位置；名字在两侧各让出同样宽度后居中
fn title_layout(w: f32) -> (D2D_RECT_F, D2D_RECT_F, (f32, f32)) {
    let top = (TITLE_H - BTN) / 2.0;
    let roll = rect(w - BTN_PAD - BTN, top, w - BTN_PAD, top + BTN);
    let lock = rect(roll.left - BTN, top, roll.left, top + BTN);
    let side = BTN_PAD + 2.0 * BTN;
    (roll, lock, (side, (w - side).max(side)))
}

/// 格子里文件名的位置：图标下方，左右各缩进 4，高两行
fn label_rect(r: &D2D_RECT_F) -> D2D_RECT_F {
    let top = r.top + CELL_PAD + ICON + GAP;
    rect(r.left + 4.0, top, r.right - 4.0, top + LABEL_H)
}

impl Tile {
    /// 位置和大小由 App::layout 按配置摆
    pub fn create(a: &App, name: &str, rolled: bool, locked: bool) -> Box<Tile> {
        let mut t = Box::new(Tile {
            name: name.into(),
            hwnd: HWND::default(),
            rolled,
            locked,
            items: Vec::new(),
            surf: None,
            scroll: 0.0,
            hover: None,
            inside: false,
            hot: Btn::None,
            tracking: false,
            sel: Vec::new(),
            anchor: 0,
            press: None,
            edit: None,
            drop_hot: false,
            insert: None,
            name_span: (0.0, 0.0),
            focus: 0,
            kbd: false,
            moving: None,
        });
        unsafe {
            let title = shell::wide(name);
            t.hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP | if a.topmost { WS_EX_TOPMOST } else { WINDOW_EX_STYLE(0) },
                crate::TILE_CLASS,
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
                0,
                0,
                0,
                0,
                a.owner,
                None,
                None,
                None,
            )
            .expect("创建磁贴窗口失败");
            SetWindowLongPtrW(t.hwnd, GWLP_USERDATA, &mut *t as *mut Tile as isize);
            crate::drop::register(&mut t);
            t.apply_style(a);
        }
        t
    }

    /// 背景（亚克力 / 半透明 / 全透明）、圆角、深浅色，随透明度档位切换
    pub fn apply_style(&self, a: &App) {
        unsafe {
            use windows::Win32::Graphics::Dwm::*;
            let th = &a.theme;
            let dark = BOOL::from(th.dark);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            // 全透明时关掉 DWM 的边框、阴影和圆角裁剪，边框由我们自己画一圈淡线
            let policy = if th.edge.is_some() { DWMNCRP_DISABLED } else { DWMNCRP_USEWINDOWSTYLE };
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_NCRENDERING_POLICY, &policy as *const _ as _, 4);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
            // 阴影跟着 WS_THICKFRAME 走，关 DWM 非客户区渲染也去不掉：全透明时去掉这个样式。
            // 改尺寸靠 WM_NCHITTEST 返回的边缘码，不依赖这个样式
            let style = GetWindowLongW(self.hwnd, GWL_STYLE) as u32;
            let want = if th.edge.is_some() { style & !WS_THICKFRAME.0 } else { style | WS_THICKFRAME.0 };
            if want != style {
                SetWindowLongW(self.hwnd, GWL_STYLE, want as i32);
                let _ = SetWindowPos(self.hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            }
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
                // 4 = ACCENT_ENABLE_ACRYLICBLURBEHIND，0 = 关闭；tint 是 0xAABBGGRR，染色交给 D2D 画的底色
                let mut accent = Accent { state: if th.blur { 4 } else { 0 }, flags: 0, tint: 0x01000000, anim: 0 };
                let mut d = Data { attr: 19, data: &mut accent, size: size_of::<Accent>() };
                let _ = f(self.hwnd, &mut d);
            }
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// 内容变了才重建列表，图标按（名字，修改时间）复用
    pub fn set_items(&mut self, list: Vec<Entry>) {
        let same = list.len() == self.items.len() && list.iter().zip(&self.items).all(|(e, it)| e.name == it.name && e.mtime == it.mtime);
        if same {
            return;
        }
        let mut old: Vec<Item> = std::mem::take(&mut self.items);
        self.items = list
            .into_iter()
            .map(|Entry { name, path, is_dir, mtime }| match old.iter().position(|o| o.name == name && o.mtime == mtime) {
                Some(i) => old.swap_remove(i),
                None => Item { name, path, is_dir, mtime, icon: None, icon_tried: false, layout: None },
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

    /// 屏幕坐标处的插入位置（0..=项数）：落在格子左半边插到它前面，右半边插到它后面
    pub fn slot_at(&self, x: i32, y: i32) -> usize {
        let mut p = POINT { x, y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        let s = scale(self.hwnd);
        let (w, _) = self.size_dip();
        let (n, cw) = grid(w);
        let col = (((p.x as f32 / s - PAD) / cw + 0.5).max(0.0) as usize).min(n);
        let row = ((p.y as f32 / s - TITLE_H - PAD + self.scroll) / CELL_H).max(0.0) as usize;
        (row * n + col).min(self.items.len())
    }

    fn btn_at(&self, x: f32, y: f32) -> Btn {
        let (w, _) = self.size_dip();
        let (roll, lock, _) = title_layout(w);
        if inside(&roll, x, y) {
            Btn::Roll
        } else if inside(&lock, x, y) {
            Btn::Lock
        } else {
            Btn::None
        }
    }

    fn display_name(a: &App, it: &Item) -> String {
        if it.name.starts_with("::") {
            return shell::display_name(&it.path).unwrap_or_default();
        }
        match it.name.rfind('.') {
            // 快捷方式和网址和桌面上一样总是不显示扩展名
            Some(i) if !it.is_dir && i > 0 && (a.hide_ext || [".lnk", ".url"].iter().any(|x| it.name[i..].eq_ignore_ascii_case(x))) => it.name[..i].to_string(),
            _ => it.name.clone(),
        }
    }

    pub fn drop_device(&mut self) {
        self.surf = None;
        for it in &mut self.items {
            it.icon = None;
            it.icon_tried = false;
        }
    }

    /// 后台取回的图标：项还在（名字和修改时间对得上）就建成位图重画，已换掉的项丢弃
    pub fn set_icon(&mut self, a: &App, job: IconJob) {
        if let Some(it) = self.items.iter_mut().find(|it| it.name == job.name && it.mtime == job.mtime) {
            it.icon = job.pixels.as_ref().and_then(|p| shell::to_bitmap(&a.gfx.dc, p));
            self.invalidate();
        }
    }

    fn paint(&mut self, a: &App) {
        unsafe {
            let _ = ValidateRect(Some(self.hwnd), None);
        }
        // 看得见的项第一次画时交给后台线程取图标，取回来由 set_icon 补上
        let (w, h) = self.size_dip();
        let px = (ICON * scale(self.hwnd)) as i32;
        for i in 0..self.items.len() {
            let r = self.item_rect(i, w);
            let it = &mut self.items[i];
            if !it.icon_tried && r.bottom >= TITLE_H && r.top <= h {
                it.icon_tried = true;
                a.icons.load(IconJob { tile: self.hwnd.0 as usize, name: it.name.clone(), mtime: it.mtime, path: it.path.clone(), px, pixels: None });
            }
        }
        self.draw(a);
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
            // 透明度低时文字直接压在壁纸上：四个方向各描一层暗边，白底上也有自己的轮廓（单层阴影在白色壁纸上看不清）
            let text = |layout: &IDWriteTextLayout, x: f32, y: f32, color: &D2D1_COLOR_F| {
                if th.shadow {
                    br.SetColor(&th.shade);
                    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (1.0, 1.0)] {
                        rt.DrawTextLayout(Vector2 { X: x + dx, Y: y + dy }, layout, br, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                    }
                }
                br.SetColor(color);
                rt.DrawTextLayout(Vector2 { X: x, Y: y }, layout, br, D2D1_DRAW_TEXT_OPTIONS_CLIP);
            };
            rt.BeginDraw();
            rt.Clear(Some(if th.edge.is_some() && (self.inside || self.drop_hot) { &th.peek } else { &th.tint }));
            if let Some(edge) = &th.edge {
                // 外圈淡白、内圈淡黑：深色、浅色壁纸上都看得出一圈不显眼的边
                br.SetColor(edge);
                rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(0.5, 0.5, w - 0.5, h - 0.5), radiusX: RADIUS_PANEL, radiusY: RADIUS_PANEL }, br, HAIR, None);
                br.SetColor(&th.edge_in);
                rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(1.5, 1.5, w - 1.5, h - 1.5), radiusX: RADIUS_PANEL - 1.0, radiusY: RADIUS_PANEL - 1.0 }, br, HAIR, None);
            }

            let (roll, lock, (nl, nr)) = title_layout(w);
            if let Ok(l) = a.dw.CreateTextLayout(&u16s(&self.name), &a.fmt_title, nr - nl, TITLE_H) {
                let mut m = DWRITE_TEXT_METRICS::default();
                let _ = l.GetMetrics(&mut m);
                self.name_span = (nl + m.left, nl + m.left + m.width);
                text(&l, nl, 0.0, &th.text);
            }
            for (btn, r, glyph, shown) in [
                (Btn::Roll, roll, if self.rolled { Glyph::Expand } else { Glyph::Collapse }, self.inside || self.rolled),
                (Btn::Lock, lock, if self.locked { Glyph::Lock } else { Glyph::Unlock }, self.inside || self.locked),
            ] {
                if !shown {
                    continue;
                }
                if self.hot == btn {
                    br.SetColor(&th.hover);
                    rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: RADIUS_CONTROL, radiusY: RADIUS_CONTROL }, br);
                }
                // 壁纸上的图标和文字一样先描一层暗边
                if th.shadow {
                    draw_glyph(a, glyph, &r, &th.shade, 1.0);
                }
                draw_glyph(a, glyph, &r, if self.hot == btn { &th.text } else { &th.muted }, 0.0);
            }

            if !self.rolled {
                if self.items.is_empty() {
                    if let Ok(l) = a.dw.CreateTextLayout(&u16s(tr("拖放文件到这里", "Drop files here")), &a.fmt_hint, w, h - TITLE_H) {
                        text(&l, 0.0, TITLE_H, &th.muted);
                    }
                }
                rt.PushAxisAlignedClip(&rect(0.0, TITLE_H, w, h), D2D1_ANTIALIAS_MODE_ALIASED);
                let s = scale(self.hwnd);
                for i in 0..self.items.len() {
                    let r = self.item_rect(i, w);
                    if r.bottom < TITLE_H || r.top > h {
                        continue;
                    }
                    let cell = D2D1_ROUNDED_RECT { rect: rect(r.left + 2.0, r.top + 2.0, r.right - 2.0, r.bottom - 2.0), radiusX: RADIUS, radiusY: RADIUS };
                    if self.sel.contains(&self.items[i].name) {
                        br.SetColor(&th.sel_fill);
                        rt.FillRoundedRectangle(&cell, br);
                        br.SetColor(&th.sel_line);
                        rt.DrawRoundedRectangle(&cell, br, HAIR, None);
                    } else if self.hover == Some(i) {
                        br.SetColor(&th.hover);
                        rt.FillRoundedRectangle(&cell, br);
                    }
                    if self.press.is_some_and(|p| p.1 == i) {
                        br.SetColor(&th.press);
                        rt.FillRoundedRectangle(&cell, br);
                    }
                    if self.kbd && self.focus == i {
                        // 键盘焦点：1 px 强调色边框加 3 px 淡光环
                        br.SetColor(&th.ring);
                        rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(cell.rect.left - 1.0, cell.rect.top - 1.0, cell.rect.right + 1.0, cell.rect.bottom + 1.0), radiusX: RADIUS + 1.0, radiusY: RADIUS + 1.0 }, br, 3.0, None);
                        br.SetColor(&th.accent);
                        rt.DrawRoundedRectangle(&cell, br, HAIR, None);
                    }
                    let it = &mut self.items[i];
                    if let Some(bmp) = &it.icon {
                        // 按位图原尺寸、对齐到整像素画：落在半像素上会被插值糊掉
                        let px = bmp.GetPixelSize();
                        let (bw, bh) = (px.width as f32, px.height as f32);
                        let x = (((r.left + r.right) * s - bw) / 2.0).round();
                        let y = ((r.top + CELL_PAD) * s + (ICON * s - bh) / 2.0).round();
                        let dst = rect(x / s, y / s, (x + bw) / s, (y + bh) / s);
                        rt.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                    }
                    let lr = label_rect(&r);
                    if it.layout.is_none() {
                        let t = u16s(&Self::display_name(a, it));
                        it.layout = a.dw.CreateTextLayout(&t, &a.fmt_label, lr.right - lr.left, LABEL_H).ok();
                    }
                    if let Some(l) = &it.layout {
                        text(l, lr.left, lr.top, &th.text);
                    }
                }
                if let Some(k) = self.insert {
                    let r = self.item_rect(k.min(self.items.len() - 1), w);
                    let x = if k < self.items.len() { r.left } else { r.right };
                    br.SetColor(&th.sel_line);
                    rt.FillRectangle(&rect(x - 1.0, r.top + 6.0, x + 1.0, r.bottom - 6.0), br);
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
                let edge = D2D1_ROUNDED_RECT { rect: rect(1.0, 1.0, w - 1.0, h - 1.0), radiusX: RADIUS_PANEL - 1.0, radiusY: RADIUS_PANEL - 1.0 };
                br.SetColor(&th.sel_fill);
                rt.FillRoundedRectangle(&edge, br);
                br.SetColor(&th.accent);
                rt.DrawRoundedRectangle(&edge, br, HAIR, None);
            }
            let ok = rt.EndDraw(None, None).is_ok();
            if !(a.gfx.present(&surf).is_ok() && ok) {
                // 显卡驱动重置等：整套设备重建
                app().reset_gfx();
            }
        }
    }

    /// 边缘交给系统改尺寸；标题栏是客户区，单击改名、按住拖动在 on_down 里处理。锁定后不能改尺寸
    fn nc_hit(&self, lp: LPARAM) -> LRESULT {
        if self.locked {
            return LRESULT(HTCLIENT as isize);
        }
        let p = lparam_pt(lp);
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        let e = (EDGE * scale(self.hwnd)) as i32;
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
            _ => HTCLIENT,
        };
        LRESULT(code as isize)
    }

    fn selected_idx(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|i| self.sel.contains(&self.items[*i].name)).collect()
    }

    fn sel_items(&self) -> Option<Items> {
        let paths: Vec<&Path> = self.selected_idx().into_iter().map(|i| self.items[i].path.as_path()).collect();
        Items::new(self.hwnd, &paths)
    }

    /// 标题栏上按下：按钮直接生效；名字上单击改名；按住拖动移动磁贴（自己捕获鼠标处理，见 drag_to）
    fn on_title_down(&mut self, a: &mut App, x: f32, y: f32, lp: LPARAM) {
        match self.btn_at(x, y) {
            Btn::Roll => return a.toggle_roll(self),
            Btn::Lock => return a.toggle_lock(self),
            Btn::None => {}
        }
        let on_name = x >= self.name_span.0 - 4.0 && x <= self.name_span.1 + 4.0;
        if self.locked {
            if on_name {
                self.start_rename(a, None);
            }
            return;
        }
        // 不借用系统移动循环（WM_NCLBUTTONDOWN + HTCAPTION）：2026-10-17 实际用下来拖不动
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut r);
            self.moving = Some((self.screen_pt(lp), r, false, on_name));
            SetCapture(self.hwnd);
        }
    }

    fn screen_pt(&self, lp: LPARAM) -> POINT {
        let mut pt = lparam_pt(lp);
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        pt
    }

    /// 拖动中：超过系统拖拽阈值后按位移挪窗口，吸附规则和改尺寸一致（按住 Alt 不吸附）；整块磁贴不出工作区
    fn drag_to(&mut self, a: &App, lp: LPARAM) {
        let p = self.screen_pt(lp);
        let Some((p0, r0, moved, _)) = self.moving.as_mut() else { return };
        let (dx, dy) = (p.x - p0.x, p.y - p0.y);
        let (tx, ty) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
        if !*moved && dx.abs() <= tx && dy.abs() <= ty {
            return;
        }
        *moved = true;
        let mut r = RECT { left: r0.left + dx, top: r0.top + dy, right: r0.right + dx, bottom: r0.bottom + dy };
        snap(a, self.hwnd, &mut r, None);
        let (x, y) = fit_work_area(&r);
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// 松开：拖过就保存位置；没拖、按下时在名字上就进入改名
    fn end_drag(&mut self, a: &mut App) {
        let Some((_, _, moved, on_name)) = self.moving.take() else { return };
        unsafe {
            let _ = ReleaseCapture();
        }
        if moved {
            a.save_rect(self);
        } else if on_name {
            self.start_rename(a, None);
        }
    }

    fn on_down(&mut self, a: &mut App, lp: LPARAM) {
        let p = lparam_pt(lp);
        let s = scale(self.hwnd);
        let (x, y) = (p.x as f32 / s, p.y as f32 / s);
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        self.kbd = false;
        if y < TITLE_H {
            return self.on_title_down(a, x, y, lp);
        }
        match self.hit(x, y) {
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
                self.focus = i;
            }
            None => self.sel.clear(),
        }
        self.invalidate();
    }

    fn on_move(&mut self, a: &App, wp: WPARAM, lp: LPARAM) {
        if self.moving.is_some() {
            return self.drag_to(a, lp);
        }
        let p = lparam_pt(lp);
        let s = scale(self.hwnd);
        let (x, y) = (p.x as f32 / s, p.y as f32 / s);
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            self.tracking = unsafe { TrackMouseEvent(&mut tme).is_ok() };
        }
        let (hover, hot) = (self.hit(x, y), self.btn_at(x, y));
        if hover != self.hover || hot != self.hot || !self.inside {
            (self.hover, self.hot, self.inside) = (hover, hot, true);
            self.invalidate();
        }
        if let Some((p0, _)) = self.press {
            let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if wp.0 as u32 & MK_LBUTTON.0 == 0 {
                self.press = None;
            } else if (p.x - p0.x).abs() > dx || (p.y - p0.y).abs() > dy {
                self.press = None;
                if let Some(items) = self.sel_items() {
                    items.drag(self.hwnd);
                }
            }
        }
    }

    fn on_up(&mut self, a: &mut App) {
        if self.moving.is_some() {
            return self.end_drag(a);
        }
        // 在多选中的某项上单击（没拖动），松开后只选它
        if let Some((_, i)) = self.press.take() {
            if !key_down(VK_CONTROL) && !key_down(VK_SHIFT) {
                self.sel = vec![self.items[i].name.clone()];
            }
            self.invalidate();
        }
    }

    fn open_selected(&self) {
        for i in self.selected_idx() {
            shell::open(&self.items[i].path);
        }
    }

    fn verb(&self, verb: &str) {
        if let Some(cm) = self.sel_items().and_then(|it| it.menu(self.hwnd)) {
            shell::invoke_verb(&cm, self.hwnd, verb, key_down(VK_SHIFT));
        }
    }

    /// 粘贴到桌面目录，剪贴板里的这些名字记到本磁贴
    fn paste(&self, a: &mut App) {
        let names = shell::clipboard_names();
        if let Some(cm) = shell::background_menu(self.hwnd, &a.root) {
            shell::invoke_verb(&cm, self.hwnd, "paste", false);
        }
        for n in &names {
            a.cfg.set_map(n, &self.name);
        }
        a.save();
    }

    /// 方向键像资源管理器一样移动选中项，并把它滚进可视区
    fn move_focus(&mut self, vk: VIRTUAL_KEY) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let (w, h) = self.size_dip();
        let (cols, _) = grid(w);
        let f = self.focus.min(n - 1);
        self.focus = match vk {
            VK_LEFT => f.saturating_sub(1),
            VK_RIGHT => (f + 1).min(n - 1),
            VK_UP => f.saturating_sub(cols),
            VK_DOWN => (f + cols).min(n - 1),
            VK_HOME => 0,
            _ => n - 1,
        };
        self.kbd = true;
        self.anchor = self.focus;
        self.sel = vec![self.items[self.focus].name.clone()];
        let row = (self.focus / cols) as f32 * CELL_H;
        let view = h - TITLE_H - 2.0 * PAD;
        // 不用 clamp：磁贴折叠时 view 为负，clamp 的上下界会颠倒而 panic
        self.scroll = self.scroll.min(row).max(row + CELL_H - view);
        self.clamp_scroll();
        self.invalidate();
    }

    fn on_key(&mut self, a: &mut App, vk: u16) {
        let ctrl = key_down(VK_CONTROL);
        match VIRTUAL_KEY(vk) {
            VK_F2 => {
                if let Some(&i) = self.selected_idx().first() {
                    self.start_rename(a, Some(i));
                }
            }
            VK_DELETE => self.verb("delete"),
            VK_RETURN => self.open_selected(),
            VK_F5 => a.rescan(),
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END => self.move_focus(VIRTUAL_KEY(vk)),
            VK_A if ctrl => {
                self.sel = self.items.iter().map(|it| it.name.clone()).collect();
                self.invalidate();
            }
            VK_C if ctrl => self.verb("copy"),
            VK_X if ctrl => self.verb("cut"),
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
        let Some(cm) = self.sel_items().and_then(|it| it.menu(self.hwnd)) else { return };
        if let Picked::Rename = shell::track_menu(&cm, self.hwnd, screen, &mut a.menu2) {
            self.start_rename(a, Some(i));
        }
    }

    /// 标题栏或空白处右键：只留打开所在文件夹和删除磁贴
    fn tile_menu(&mut self, a: &mut App, pt: POINT) {
        match popup(self.hwnd, pt, &[(3, tr("在资源管理器中打开", "Open in File Explorer")), (0, ""), (5, tr("删除磁贴", "Delete tile"))]) {
            3 => shell::open(&a.root),
            5 => {
                let msg = shell::wide(&if crate::lang::en() {
                    format!("Delete tile \"{}\"? Its items will be sorted into other tiles by the rules. Files are not touched.", self.name)
                } else {
                    format!("删除磁贴「{}」？里面的项会按规则分到其他磁贴，文件不动。", self.name)
                });
                let ok = unsafe { MessageBoxW(Some(self.hwnd), PCWSTR(msg.as_ptr()), w!("Nookery"), MB_YESNO | MB_ICONQUESTION) } == IDYES;
                if ok {
                    // 磁贴会被销毁，之后不能再碰 self
                    let name = self.name.clone();
                    a.delete_tile(&name);
                }
            }
            _ => {}
        }
    }

    /// 改名框：无边框、颜色跟主题（WM_CTLCOLOREDIT 里设）；点别处失去焦点即保存
    pub fn start_rename(&mut self, a: &App, item: Option<usize>) {
        self.finish_rename(false);
        // 系统图标不改名
        if item.is_some_and(|i| self.items[i].name.starts_with("::")) {
            return;
        }
        let (w, _) = self.size_dip();
        let s = scale(self.hwnd);
        let (r, text, font) = match item {
            Some(i) => {
                // 盖住整块文件名：同字体、同左右宽度，单行编辑框的字贴着框顶画，和文件名第一行基线重合
                (label_rect(&self.item_rect(i, w)), self.items[i].name.clone(), a.edit_font)
            }
            None => {
                // 盖在标题上：同字体、同左右范围、一样居中；框顶是标题字的上沿，框在标题栏里上下对称
                let (_, _, (nl, nr)) = title_layout(w);
                (rect(nl, a.title_top, nr, TITLE_H - a.title_top), self.name.clone(), a.title_font)
            }
        };
        // D2D 画字时基线对齐到整像素，框的位置也四舍五入，字才落在同一行
        let mut pt = POINT { x: (r.left * s).round() as i32, y: (r.top * s).round() as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let t = shell::wide(&text);
            // 置顶：磁贴可能在所有者链上被系统调到它上面
            let Ok(e) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("EDIT"),
                PCWSTR(t.as_ptr()),
                WS_POPUP | WS_VISIBLE | WINDOW_STYLE((ES_AUTOHSCROLL | ES_CENTER) as u32),
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
            // 先记下正在改名，再抢焦点：抢焦点会触发磁贴的 z 序变化，WM_WINDOWPOSCHANGING 要据此不去盖住改名框
            self.edit = Some((e, item.map(|i| self.items[i].name.clone())));
            SendMessageW(e, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            // 和资源管理器一样只选中主文件名
            let stem = match text.rfind('.') {
                Some(i) if item.is_some() && i > 0 && !self.items[item.unwrap()].is_dir => text[..i].encode_utf16().count(),
                _ => text.encode_utf16().count(),
            };
            SendMessageW(e, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(stem as isize)));
            let _ = SetWindowSubclass(e, Some(edit_proc), 1, self as *mut Tile as usize);
            let _ = SetForegroundWindow(e);
            let _ = SetFocus(Some(e));
            let _ = SetWindowPos(e, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
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

/// 拖动或拖边缘改尺寸时，靠近工作区边缘和其他磁贴的边 8px 内吸附，按住 Alt 不吸附（自由摆放）。
/// edge 是 WM_SIZING 的 WMSZ_*，None 表示整体移动
fn snap(a: &App, me: HWND, r: &mut RECT, edge: Option<u32>) {
    if key_down(VK_MENU) {
        return;
    }
    unsafe {
        let s = scale(me);
        let (d, gap) = ((8.0 * s) as i32, (8.0 * s) as i32);
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
        let near = |v: i32, c: &[i32]| c.iter().copied().find(|x| (v - x).abs() <= d);
        let Some(e) = edge else {
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if let Some(x) = near(r.left, &xl) {
                (r.left, r.right) = (x, x + w);
            } else if let Some(x) = near(r.right, &xr) {
                (r.left, r.right) = (x - w, x);
            }
            if let Some(y) = near(r.top, &yt) {
                (r.top, r.bottom) = (y, y + h);
            } else if let Some(y) = near(r.bottom, &yb) {
                (r.top, r.bottom) = (y - h, y);
            }
            return;
        };
        // 改尺寸：只吸附正在拖的那几条边
        if matches!(e, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
            r.left = near(r.left, &xl).unwrap_or(r.left);
        }
        if matches!(e, WMSZ_RIGHT | WMSZ_TOPRIGHT | WMSZ_BOTTOMRIGHT) {
            r.right = near(r.right, &xr).unwrap_or(r.right);
        }
        if matches!(e, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
            r.top = near(r.top, &yt).unwrap_or(r.top);
        }
        if matches!(e, WMSZ_BOTTOM | WMSZ_BOTTOMLEFT | WMSZ_BOTTOMRIGHT) {
            r.bottom = near(r.bottom, &yb).unwrap_or(r.bottom);
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
                // 按着左键是用户正把磁贴拖到 / 拉到另一块显示器上：用系统建议的矩形保持看上去一样大，松手时存盘；
                // 否则是改了缩放比例，或程序自己把磁贴挪到了别的显示器：按配置里的比例重新摆
                if key_down(VK_LBUTTON) {
                    let r = &*(lp.0 as *const RECT);
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                } else {
                    a.layout();
                }
                t.drop_device();
            }
            WM_NCHITTEST => return t.nc_hit(lp),
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp.0 as *mut MINMAXINFO);
                let s = scale(hwnd);
                mm.ptMinTrackSize.x = (MIN_W * s) as i32;
                mm.ptMinTrackSize.y = ((if t.rolled { TITLE_H } else { TITLE_H + CELL_H + 2.0 * PAD }) * s) as i32;
            }
            WM_KILLFOCUS => {
                t.kbd = false;
                t.invalidate();
            }
            // 改名框（以磁贴为所有者的弹出式 Edit）的配色
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as _);
                SetTextColor(dc, a.theme.edit_fg);
                SetBkColor(dc, a.theme.edit_bg);
                return LRESULT(a.edit_brush.0 as isize);
            }
            WM_WINDOWPOSCHANGING => {
                // 始终贴在桌面这一层，不盖住其他程序；改名期间不动，免得盖住改名框
                let pos = &mut *(lp.0 as *mut WINDOWPOS);
                if pos.flags & SWP_NOZORDER == SET_WINDOW_POS_FLAGS(0) && t.edit.is_none() {
                    pos.hwndInsertAfter = if a.topmost { HWND_TOPMOST } else { HWND_BOTTOM };
                }
            }
            WM_MOVING => {
                snap(a, hwnd, &mut *(lp.0 as *mut RECT), None);
                return LRESULT(1);
            }
            WM_SIZING => {
                snap(a, hwnd, &mut *(lp.0 as *mut RECT), Some(wp.0 as u32));
                return LRESULT(1);
            }
            WM_EXITSIZEMOVE => a.save_rect(t),
            WM_MOUSEMOVE => t.on_move(a, wp, lp),
            WM_MOUSELEAVE => {
                (t.tracking, t.hover, t.hot, t.inside) = (false, None, Btn::None, false);
                t.invalidate();
            }
            WM_LBUTTONDOWN => t.on_down(a, lp),
            WM_LBUTTONUP => t.on_up(a),
            // 拖动中鼠标被别的窗口抢走（弹窗、Alt+Tab）：当作松开
            WM_CAPTURECHANGED if t.moving.is_some() && lp.0 != hwnd.0 as isize => t.end_drag(a),
            WM_LBUTTONDBLCLK => {
                // 标题栏上的双击当成两次单击（按钮、改名），不打开文件
                if (lparam_pt(lp).y as f32) < TITLE_H * scale(hwnd) {
                    t.on_down(a, lp);
                } else {
                    t.press = None;
                    t.open_selected();
                }
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
