//! 测试驱动：不动真实鼠标，直接给磁贴窗口发消息。坐标是客户区像素。
//! 用法：drive dblclk <屏幕x> <屏幕y> | click <磁贴> <x> <y> | hover <磁贴> <x> <y> | key <磁贴> <vk> | rename <磁贴> <新名字> | roll <磁贴>

use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn tile(name: &str) -> HWND {
    let n = wide(name);
    unsafe { FindWindowW(w!("NookeryTile"), PCWSTR(n.as_ptr())).expect("找不到磁贴") }
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
    if a[1] == "dblclk" {
        // 模拟在 (x, y) 双击桌面空白处：直接发给主窗口，wparam=1 让它跳过空白处判断
        let main = unsafe { FindWindowW(w!("NookeryMain"), PCWSTR::null()).expect("找不到主窗口") };
        let (x, y): (isize, isize) = (a[2].parse().unwrap(), a[3].parse().unwrap());
        unsafe {
            let _ = PostMessageW(Some(main), WM_APP + 3, WPARAM(1), LPARAM((y << 32) | (x as u32 as isize)));
        }
        return;
    }
    let h = tile(&a[2]);
    let xy = || (a[4].parse::<isize>().unwrap() << 16) | (a[3].parse::<isize>().unwrap() & 0xFFFF);
    match a[1].as_str() {
        "hover" => send(h, WM_MOUSEMOVE, 0, xy()),
        "click" => {
            send(h, WM_LBUTTONDOWN, 1, xy());
            send(h, WM_LBUTTONUP, 0, xy());
        }
        "down" => send(h, WM_LBUTTONDOWN, 1, xy()),
        // 按住左键移动（wparam = MK_LBUTTON）
        "drag" => send(h, WM_MOUSEMOVE, 1, xy()),
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
        "editinfo" => unsafe {
            // 打印改名框的位置、可见性，以及在 z 序上是否压在磁贴上面
            let mut e = None;
            loop {
                e = Some(FindWindowExW(None, e, w!("Edit"), PCWSTR::null()).expect("找不到改名框"));
                if GetWindow(e.unwrap(), GW_OWNER) == Ok(h) {
                    break;
                }
            }
            let e = e.unwrap();
            let (mut re, mut rt) = (RECT::default(), RECT::default());
            let _ = GetWindowRect(e, &mut re);
            let _ = GetWindowRect(h, &mut rt);
            let mut above = false;
            let mut w = GetWindow(h, GW_HWNDPREV).ok();
            while let Some(x) = w {
                if x == e {
                    above = true;
                    break;
                }
                w = GetWindow(x, GW_HWNDPREV).ok();
            }
            let ex = GetWindowLongW(e, GWL_EXSTYLE);
            println!("edit {re:?} visible={} above_tile={above} exstyle={ex:#x}; tile {rt:?}", IsWindowVisible(e).as_bool());
        },
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
