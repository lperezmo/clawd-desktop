//! One transparent, topmost layered window per Clawd per monitor.
//!
//! Each overlay is only as big as the part of Clawd on its monitor: every
//! update moves and resizes it through `UpdateLayeredWindow`, so a frame costs
//! a few hundred kilobytes of copying at most.
//!
//! Clickable off, every overlay is `WS_EX_TRANSPARENT`: the mouse goes
//! straight through. Clickable on, that style comes off and each overlay's
//! window region is set to the opaque pixels of the frame it shows, rebuilt
//! whenever the frame changes; outside the region the mouse goes to whatever
//! is underneath, as before.

use crate::hit::opaque_rects;
use crate::surfaces::Rect;
use log::warn;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, ExtCreateRegion, HBITMAP, HDC, HGDIOBJ,
    HRGN, RDH_RECTANGLES, RGNDATA, RGNDATAHEADER, SelectObject, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, HTCLIENT,
    HTTRANSPARENT, MA_NOACTIVATE, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("ClawdDesktopOverlay");

/// What the mouse did on an overlay, with the cursor in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Down(isize, (i32, i32)),
    Move(isize, (i32, i32)),
    Up(isize, (i32, i32)),
    Double(isize, (i32, i32)),
    Right(isize, (i32, i32)),
    /// The capture was taken away mid-press.
    Lost(isize),
}

thread_local! {
    static CLICKABLE: Cell<bool> = const { Cell::new(false) };
    static MOUSE: RefCell<Vec<Mouse>> = const { RefCell::new(Vec::new()) };
}

/// Whether overlays take the mouse on his pixels; applies to new overlays.
/// Existing ones change through `Overlay::set_clickable`.
pub fn set_clickable_default(on: bool) {
    CLICKABLE.set(on);
}

/// The mouse events since the last call.
pub fn take_mouse() -> Vec<Mouse> {
    MOUSE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let id = h.0 as isize;
    let push = |m: Mouse| MOUSE.with(|q| q.borrow_mut().push(m));
    match msg {
        // The window region already limits hits to his pixels.
        WM_NCHITTEST if CLICKABLE.get() => LRESULT(HTCLIENT as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            unsafe { SetCapture(h) };
            push(Mouse::Down(id, cursor()));
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            unsafe { SetCapture(h) };
            push(Mouse::Double(id, cursor()));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            push(Mouse::Move(id, cursor()));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            push(Mouse::Up(id, cursor()));
            unsafe {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            push(Mouse::Lost(id));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            push(Mouse::Right(id, cursor()));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// A region of `rects` (window coordinates), or None for no rectangles.
fn region(rects: &[Rect]) -> Option<HRGN> {
    if rects.is_empty() {
        return None;
    }
    // The header is 32 bytes, two RECTs' worth, so a Vec<RECT> keeps it aligned.
    let mut buf = vec![RECT::default(); 2 + rects.len()];
    let bound = rects.iter().fold(rects[0], |a, r| {
        Rect::new(
            a.left.min(r.left),
            a.top.min(r.top),
            a.right.max(r.right),
            a.bottom.max(r.bottom),
        )
    });
    for (d, r) in buf[2..].iter_mut().zip(rects) {
        *d = RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
    }
    let header = RGNDATAHEADER {
        dwSize: size_of::<RGNDATAHEADER>() as u32,
        iType: RDH_RECTANGLES,
        nCount: rects.len() as u32,
        nRgnSize: (rects.len() * size_of::<RECT>()) as u32,
        rcBound: RECT {
            left: bound.left,
            top: bound.top,
            right: bound.right,
            bottom: bound.bottom,
        },
    };
    unsafe {
        std::ptr::write(buf.as_mut_ptr() as *mut RGNDATAHEADER, header);
        let r = ExtCreateRegion(
            None,
            (buf.len() * size_of::<RECT>()) as u32,
            buf.as_ptr() as *const RGNDATA,
        );
        (!r.is_invalid()).then_some(r)
    }
}

pub fn register_class() {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
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
    /// Whether this window takes the mouse on his pixels.
    clickable: bool,
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
            let mut o = Overlay {
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
                clickable: false,
            };
            o.set_clickable(CLICKABLE.get());
            Some(o)
        }
    }

    pub fn id(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Clickable on: drops `WS_EX_TRANSPARENT` and limits hits to the current
    /// frame's opaque pixels. Off: click-through everywhere, no region.
    pub fn set_clickable(&mut self, on: bool) {
        if on == self.clickable {
            return;
        }
        self.clickable = on;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let t = WS_EX_TRANSPARENT.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { ex & !t } else { ex | t });
            if !on {
                SetWindowRgn(self.hwnd, None, false);
            }
        }
        // Rebuild the region with the next frame.
        self.last = None;
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
            if self.clickable {
                // Window coordinates: the DIB's top-left is the window's.
                let rects = opaque_rects(dib, self.cap_w as usize, vw as usize, vh as usize);
                // The system owns the region from here; None clears it, so
                // an all-clear frame takes no clicks either.
                let rgn = region(&rects).unwrap_or_else(|| CreateRectRgn(0, 0, 0, 0));
                SetWindowRgn(self.hwnd, Some(rgn), false);
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
