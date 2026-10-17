//! 逐步初始化，打印每一步之后的私有内存，找常驻内存花在哪：cargo run --release --example membench

use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::*;
use windows::core::*;

fn private_mb() -> f64 {
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as _, size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32);
    }
    c.PrivateUsage as f64 / 1048576.0
}

fn step(name: &str, last: &mut f64) {
    let now = private_mb();
    println!("{name:<28} {now:6.1} MB  (+{:.1})", now - *last);
    *last = now;
}

fn main() {
    let hw = !std::env::args().any(|a| a == "--warp");
    let mut last = 0.0;
    step("启动", &mut last);
    unsafe {
        let _ = OleInitialize(None);
        step("OleInitialize", &mut last);
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let _f = dw.CreateTextFormat(w!("Segoe UI"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 12.0, w!("zh-CN")).unwrap();
        step("DirectWrite + 字体", &mut last);
        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
        step("D2D 工厂", &mut last);
        let mut d3d = None;
        let ty = if hw { D3D_DRIVER_TYPE_HARDWARE } else { D3D_DRIVER_TYPE_WARP };
        D3D11CreateDevice(None, ty, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None).unwrap();
        let d3d = d3d.unwrap();
        step(if hw { "D3D11 硬件设备" } else { "D3D11 WARP 设备" }, &mut last);
        let dxgi: IDXGIDevice = d3d.cast().unwrap();
        let dc = d2d.CreateDevice(&dxgi).unwrap().CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).unwrap();
        step("D2D 设备上下文", &mut last);
        draw_bench(&dc, &_f, &dw);
        let _dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).unwrap();
        step("DirectComposition", &mut last);
        let p = desktop();
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(p.as_ptr()), None).unwrap();
        let _ = f.GetImage(windows::Win32::Foundation::SIZE { cx: 48, cy: 48 }, SIIGBF_RESIZETOFIT);
        step("Shell 取一个图标", &mut last);
    }
}

/// 按 2 倍 DPI 画一块 340x232 的磁贴：24 个圆角格子 + 24 段两行文字，贴近悬停时的一次整块重绘
unsafe fn draw_bench(dc: &ID2D1DeviceContext, f: &IDWriteTextFormat, dw: &IDWriteFactory) {
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    unsafe {
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 192.0,
            dpiY: 192.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let target = dc.CreateBitmap(D2D_SIZE_U { width: 680, height: 464 }, None, 0, &props).unwrap();
        dc.SetTarget(&target);
        dc.SetDpi(192.0, 192.0);
        let br = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.5 }, None).unwrap();
        let text: Vec<u16> = "一个名字特别长的项目需求说明文档.pdf".encode_utf16().collect();
        let layouts: Vec<IDWriteTextLayout> = (0..24).map(|_| dw.CreateTextLayout(&text, f, 76.0, 32.0).unwrap()).collect();
        let n = 200;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            dc.BeginDraw();
            dc.Clear(Some(&D2D1_COLOR_F { r: 0.1, g: 0.1, b: 0.1, a: 0.35 }));
            for (i, l) in layouts.iter().enumerate() {
                let (x, y) = (8.0 + (i % 4) as f32 * 81.0, 42.0 + (i / 4) as f32 * 86.0);
                let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + 79.0, bottom: y + 84.0 }, radiusX: 6.0, radiusY: 6.0 };
                dc.FillRoundedRectangle(&rr, &br);
                dc.DrawTextLayout(windows_numerics::Vector2 { X: x + 4.0, Y: y + 54.0 }, l, &br, D2D1_DRAW_TEXT_OPTIONS_CLIP);
            }
            dc.EndDraw(None, None).unwrap();
        }
        // 读回一个像素，确保 GPU 真的画完了再计时
        let readback = D2D1_BITMAP_PROPERTIES1 { bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW, ..props };
        let cpu = dc.CreateBitmap(D2D_SIZE_U { width: 680, height: 464 }, None, 0, &readback).unwrap();
        cpu.CopyFromBitmap(None, &target, None).unwrap();
        let _ = cpu.Map(D2D1_MAP_OPTIONS_READ).unwrap();
        println!("{:<28} {:6.3} ms/次", "整块磁贴重绘", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        dc.SetTarget(None);
    }
}

fn desktop() -> Vec<u16> {
    "C:\\Windows\\notepad.exe".encode_utf16().chain(Some(0)).collect()
}
