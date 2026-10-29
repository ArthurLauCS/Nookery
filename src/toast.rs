//! 桌面上的短暂提示：小浮层，停留 1.4 秒后用 8 帧淡出，之后隐藏复用。只在淡出期间重绘。

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::gfx::Surface;
use crate::theme::{HAIR, RADIUS_PANEL};
use crate::{App, app};

pub const TOAST_CLASS: PCWSTR = w!("NookeryToast");
const T_HOLD: usize = 1;
const T_FADE: usize = 2;
const H: f32 = 36.0;
const PAD_X: f32 = 16.0;

pub struct Toast {
    hwnd: HWND,
    surf: Option<Surface>,
    layout: Option<IDWriteTextLayout>,
    alpha: f32,
}

/// 在 at（屏幕坐标）上方弹出提示
pub fn show(a: &mut App, text: &str, at: POINT) {
    unsafe {
        if a.toast.is_none() {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                TOAST_CLASS,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            ) else {
                return;
            };
            a.toast = Some(Toast { hwnd, surf: None, layout: None, alpha: 0.0 });
        }
        let (dw, fmt) = (a.dw.clone(), a.fmt_toast.clone());
        let Some(t) = a.toast.as_mut() else { return };
        let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let w16: Vec<u16> = text.encode_utf16().collect();
        let Ok(layout) = dw.CreateTextLayout(&w16, &fmt, 800.0, H) else { return };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);
        let (w, h) = ((m.width + 2.0 * PAD_X) * s, H * s);
        let _ = layout.SetMaxWidth(m.width + 2.0 * PAD_X);
        t.layout = Some(layout);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let x = (at.x - w as i32 / 2).clamp(wa.left, (wa.right - w as i32).max(wa.left));
        let y = (at.y - h as i32 - (24.0 * s) as i32).clamp(wa.top, (wa.bottom - h as i32).max(wa.top));
        t.surf = None;
        t.alpha = 1.0;
        let _ = SetWindowPos(t.hwnd, Some(HWND_TOPMOST), x, y, w as i32, h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = KillTimer(Some(t.hwnd), T_FADE);
        SetTimer(Some(t.hwnd), T_HOLD, 1400, None);
        let _ = InvalidateRect(Some(t.hwnd), None, false);
    }
}

fn draw(a: &App, t: &mut Toast) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(t.hwnd, &mut r);
        if t.surf.is_none() {
            t.surf = a.gfx.surface(t.hwnd, r.right as u32, r.bottom as u32).ok();
        }
        let (Some(surf), Some(layout)) = (t.surf.clone(), t.layout.clone()) else { return };
        let dpi = GetDpiForWindow(t.hwnd) as f32;
        if a.gfx.bind(&surf, dpi).is_err() {
            return;
        }
        let (rt, br) = (&a.gfx.dc, &a.gfx.brush);
        let (w, h) = (r.right as f32 * 96.0 / dpi, r.bottom as f32 * 96.0 / dpi);
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        // 浮层：bg 底、1 px border、圆角 8，随磁贴的深浅色
        let th = &a.theme;
        let fade = |c: &D2D1_COLOR_F| D2D1_COLOR_F { a: c.a * t.alpha, ..*c };
        let pop = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: 0.5, top: 0.5, right: w - 0.5, bottom: h - 0.5 }, radiusX: RADIUS_PANEL, radiusY: RADIUS_PANEL };
        br.SetColor(&fade(&th.pop_bg));
        rt.FillRoundedRectangle(&pop, br);
        br.SetColor(&fade(&th.pop_line));
        rt.DrawRoundedRectangle(&pop, br, HAIR, None);
        br.SetColor(&fade(&th.pop_text));
        rt.DrawTextLayout(Vector2 { X: 0.0, Y: 0.0 }, &layout, br, D2D1_DRAW_TEXT_OPTIONS_NONE);
        let _ = rt.EndDraw(None, None);
        let _ = a.gfx.present(&surf);
        let _ = ValidateRect(Some(t.hwnd), None);
    }
}

pub unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let a = app();
        let Some(t) = a.toast.as_mut().filter(|t| t.hwnd == hwnd) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_PAINT => draw(app(), t),
            WM_TIMER if wp.0 == T_HOLD => {
                let _ = KillTimer(Some(hwnd), T_HOLD);
                SetTimer(Some(hwnd), T_FADE, 25, None);
            }
            WM_TIMER if wp.0 == T_FADE => {
                t.alpha -= 0.125;
                if t.alpha <= 0.0 {
                    let _ = KillTimer(Some(hwnd), T_FADE);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                } else {
                    draw(app(), t);
                }
            }
            // 不抢焦点、不挡鼠标
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}
