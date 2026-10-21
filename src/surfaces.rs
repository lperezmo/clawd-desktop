//! Walkable surfaces: window top edges and the taskbar top, in physical
//! screen pixels, plus the invisible lanes of free roam.
//!
//! `build` is pure so it can be tested; `scan_windows` and `scan_monitors`
//! read the live desktop.

use log::debug;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect::new(
            self.left.max(o.left),
            self.top.max(o.top),
            self.right.min(o.right),
            self.bottom.min(o.bottom),
        );
        (r.left < r.right && r.top < r.bottom).then_some(r)
    }
}

/// Owner id of the taskbar ground; windows use their HWND value.
pub const GROUND: isize = 0;
/// Owner id of an invisible free-roam lane.
pub const LANE: isize = -1;

/// Two segments at the same y closer than this merge into one surface.
const MERGE_GAP: i32 = 2;
/// Segments narrower than this are not worth standing on.
const MIN_WIDTH: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub x0: i32,
    pub x1: i32,
    pub owner: isize,
}

/// A horizontal segment `[x0, x1)` at height `y`; Clawd's feet go on `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
    pub ground: bool,
    pub parts: Vec<Part>,
}

impl Surface {
    pub fn contains_x(&self, x: i32) -> bool {
        x >= self.x0 && x < self.x1
    }
    pub fn owner_at(&self, x: i32) -> isize {
        // A window or the ground under him beats a lane: he rides it.
        let under = |p: &&Part| x >= p.x0 && x < p.x1;
        self.parts
            .iter()
            .filter(under)
            .find(|p| p.owner != LANE)
            .or_else(|| self.parts.iter().find(under))
            .or(self.parts.first())
            .map_or(GROUND, |p| p.owner)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    pub rect: Rect,
    pub work: Rect,
    pub primary: bool,
}

/// Subtracts `[a, b)` from every span in `spans`.
fn subtract(spans: &mut Vec<(i32, i32)>, a: i32, b: i32) {
    let mut out = Vec::with_capacity(spans.len() + 1);
    for &(x0, x1) in spans.iter() {
        if b <= x0 || a >= x1 {
            out.push((x0, x1));
            continue;
        }
        if a > x0 {
            out.push((x0, a));
        }
        if b < x1 {
            out.push((b, x1));
        }
    }
    *spans = out;
}

/// Builds the surface list. `windows` must be in z-order, topmost first.
/// A window's top edge counts only where no higher window covers that row,
/// only on a monitor, at least `headroom` pixels below the monitor top (so
/// Clawd fits above it) and above the taskbar. Each monitor's work-area
/// bottom is ground. Touching segments at the same y merge.
#[cfg(test)]
pub fn build(windows: &[WindowInfo], monitors: &[MonitorInfo], headroom: i32) -> Vec<Surface> {
    build_with(windows, monitors, headroom, None)
}

/// `build`, plus with `lanes` (free roam) an invisible full-width lane on
/// every monitor every `lanes` pixels above its work-area bottom, up to
/// `headroom` below its top, the way the phone has them.
pub fn build_with(windows: &[WindowInfo], monitors: &[MonitorInfo], headroom: i32, lanes: Option<i32>) -> Vec<Surface> {
    let mut segs: Vec<(i32, Part, bool)> = Vec::new();
    for (i, w) in windows.iter().enumerate() {
        let y = w.rect.top;
        let mut spans = vec![(w.rect.left, w.rect.right)];
        for higher in &windows[..i] {
            let h = higher.rect;
            if h.top <= y && y < h.bottom {
                subtract(&mut spans, h.left, h.right);
                if spans.is_empty() {
                    break;
                }
            }
        }
        for m in monitors {
            if y < m.rect.top + headroom || y >= m.work.bottom {
                continue;
            }
            for &(x0, x1) in &spans {
                let (a, b) = (x0.max(m.rect.left), x1.min(m.rect.right));
                if b - a >= MIN_WIDTH {
                    segs.push((
                        y,
                        Part {
                            x0: a,
                            x1: b,
                            owner: w.hwnd,
                        },
                        false,
                    ));
                }
            }
        }
    }
    for m in monitors {
        let w = m.work;
        if w.right - w.left >= MIN_WIDTH {
            segs.push((
                w.bottom,
                Part {
                    x0: w.left,
                    x1: w.right,
                    owner: GROUND,
                },
                true,
            ));
        }
    }
    if let Some(spacing) = lanes.filter(|&g| g > 0) {
        for m in monitors {
            let mut y = m.work.bottom - spacing;
            while y >= m.rect.top + headroom {
                segs.push((
                    y,
                    Part {
                        x0: m.rect.left,
                        x1: m.rect.right,
                        owner: LANE,
                    },
                    false,
                ));
                y -= spacing;
            }
        }
    }
    segs.sort_by_key(|(y, p, _)| (*y, p.x0, p.x1));

    let mut out: Vec<Surface> = Vec::new();
    for (y, part, ground) in segs {
        if let Some(last) = out.last_mut()
            && last.y == y
            && part.x0 <= last.x1 + MERGE_GAP
        {
            last.x1 = last.x1.max(part.x1);
            last.ground |= ground;
            last.parts.push(part);
            continue;
        }
        out.push(Surface {
            y,
            x0: part.x0,
            x1: part.x1,
            ground,
            parts: vec![part],
        });
    }
    out
}

/// The first surface strictly below `y` that spans `x`: where a fall from
/// (`x`, `y`) lands.
pub fn fall_target(surfaces: &[Surface], x: i32, y: i32) -> Option<usize> {
    surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.y > y && s.contains_x(x))
        .min_by_key(|(_, s)| s.y)
        .map(|(i, _)| i)
}

