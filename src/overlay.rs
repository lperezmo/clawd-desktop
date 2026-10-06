//! One transparent, click-through, topmost layered window per monitor.
//!
//! Each overlay is only as big as the part of Clawd on its monitor: every
//! update moves and resizes it through `UpdateLayeredWindow`, so a frame costs
//! a few hundred kilobytes of copying at most.

use crate::surfaces::Rect;
use log::warn;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, MA_NOACTIVATE, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("ClawdDesktopOverlay");

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

pub fn register_class() {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

pub struct Overlay {
    pub monitor: Rect,
    hwnd: HWND,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    cap_w: i32,
    cap_h: i32,
    shown: bool,
    last: Option<(Rect, u64)>,
}

impl Overlay {
    /// Creates a hidden overlay for `monitor` able to show up to `cap_w` by
    /// `cap_h` pixels.
    pub fn new(monitor: Rect, cap_w: i32, cap_h: i32) -> Option<Overlay> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                w!("Clawd"),
                WS_POPUP,
                monitor.left,
                monitor.top,
                1,
                1,
                None,
                None,
                Some(GetModuleHandleW(None).unwrap_or_default().into()),
                None,
            )
            .map_err(|e| warn!("overlay window: {e}"))
            .ok()?;
            let dc = CreateCompatibleDC(None);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: cap_w,
                    biHeight: -cap_h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    warn!("overlay bitmap: {e}");
                    let _ = DeleteDC(dc);
                    let _ = DestroyWindow(hwnd);
                    return None;
                }
            };
            let old = SelectObject(dc, bmp.into());
            Some(Overlay {
                monitor,
                hwnd,
                dc,
                bmp,
                old,
                bits: bits as *mut u32,
                cap_w,
                cap_h,
                shown: false,
                last: None,
            })
        }
    }

    /// Shows the part of `sprite` (a `sw` by `sh` premultiplied BGRA image
    /// whose top-left is at `pos` on screen) that falls on this monitor.
    /// `key` identifies the image; an unchanged key and position skips the
    /// update. Returns true when the window was updated.
    pub fn show(&mut self, sprite: &[u32], sw: i32, sh: i32, pos: (i32, i32), key: u64) -> bool {
        let full = Rect::new(pos.0, pos.1, pos.0 + sw, pos.1 + sh);
        let Some(vis) = full.intersect(&self.monitor) else {
            self.hide();
            return false;
        };
        let vis = Rect::new(
            vis.left,
            vis.top,
            vis.right.min(vis.left + self.cap_w),
            vis.bottom.min(vis.top + self.cap_h),
        );
        if self.shown && self.last == Some((vis, key)) {
            return false;
        }
        let (vw, vh) = (vis.width(), vis.height());
        let (ox, oy) = (vis.left - full.left, vis.top - full.top);
        // Copy the visible rows into the DIB's top-left corner.
        unsafe {
            let dib = core::slice::from_raw_parts_mut(self.bits, (self.cap_w * self.cap_h) as usize);
            for y in 0..vh {
                let src = ((oy + y) * sw + ox) as usize;
                let dst = (y * self.cap_w) as usize;
                dib[dst..dst + vw as usize].copy_from_slice(&sprite[src..src + vw as usize]);
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let dst = POINT {
                x: vis.left,
                y: vis.top,
            };
            let size = SIZE { cx: vw, cy: vh };
            let src = POINT { x: 0, y: 0 };
            if let Err(e) = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            ) {
                warn!("UpdateLayeredWindow: {e}");
                return false;
            }
            if !self.shown {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.shown = true;
            }
        }
        self.last = Some((vis, key));
        true
    }

    pub fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
            self.last = None;
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
