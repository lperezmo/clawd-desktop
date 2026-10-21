//! Tray icon and its menu.

use crate::config::{Config, SCALES};
use crate::crowd::MAX_COUNT;
use crate::install::startup_enabled;
use log::warn;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, IMAGE_ICON,
    LR_DEFAULTCOLOR, LoadImageW, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW,
    SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR, w};

pub const WM_TRAY: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    TogglePause,
    Scale(i32),
    Count(usize),
    ToggleFullscreen,
    ToggleStartup,
    ToggleClickable,
    ToggleFollow,
    ToggleAntics,
    ToggleRoam,
    ToggleClimb,
    Perform,
    /// The Clawd the menu was opened on plays the space act in zero g.
    Float,
    Quit,
}

const ID_PAUSE: usize = 1;
const ID_FULLSCREEN: usize = 2;
const ID_STARTUP: usize = 3;
const ID_QUIT: usize = 4;
const ID_CLICKABLE: usize = 5;
const ID_FOLLOW: usize = 6;
const ID_PERFORM: usize = 7;
const ID_ANTICS: usize = 8;
const ID_FLOAT: usize = 9;
const ID_ROAM: usize = 10;
const ID_CLIMB: usize = 11;
const ID_SCALE: usize = 100;
const ID_COUNT: usize = 200;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

/// MAKEINTRESOURCE(1): icon group 1 in build.rs.
#[allow(clippy::manual_dangling_ptr)]
const ICON_ID: PCWSTR = PCWSTR(1 as *const u16);

/// Clawd's icon from the exe's resources (icon group 1, see build.rs), at
/// the small icon size.
fn load_icon() -> HICON {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        LoadImageW(Some(hinst.into()), ICON_ID, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_else(|e| {
                warn!("tray icon: {e}");
                HICON::default()
            })
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let t = Tray {
            hwnd,
            icon: load_icon(),
        };
        t.add();
        t
    }

    /// Adds the icon; also called again when Explorer restarts.
    pub fn add(&self) {
        let mut d = icon_data(self.hwnd);
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = self.icon;
        let tip: Vec<u16> = "Clawd".encode_utf16().collect();
        d.szTip[..tip.len()].copy_from_slice(&tip);
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &d).as_bool() {
                warn!("Shell_NotifyIcon add failed");
            }
        }
    }

    /// Shows the menu at the cursor and returns the chosen command.
    /// `on_clawd`: opened with a right click on a Clawd, which adds Float.
    pub fn menu(&self, cfg: &Config, on_clawd: bool) -> Option<Command> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let scales = CreatePopupMenu().ok()?;
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            for s in SCALES {
                let label = HSTRING::from(format!("{s}x"));
                let _ = AppendMenuW(scales, MF_STRING | check(cfg.scale == s), ID_SCALE + s as usize, &label);
            }
            let counts = CreatePopupMenu().ok()?;
            for n in 1..=MAX_COUNT {
                let label = HSTRING::from(n.to_string());
                let _ = AppendMenuW(counts, MF_STRING | check(cfg.count == n), ID_COUNT + n, &label);
            }
            let pause = if cfg.paused { w!("Resume") } else { w!("Pause") };
            if on_clawd {
                let _ = AppendMenuW(m, MF_STRING, ID_FLOAT, w!("Float"));
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let _ = AppendMenuW(m, MF_STRING, ID_PERFORM, w!("Perform\tCtrl+Alt+C"));
            let _ = AppendMenuW(m, MF_STRING, ID_PAUSE, pause);
            let _ = AppendMenuW(m, MF_STRING | MF_POPUP, counts.0 as usize, w!("Clawds"));
            let _ = AppendMenuW(m, MF_STRING | MF_POPUP, scales.0 as usize, w!("Scale"));
            let _ = AppendMenuW(m, MF_STRING | check(cfg.clickable), ID_CLICKABLE, w!("Clickable"));
            let _ = AppendMenuW(
                m,
                MF_STRING | check(cfg.follow_claude),
                ID_FOLLOW,
                w!("Follow Claude Code"),
            );
            let _ = AppendMenuW(m, MF_STRING | check(cfg.crowd_antics), ID_ANTICS, w!("Crowd antics"));
            let _ = AppendMenuW(m, MF_STRING | check(cfg.free_roam), ID_ROAM, w!("Free roam"));
            let _ = AppendMenuW(m, MF_STRING | check(cfg.climb_walls), ID_CLIMB, w!("Climb walls"));
            let _ = AppendMenuW(
                m,
                MF_STRING | check(cfg.hide_fullscreen),
                ID_FULLSCREEN,
                w!("Hide while fullscreen"),
            );
            let _ = AppendMenuW(
                m,
                MF_STRING | check(startup_enabled()),
                ID_STARTUP,
                w!("Start with Windows"),
            );
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(m, MF_STRING, ID_QUIT, w!("Quit"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // The menu closes properly on an outside click only if our window
            // is foreground while it is open (documented TrackPopupMenu quirk).
            let _ = SetForegroundWindow(self.hwnd);
            let id = TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            )
            .0 as usize;
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, Default::default(), Default::default());
            let _ = DestroyMenu(m);
            command(id)
        }
    }
}

/// The command behind a menu item id; None for a dismissed menu.
fn command(id: usize) -> Option<Command> {
    match id {
        ID_PAUSE => Some(Command::TogglePause),
        ID_FULLSCREEN => Some(Command::ToggleFullscreen),
        ID_STARTUP => Some(Command::ToggleStartup),
        ID_CLICKABLE => Some(Command::ToggleClickable),
        ID_FOLLOW => Some(Command::ToggleFollow),
        ID_ANTICS => Some(Command::ToggleAntics),
        ID_ROAM => Some(Command::ToggleRoam),
        ID_CLIMB => Some(Command::ToggleClimb),
        ID_PERFORM => Some(Command::Perform),
        ID_FLOAT => Some(Command::Float),
        ID_QUIT => Some(Command::Quit),
        n if n > ID_COUNT => Some(Command::Count(n - ID_COUNT)),
        n if n > ID_SCALE => Some(Command::Scale((n - ID_SCALE) as i32)),
        _ => None,
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(self.hwnd));
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_item_maps_to_its_command() {
        assert_eq!(command(ID_FLOAT), Some(Command::Float));
        assert_eq!(command(ID_PERFORM), Some(Command::Perform));
        assert_eq!(command(ID_QUIT), Some(Command::Quit));
        assert_eq!(command(ID_SCALE + 4), Some(Command::Scale(4)));
        assert_eq!(command(ID_COUNT + 3), Some(Command::Count(3)));
        assert_eq!(command(0), None, "dismissed");
        // The plain items stay clear of the scale and count ranges.
        let plain = [
            ID_PAUSE,
            ID_FULLSCREEN,
            ID_STARTUP,
            ID_QUIT,
            ID_CLICKABLE,
            ID_FOLLOW,
            ID_PERFORM,
            ID_ANTICS,
            ID_FLOAT,
            ID_ROAM,
            ID_CLIMB,
        ];
        for (k, id) in plain.iter().enumerate() {
            assert!(*id < ID_SCALE);
            assert!(!plain[k + 1..].contains(id), "{id} used twice");
        }
    }
}