/// The surface Clawd is standing on: the part owned by `owner` under `x` if
/// that window still has one (it may have moved), otherwise any surface at
/// exactly `y` under `x`.
pub fn support(surfaces: &[Surface], x: i32, y: i32, owner: isize) -> Option<(usize, isize)> {
    if owner != GROUND && owner != LANE {
        for (i, s) in surfaces.iter().enumerate() {
            if s.parts.iter().any(|p| p.owner == owner && x >= p.x0 && x < p.x1) {
                return Some((i, owner));
            }
        }
    }
    surfaces
        .iter()
        .position(|s| s.y == y && s.contains_x(x))
        .map(|i| (i, surfaces[i].owner_at(x)))
}

/// The nearest surface above feet at `y` that spans `x`, no more than
/// `reach` pixels up: where a hop up from (`x`, `y`) lands.
pub fn hop_target(surfaces: &[Surface], x: i32, y: i32, reach: i32) -> Option<usize> {
    surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.y < y && s.y >= y - reach && s.contains_x(x))
        .max_by_key(|(_, s)| s.y)
        .map(|(i, _)| i)
}

/// The ground surface nearest to `x`, used when Clawd ends up with nothing below.
pub fn nearest_ground(surfaces: &[Surface], x: i32) -> Option<usize> {
    surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.ground)
        .min_by_key(|(_, s)| {
            if s.contains_x(x) {
                0
            } else {
                (s.x0 - x).abs().min((s.x1 - 1 - x).abs())
            }
        })
        .map(|(i, _)| i)
}

#[cfg(windows)]
pub mod scan {
    use super::*;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    use windows::core::BOOL;

    const MONITORINFOF_PRIMARY: u32 = 1;

    fn rect(r: RECT) -> Rect {
        Rect::new(r.left, r.top, r.right, r.bottom)
    }

