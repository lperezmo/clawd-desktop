//! Tray icon, its menu, and the Start with Windows shortcut.

use crate::config::{Config, SCALES};
use crate::sprites::{SpriteRef, frame, sheet_index};
use log::warn;
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile};
use windows::Win32::UI::Shell::{
    FOLDERID_Startup, IShellLinkW, KNOWN_FOLDER_FLAG, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW, SHGetKnownFolderPath, Shell_NotifyIconW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, ICONINFO,
    MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW, SetForegroundWindow, TPM_BOTTOMALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

pub const WM_TRAY: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    TogglePause,
    Scale(i32),
    ToggleFullscreen,
    ToggleStartup,
    Quit,
}

const ID_PAUSE: usize = 1;
const ID_FULLSCREEN: usize = 2;
const ID_STARTUP: usize = 3;
const ID_QUIT: usize = 4;
const ID_SCALE: usize = 100;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

/// Clawd's standing frame, centred on a 32 by 32 icon.
fn make_icon() -> Option<HICON> {
    let f = frame(SpriteRef::Sheet(sheet_index("walk")), 0);
    let mut px = Vec::new();
    f.render(1, false, &mut px);
    let (n, ox, oy) = (32usize, (32 - f.w) / 2, (32 - f.h) / 2);
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n as i32,
                biHeight: -(n as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dib = core::slice::from_raw_parts_mut(bits as *mut u32, n * n);
        dib.fill(0);
        for y in 0..f.h {
            for x in 0..f.w {
                dib[(y + oy) * n + x + ox] = px[y * f.w + x];
            }
        }
        let mask = CreateBitmap(n as i32, n as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
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
            icon: make_icon().unwrap_or_default(),
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
    pub fn menu(&self, cfg: &Config) -> Option<Command> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let scales = CreatePopupMenu().ok()?;
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            for s in SCALES {
                let label = HSTRING::from(format!("{s}x"));
                let _ = AppendMenuW(scales, MF_STRING | check(cfg.scale == s), ID_SCALE + s as usize, &label);
            }
            let pause = if cfg.paused { w!("Resume") } else { w!("Pause") };
            let _ = AppendMenuW(m, MF_STRING, ID_PAUSE, pause);
            let _ = AppendMenuW(m, MF_STRING | MF_POPUP, scales.0 as usize, w!("Scale"));
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
            match id {
                ID_PAUSE => Some(Command::TogglePause),
                ID_FULLSCREEN => Some(Command::ToggleFullscreen),
                ID_STARTUP => Some(Command::ToggleStartup),
                ID_QUIT => Some(Command::Quit),
                n if n > ID_SCALE => Some(Command::Scale((n - ID_SCALE) as i32)),
                _ => None,
            }
        }
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

fn shortcut_path() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Startup, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        Some(PathBuf::from(s?).join("Clawd Desktop.lnk"))
    }
}

pub fn startup_enabled() -> bool {
    shortcut_path().is_some_and(|p| p.exists())
}

/// Adds or removes the Startup folder shortcut.
pub fn set_startup(on: bool) -> Result<(), String> {
    let Some(path) = shortcut_path() else {
        return Err("no Startup folder".into());
    };
    if !on {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    make_link(&exe, &path).map_err(|e| e.to_string())
}

fn make_link(exe: &std::path::Path, path: &std::path::Path) -> windows::core::Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(w!("Clawd walks along your windows"))?;
        let pf: IPersistFile = link.cast()?;
        pf.Save(&HSTRING::from(path.as_os_str()), true)?;
    }
    Ok(())
}
