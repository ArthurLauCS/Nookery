//! 测试驱动：不动真实鼠标，直接给磁贴窗口发消息。坐标是客户区像素。
//! 用法：drive click <磁贴> <x> <y> | hover <磁贴> <x> <y> | key <磁贴> <vk> | rename <磁贴> <新名字> | roll <磁贴>

use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn tile(name: &str) -> HWND {
    let n = wide(name);
    unsafe { FindWindowW(w!("DeskTilesTile"), PCWSTR(n.as_ptr())).expect("找不到磁贴") }
}

fn send(h: HWND, m: u32, wp: usize, lp: isize) {
    unsafe {
        SendMessageW(h, m, Some(WPARAM(wp)), Some(LPARAM(lp)));
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "moves" {
        // 位移为 0 的鼠标移动：光标不动，但全局鼠标钩子每次都会被调用，用来测钩子开销
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } } };
        let n: u32 = a[2].parse().unwrap();
        let t0 = std::time::Instant::now();
        let sent: u32 = (0..n).map(|_| unsafe { SendInput(&[input], size_of::<INPUT>() as i32) }).sum();
        println!("sent {sent}/{n} in {:?}", t0.elapsed());
        return;
    }
    let h = tile(&a[2]);
    let xy = || (a[4].parse::<isize>().unwrap() << 16) | a[3].parse::<isize>().unwrap();
    match a[1].as_str() {
        "hover" => send(h, WM_MOUSEMOVE, 0, xy()),
        "click" => {
            send(h, WM_LBUTTONDOWN, 1, xy());
            send(h, WM_LBUTTONUP, 0, xy());
        }
        "down" => send(h, WM_LBUTTONDOWN, 1, xy()),
        "up" => send(h, WM_LBUTTONUP, 0, xy()),
        "size" => unsafe {
            let _ = SetWindowPos(h, None, 0, 0, a[3].parse().unwrap(), a[4].parse().unwrap(), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        },
        "rect" => unsafe {
            let v: Vec<i32> = a[3..7].iter().map(|s| s.parse().unwrap()).collect();
            let _ = SetWindowPos(h, None, v[0], v[1], v[2], v[3], SWP_NOZORDER | SWP_NOACTIVATE);
        },
        "key" => send(h, WM_KEYDOWN, a[3].parse().unwrap(), 0),
        "roll" => send(h, WM_NCLBUTTONDBLCLK, HTCAPTION as usize, 0),
        "rename" => unsafe {
            // 改名框是磁贴拥有的弹出式 Edit
            let mut e = None;
            loop {
                e = Some(FindWindowExW(None, e, w!("Edit"), PCWSTR::null()).expect("找不到改名框"));
                if GetWindow(e.unwrap(), GW_OWNER) == Ok(h) {
                    break;
                }
            }
            let e = e.unwrap();
            let t = wide(&a[3]);
            SendMessageW(e, WM_SETTEXT, None, Some(LPARAM(t.as_ptr() as isize)));
            send(e, WM_KEYDOWN, 0x0D, 0);
        },
        _ => panic!("未知命令"),
    }
}