    pub fn monitors() -> Vec<MonitorInfo> {
        unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
            let out = unsafe { &mut *(lp.0 as *mut Vec<MonitorInfo>) };
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(hm, &mut mi) }.as_bool() {
                out.push(MonitorInfo {
                    rect: rect(mi.rcMonitor),
                    work: rect(mi.rcWork),
                    primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
            BOOL(1)
        }
        let mut out: Vec<MonitorInfo> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    fn class_name(h: HWND) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(h, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn is_shell_class(c: &str) -> bool {
        matches!(c, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
    }

    pub fn cloaked(h: HWND) -> bool {
        let mut v: u32 = 0;
        unsafe {
            DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut _ as *mut _, size_of::<u32>() as u32).is_ok()
                && v != 0
        }
    }

    fn frame_bounds(h: HWND) -> Option<Rect> {
        let mut r = RECT::default();
        unsafe {
            if DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut r as *mut _ as *mut _,
                size_of::<RECT>() as u32,
            )
            .is_err()
            {
                GetWindowRect(h, &mut r).ok()?;
            }
        }
        let r = rect(r);
        (r.width() > 0 && r.height() > 0).then_some(r)
    }

    /// Visible, non-minimized, non-cloaked top-level windows of other
    /// processes, topmost first. Tool windows, click-through overlays and the
    /// desktop and taskbar are skipped.
    pub fn windows() -> Vec<WindowInfo> {
        struct Ctx {
            pid: u32,
            out: Vec<WindowInfo>,
        }
        unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
            let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
            unsafe {
                if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                    return BOOL(1);
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                if pid == ctx.pid {
                    return BOOL(1);
                }
                let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
                if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_TRANSPARENT.0) != 0 {
                    return BOOL(1);
                }
            }
            if cloaked(h) || is_shell_class(&class_name(h)) {
                return BOOL(1);
            }
            if let Some(rect) = frame_bounds(h) {
                ctx.out.push(WindowInfo {
                    hwnd: h.0 as isize,
                    rect,
                });
            }
            BOOL(1)
        }
        let mut ctx = Ctx {
            pid: std::process::id(),
            out: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        }
        debug!("enumerated {} windows", ctx.out.len());
        ctx.out
    }

