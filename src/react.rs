//! Reacting to the desktop: which kind of window just opened, and what
//! Clawd does about it.

/// The kinds of new window Clawd reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Terminal,
    Browser,
    Explorer,
}

/// Classifies a window by its process's exe name and its window class.
pub fn classify(exe: &str, class: &str) -> Option<WindowKind> {
    let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    match class {
        "CASCADIA_HOSTING_WINDOW_CLASS" | "ConsoleWindowClass" => return Some(WindowKind::Terminal),
        "CabinetWClass" => return Some(WindowKind::Explorer),
        _ => {}
    }
    match stem {
        "windowsterminal" | "powershell" | "pwsh" | "cmd" | "conhost" | "wt" | "openconsole" => {
            Some(WindowKind::Terminal)
        }
        s if s.starts_with("alacritty") || s.starts_with("wezterm") => Some(WindowKind::Terminal),
        "chrome" | "msedge" | "firefox" | "brave" => Some(WindowKind::Browser),
        // Explorer owns the taskbar, the desktop and more; only its folder windows count.
        _ => None,
    }
}

/// The acts a new window of `kind` may start.
pub fn acts_for(kind: WindowKind) -> &'static [&'static str] {
    match kind {
        WindowKind::Terminal => &["coffee", "laptop"],
        WindowKind::Browser => &["lightbulb"],
        WindowKind::Explorer => &["notebook"],
    }
}

/// What a foreground change may start.
pub const FOREGROUND_ACTS: [&str; 2] = ["turning", "looking"];

/// What a double click, or Claude Code celebrating, may start.
pub const CELEBRATION_ACTS: [&str; 11] = [
    "confetti",
    "confettiHappy",
    "dancing",
    "dancingHappy",
    "jumpingHappy",
    "excited",
    "breakDancing",
    "juggling",
    "hulaHoop",
    "arcade",
    "spark",
];

/// Indices of `(x, y)` points ordered by distance to `to`, nearest first.
pub fn nearest_first(points: &[(i32, i32)], to: (i32, i32)) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..points.len()).collect();
    idx.sort_by_key(|&i| {
        let (dx, dy) = ((points[i].0 - to.0) as i64, (points[i].1 - to.1) as i64);
        dx * dx + dy * dy
    });
    idx
}

#[cfg(windows)]
pub mod win {
    use crate::surfaces::Rect;
    use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
        WS_EX_TOOLWINDOW,
    };
    use windows::core::PWSTR;

    /// A shown top-level window: its exe path, class and rectangle. None for
    /// hidden windows and tool windows.
    pub fn describe(h: HWND) -> Option<(String, String, Rect)> {
        unsafe {
            if !IsWindowVisible(h).as_bool() || GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            let mut buf = [0u16; 128];
            let n = GetClassNameW(h, &mut buf);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            let mut r = RECT::default();
            GetWindowRect(h, &mut r).ok()?;
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let exe = exe_of(pid).unwrap_or_default();
            Some((exe, class, Rect::new(r.left, r.top, r.right, r.bottom)))
        }
    }

    pub fn exe_of(pid: u32) -> Option<String> {
        unsafe {
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = CloseHandle(p);
            ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprites::act_index;

    #[test]
    fn classifies_by_process_and_class() {
        let t = Some(WindowKind::Terminal);
        for exe in [
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21\WindowsTerminal.exe",
            "powershell.exe",
            "PWSH.EXE",
            "cmd.exe",
            "conhost.exe",
            "wt.exe",
            r"C:\Program Files\Alacritty\alacritty.exe",
            "wezterm-gui.exe",
        ] {
            assert_eq!(classify(exe, "Whatever"), t, "{exe}");
        }
        assert_eq!(classify("", "CASCADIA_HOSTING_WINDOW_CLASS"), t);
        assert_eq!(classify("", "ConsoleWindowClass"), t);
        let b = Some(WindowKind::Browser);
        for exe in ["chrome.exe", r"C:\x\msedge.exe", "firefox.exe", "brave.exe"] {
            assert_eq!(classify(exe, "Chrome_WidgetWin_1"), b, "{exe}");
        }
        assert_eq!(
            classify(r"C:\Windows\explorer.exe", "CabinetWClass"),
            Some(WindowKind::Explorer)
        );
        // Explorer's other windows (taskbar, desktop, Start) do not count.
        assert_eq!(classify(r"C:\Windows\explorer.exe", "Shell_TrayWnd"), None);
        assert_eq!(classify("notepad.exe", "Notepad"), None);
        assert_eq!(classify("chromedriver.exe", "x"), None);
    }

    #[test]
    fn reaction_acts_exist() {
        for k in [WindowKind::Terminal, WindowKind::Browser, WindowKind::Explorer] {
            for a in acts_for(k) {
                act_index(a);
            }
        }
        for a in FOREGROUND_ACTS.iter().chain(CELEBRATION_ACTS.iter()) {
            act_index(a);
        }
    }

    #[test]
    fn nearest_first_orders_by_distance() {
        let pts = [(1000, 500), (100, 100), (400, 400)];
        assert_eq!(nearest_first(&pts, (90, 90)), vec![1, 2, 0]);
        assert_eq!(nearest_first(&pts, (990, 520)), vec![0, 2, 1]);
    }
}
