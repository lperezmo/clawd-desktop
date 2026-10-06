//! `--install`, `--uninstall` and the Start with Windows shortcut.
//!
//! Shortcuts are written through IShellLinkW, so COM must be initialised on
//! the calling thread.

use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Startup, IShellLinkW, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{GUID, HSTRING, Interface, w};

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// `%LOCALAPPDATA%\Programs\clawd-desktop\clawd-desktop.exe`.
pub fn installed_exe() -> Option<PathBuf> {
    Some(
        known_folder(&FOLDERID_LocalAppData)?
            .join("Programs")
            .join("clawd-desktop")
            .join("clawd-desktop.exe"),
    )
}

fn desktop_link() -> Option<PathBuf> {
    Some(known_folder(&FOLDERID_Desktop)?.join("Clawd.lnk"))
}

fn startup_link() -> Option<PathBuf> {
    Some(known_folder(&FOLDERID_Startup)?.join("Clawd Desktop.lnk"))
}

/// What Start with Windows and the desktop shortcut launch: the installed
/// copy when there is one, otherwise this exe.
pub fn launch_target() -> std::io::Result<PathBuf> {
    match installed_exe() {
        Some(p) if p.exists() => Ok(p),
        _ => std::env::current_exe(),
    }
}

pub fn startup_enabled() -> bool {
    startup_link().is_some_and(|p| p.exists())
}

/// Adds or removes the Startup folder shortcut.
pub fn set_startup(on: bool) -> Result<(), String> {
    let Some(path) = startup_link() else {
        return Err("no Startup folder".into());
    };
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = launch_target().map_err(|e| e.to_string())?;
    make_link(&exe, &path).map_err(|e| e.to_string())
}

fn make_link(exe: &Path, path: &Path) -> windows::core::Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        link.SetDescription(w!("Clawd walks along your windows"))?;
        let pf: IPersistFile = link.cast()?;
        pf.Save(&HSTRING::from(path.as_os_str()), true)?;
    }
    Ok(())
}

/// Runs `f` with COM initialised on this thread.
fn with_com<T>(f: impl FnOnce() -> T) -> T {
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let r = f();
    if com {
        unsafe { CoUninitialize() };
    }
    r
}

fn require(p: Option<PathBuf>, what: &str) -> Result<PathBuf, String> {
    p.ok_or_else(|| format!("could not find the {what} folder"))
}

/// Copies this exe to the install folder and makes the desktop shortcut.
/// Returns the lines to print.
pub fn install() -> Result<Vec<String>, String> {
    with_com(|| {
        let mut out = Vec::new();
        let src = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
        let dst = require(installed_exe(), "LocalAppData")?;
        let dir = dst.parent().expect("installed exe has a parent");
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let same = std::fs::canonicalize(&src).ok() == std::fs::canonicalize(&dst).ok();
        if same {
            out.push(format!("already running from {}", dst.display()));
        } else {
            std::fs::copy(&src, &dst).map_err(|e| {
                format!(
                    "copying to {}: {e} (quit the running Clawd from the tray and try again)",
                    dst.display()
                )
            })?;
            out.push(format!("copied {} to {}", src.display(), dst.display()));
        }
        let link = require(desktop_link(), "Desktop")?;
        make_link(&dst, &link).map_err(|e| format!("shortcut {}: {e}", link.display()))?;
        out.push(format!("created {} -> {}", link.display(), dst.display()));
        if startup_enabled() {
            set_startup(true)?;
            out.push("Start with Windows now launches the installed copy".into());
        }
        Ok(out)
    })
}

/// Removes the desktop and Startup shortcuts and the installed copy.
pub fn uninstall() -> Result<Vec<String>, String> {
    with_com(|| {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let mut remove = |p: Option<PathBuf>| {
            let Some(p) = p else { return };
            match std::fs::remove_file(&p) {
                Ok(()) => out.push(format!("removed {}", p.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        };
        remove(desktop_link());
        remove(startup_link());
        remove(installed_exe());
        if let Some(dir) = installed_exe().as_deref().and_then(Path::parent)
            && std::fs::remove_dir(dir).is_ok()
        {
            out.push(format!("removed {}", dir.display()));
        }
        if out.is_empty() && errors.is_empty() {
            out.push("nothing to remove".into());
        }
        if errors.is_empty() {
            Ok(out)
        } else {
            out.extend(errors.iter().map(|e| format!("could not remove {e}")));
            Err(out.join("\n"))
        }
    })
}