    /// True when the foreground window covers its whole monitor and is not
    /// the desktop or the shell.
    pub fn foreground_is_fullscreen() -> bool {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() || is_shell_class(&class_name(fg)) {
                return false;
            }
            let mut r = RECT::default();
            if GetWindowRect(fg, &mut r).is_err() {
                return false;
            }
            let hm = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(hm, &mut mi).as_bool() {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon() -> MonitorInfo {
        MonitorInfo {
            rect: Rect::new(0, 0, 1920, 1080),
            work: Rect::new(0, 0, 1920, 1032),
            primary: true,
        }
    }

    fn win(h: isize, l: i32, t: i32, r: i32, b: i32) -> WindowInfo {
        WindowInfo {
            hwnd: h,
            rect: Rect::new(l, t, r, b),
        }
    }

    #[test]
    fn ground_is_taskbar_top() {
        let s = build(&[], &[mon()], 72);
        assert_eq!(s.len(), 1);
        assert!(s[0].ground);
        assert_eq!((s[0].x0, s[0].x1, s[0].y), (0, 1920, 1032));
    }

    #[test]
    fn higher_window_occludes_lower_edge() {
        // Window 1 on top covers the middle of window 2's title row.
        let ws = [win(1, 400, 150, 800, 600), win(2, 100, 200, 1000, 700)];
        let s = build(&ws, &[mon()], 72);
        let at200: Vec<_> = s.iter().filter(|s| s.y == 200).map(|s| (s.x0, s.x1)).collect();
        assert_eq!(at200, vec![(100, 400), (800, 1000)]);
        let at150: Vec<_> = s.iter().filter(|s| s.y == 150).map(|s| (s.x0, s.x1)).collect();
        assert_eq!(at150, vec![(400, 800)]);
    }

    #[test]
    fn lower_window_does_not_occlude_higher_edge() {
        let ws = [win(1, 100, 300, 900, 700), win(2, 0, 100, 1200, 900)];
        let s = build(&ws, &[mon()], 72);
        assert!(s.iter().any(|s| s.y == 300 && s.x0 == 100 && s.x1 == 900));
        assert!(s.iter().any(|s| s.y == 100 && s.x0 == 0 && s.x1 == 1200));
    }

    #[test]
    fn fully_covered_edge_disappears() {
        let ws = [win(1, 0, 0, 1920, 1032), win(2, 100, 200, 1000, 700)];
        let s = build(&ws, &[mon()], 72);
        assert!(s.iter().all(|s| s.y != 200));
        // Window 1 is maximized: its top is the screen top, no headroom.
        assert!(s.iter().all(|s| s.y != 0));
    }

    #[test]
    fn touching_edges_at_same_y_merge() {
        let ws = [win(1, 0, 300, 600, 700), win(2, 601, 300, 1200, 700)];
        let s = build(&ws, &[mon()], 72);
        let m: Vec<_> = s.iter().filter(|s| s.y == 300).collect();
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].x0, m[0].x1), (0, 1200));
        assert_eq!(m[0].owner_at(100), 1);
        assert_eq!(m[0].owner_at(900), 2);
    }

    #[test]
    fn edges_clip_to_monitors_and_skip_taskbar() {
        let ws = [win(1, -200, 400, 300, 800), win(2, 100, 1050, 600, 1300)];
        let s = build(&ws, &[mon()], 72);
        assert!(s.iter().any(|s| s.y == 400 && s.x0 == 0 && s.x1 == 300));
        assert!(s.iter().all(|s| s.y != 1050));
    }

    #[test]
    fn two_monitor_grounds_merge() {
        let b = MonitorInfo {
            rect: Rect::new(1920, 0, 3840, 1080),
            work: Rect::new(1920, 0, 3840, 1032),
            primary: false,
        };
        let s = build(&[], &[mon(), b], 72);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].x0, s[0].x1), (0, 3840));
    }

    #[test]
    fn fall_lands_on_first_surface_below() {
        let ws = [win(1, 100, 300, 900, 700), win(2, 500, 600, 1500, 900)];
        let s = build(&ws, &[mon()], 72);
        let t = |x, y| fall_target(&s, x, y).map(|i| s[i].y);
        assert_eq!(t(200, 100), Some(300));
        // Below window 1's edge, window 2's edge is hidden under window 1 at x = 600.
        assert_eq!(t(600, 300), Some(1032));
        assert_eq!(t(1000, 300), Some(600));
        assert_eq!(t(1000, 600), Some(1032));
        assert_eq!(t(1000, 1032), None);
        // Starting exactly on a surface does not land on it again.
        assert_eq!(t(200, 300), Some(1032));
    }

    #[test]
    fn lanes_hang_above_the_ground_and_stop_below_the_top() {
        let s = build_with(&[], &[mon()], 72, Some(81));
        let ys: Vec<i32> = s.iter().map(|s| s.y).collect();
        assert_eq!(*ys.last().unwrap(), 1032);
        assert!(s.last().unwrap().ground);
        let lanes: Vec<&Surface> = s.iter().filter(|s| !s.ground).collect();
        assert_eq!(lanes.len(), (1032 - 72) / 81);
        for l in &lanes {
            assert_eq!((l.x0, l.x1), (0, 1920));
            assert_eq!((1032 - l.y) % 81, 0);
            assert!(l.y >= 72);
            assert_eq!(l.owner_at(500), LANE);
        }
        assert_eq!(build_with(&[], &[mon()], 72, None), build(&[], &[mon()], 72));
    }

    #[test]
    fn a_window_top_on_a_lane_keeps_its_owner() {
        let s = build_with(&[win(5, 100, 1032 - 81, 900, 1032)], &[mon()], 72, Some(81));
        let row = s.iter().find(|s| s.y == 1032 - 81).unwrap();
        assert_eq!((row.x0, row.x1), (0, 1920));
        assert_eq!(row.owner_at(500), 5);
        // Standing on a lane is like standing on the ground: found by its y.
        let lane = s.iter().position(|s| s.y == 1032 - 162).unwrap();
        assert_eq!(support(&s, 50, 1032 - 162, LANE), Some((lane, LANE)));
    }

    #[test]
    fn hop_goes_to_the_nearest_surface_above_within_reach() {
        let s = build_with(&[win(1, 100, 900, 500, 1000)], &[mon()], 72, Some(300));
        let t = |x, y, r| hop_target(&s, x, y, r).map(|i| s[i].y);
        assert_eq!(t(200, 1032, 200), Some(900));
        assert_eq!(t(700, 1032, 200), None, "no window there and the lane is too high");
        assert_eq!(t(700, 1032, 400), Some(732));
        assert_eq!(t(200, 900, 200), Some(732));
    }

    #[test]
    fn support_follows_owner_window() {
        let before = build(&[win(7, 100, 400, 900, 800)], &[mon()], 72);
        assert_eq!(
            support(&before, 500, 400, 7).map(|(i, o)| (before[i].y, o)),
            Some((400, 7))
        );
        // The window is dragged up: Clawd rides it.
        let after = build(&[win(7, 100, 250, 900, 650)], &[mon()], 72);
        assert_eq!(support(&after, 500, 400, 7).map(|(i, _)| after[i].y), Some(250));
        // The window closes: nothing under him.
        let gone = build(&[], &[mon()], 72);
        assert_eq!(support(&gone, 500, 400, 7), None);
    }
}
