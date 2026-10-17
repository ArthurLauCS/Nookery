//! 绘图设备：全程序共用一个 D3D11 设备和一个 D2D 上下文；每个磁贴只有一条带透明通道的交换链，
//! 经 DirectComposition 挂到窗口上，背后透出系统亚克力。每个窗口各建 HwndRenderTarget 的话，
//! 每个窗口各有一套 D3D 设备，6 个磁贴实测私有内存 61.6 MB，而且透明背景画不出来。
//!
//! 设备用 WARP（CPU 光栅化）而不是显卡：examples/membench 实测（2026-10-17，本机）硬件设备单项 +47.6 MB
//! 私有内存（显卡驱动），WARP +0.9 MB；2 倍 DPI 下整块磁贴重绘 WARP 0.31~0.34 ms、硬件 0.24~0.28 ms。
//! 磁贴只在内容变化和悬停时重绘，这点差距感觉不到。

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::*;

pub struct Gfx {
    d3d: ID3D11Device,
    dxgi: IDXGIFactory2,
    dcomp: IDCompositionDevice,
    pub dc: ID2D1DeviceContext,
    pub brush: ID2D1SolidColorBrush,
}

#[derive(Clone)]
pub struct Surface {
    swap: IDXGISwapChain1,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

impl Gfx {
    pub fn new(factory: &ID2D1Factory1) -> Result<Gfx> {
        unsafe {
            let mut d3d = None;
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None)?;
            let d3d = d3d.unwrap();
            let dxgi_dev: IDXGIDevice = d3d.cast()?;
            let dc = factory.CreateDevice(&dxgi_dev)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let brush = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            Ok(Gfx { dxgi: CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?, dcomp: DCompositionCreateDevice(&dxgi_dev)?, d3d, dc, brush })
        }
    }

    pub fn surface(&self, hwnd: HWND, w: u32, h: u32) -> Result<Surface> {
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = self.dxgi.CreateSwapChainForComposition(&self.d3d, &desc, None)?;
            let target = self.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = self.dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            self.dcomp.Commit()?;
            Ok(Surface { swap, _target: target, _visual: visual })
        }
    }

    /// 把绘制目标切到这块表面的后台缓冲；之后正常 BeginDraw
    pub fn bind(&self, s: &Surface, dpi: f32) -> Result<()> {
        unsafe {
            let buf: IDXGISurface = s.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bmp = self.dc.CreateBitmapFromDxgiSurface(&buf, Some(&props))?;
            self.dc.SetTarget(&bmp);
            self.dc.SetDpi(dpi, dpi);
            Ok(())
        }
    }

    /// EndDraw 之后调用：解绑并呈现
    pub fn present(&self, s: &Surface) -> Result<()> {
        unsafe {
            self.dc.SetTarget(None);
            s.swap.Present(0, DXGI_PRESENT(0)).ok()
        }
    }

    pub fn resize(&self, s: &Surface, w: u32, h: u32) {
        unsafe {
            self.dc.SetTarget(None);
            let _ = s.swap.ResizeBuffers(0, w.max(1), h.max(1), DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0));
        }
    }
}
