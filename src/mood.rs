//! Claude Code's mood, from clawd-buddy's session files: one JSON file per
//! Claude Code session in %LOCALAPPDATA%\clawd\sessions,
//! `{"mood":"lockin|chill|celebration","at":<unix ms>,"pid":<pid>}`.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mood {
    Lockin,
    Chill,
    Celebration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Session {
    pub mood: Mood,
    pub at: u64,
    #[serde(default)]
    pub pid: u32,
}

/// A celebration counts for this long after it was written.
pub const CELEBRATION_MS: u64 = 10_000;
/// A file without a pid counts while it is younger than this.
pub const NO_PID_MS: u64 = 6 * 60 * 60 * 1000;

/// What lock in leans toward.
pub const LOCKIN_ACTS: [&str; 6] = ["laptop", "desktop", "notebook", "phone", "thinking", "coffee"];
/// What chill leans toward: clawd-buddy's idle acts that the desktop has.
pub const CHILL_ACTS: [&str; 13] = [
    "snooze",
    "headphones",
    "sunglasses",
    "fishing",
    "watering",
    "meditating",
    "swaying",
    "bubble",
    "berryPicking",
    "blooming",
    "rose",
    "heart",
    "rainbow",
];

pub fn parse(text: &str) -> Option<Session> {
    serde_json::from_str(text).ok()
}

/// Sessions whose Claude Code is still running (or, without a pid, that are recent).
pub fn live(sessions: &[Session], now_ms: u64, alive: impl Fn(u32) -> bool) -> Vec<Session> {
    sessions
        .iter()
        .copied()
        .filter(|s| {
            if s.pid > 0 {
                alive(s.pid)
            } else {
                now_ms.saturating_sub(s.at) < NO_PID_MS
            }
        })
        .collect()
}

/// One desktop mood from the live sessions: celebration if any celebrated in
/// the last ten seconds, else lock in if any is locked in, else chill. None
/// without sessions.
pub fn derive(live: &[Session], now_ms: u64) -> Option<Mood> {
    if live.is_empty() {
        None
    } else if newest_celebration(live, now_ms).is_some() {
        Some(Mood::Celebration)
    } else if live.iter().any(|s| s.mood == Mood::Lockin) {
        Some(Mood::Lockin)
    } else {
        Some(Mood::Chill)
    }
}

/// When the newest celebration of the last ten seconds was written.
pub fn newest_celebration(live: &[Session], now_ms: u64) -> Option<u64> {
    live.iter()
        .filter(|s| s.mood == Mood::Celebration && now_ms.saturating_sub(s.at) <= CELEBRATION_MS)
        .map(|s| s.at)
        .max()
}

/// The acts a mood leans toward.
pub fn lean_names(mood: Option<Mood>) -> &'static [&'static str] {
    match mood {
        Some(Mood::Lockin) => &LOCKIN_ACTS,
        Some(Mood::Chill) => &CHILL_ACTS,
        _ => &[],
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(windows)]
pub mod watch {
    use super::*;
    use log::info;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
        FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    use windows::core::HSTRING;

    /// Rescans this often anyway, for sessions whose process went away.
    const RESCAN: Duration = Duration::from_secs(10);
    /// Looks for the folder this often while it does not exist.
    const LOOK_FOR_DIR: Duration = Duration::from_secs(60);

    pub fn alive(pid: u32) -> bool {
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let mut code = 0u32;
                    let ok = GetExitCodeProcess(h, &mut code).is_ok();
                    let _ = CloseHandle(h);
                    !ok || code == STILL_ACTIVE.0 as u32
                }
                // Access denied means it exists; anything else, gone.
                Err(e) => e.code() == windows::Win32::Foundation::E_ACCESSDENIED,
            }
        }
    }

    /// Watches the sessions folder with a change notification handle, which
    /// the main loop waits on next to its messages: no polling while nothing
    /// changes.
    pub struct Watch {
        dir: PathBuf,
        handle: Option<HANDLE>,
        next_look: Instant,
        next_scan: Instant,
        pub sessions: Vec<Session>,
    }

    impl Watch {
        pub fn new() -> Watch {
            let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
            let now = Instant::now();
            let mut w = Watch {
                dir: base.join("clawd").join("sessions"),
                handle: None,
                next_look: now,
                next_scan: now,
                sessions: Vec::new(),
            };
            w.poll(now);
            w
        }

        pub fn handle(&self) -> Option<HANDLE> {
            self.handle
        }

        fn open(&mut self) {
            if self.handle.is_some() || !self.dir.is_dir() {
                return;
            }
            let h = unsafe {
                FindFirstChangeNotificationW(
                    &HSTRING::from(self.dir.as_os_str()),
                    false,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
                )
            };
            if let Ok(h) = h {
                info!("following Claude Code moods in {}", self.dir.display());
                self.handle = Some(h);
            }
        }

        fn scan(&mut self) {
            self.sessions = std::fs::read_dir(&self.dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                        .filter_map(|t| parse(&t))
                        .collect()
                })
                .unwrap_or_default();
        }

        /// Rescans when the folder changed or every ten seconds. Returns
        /// whether it rescanned.
        pub fn poll(&mut self, now: Instant) -> bool {
            if self.handle.is_none() && now >= self.next_look {
                self.next_look = now + LOOK_FOR_DIR;
                self.open();
            }
            let Some(h) = self.handle else {
                return false;
            };
            let changed = unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0;
            if changed && unsafe { FindNextChangeNotification(h) }.is_err() {
                // The folder went away; look for it again later.
                self.close();
            }
            if changed || now >= self.next_scan {
                self.next_scan = now + RESCAN;
                self.scan();
                return true;
            }
            false
        }

        fn close(&mut self) {
            if let Some(h) = self.handle.take() {
                unsafe {
                    let _ = FindCloseChangeNotification(h);
                }
            }
            self.sessions.clear();
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(mood: Mood, at: u64, pid: u32) -> Session {
        Session { mood, at, pid }
    }

    #[test]
    fn parses_session_files() {
        assert_eq!(
            parse(r#"{"mood":"lockin","at":1759740000000,"pid":4242}"#),
            Some(s(Mood::Lockin, 1_759_740_000_000, 4242))
        );
        assert_eq!(
            parse(r#"{"mood":"celebration","at":5,"pid":1}"#).unwrap().mood,
            Mood::Celebration
        );
        assert_eq!(parse(r#"{"mood":"chill","at":5}"#), Some(s(Mood::Chill, 5, 0)));
        assert_eq!(parse(r#"{"mood":"celebrate","at":5,"pid":1}"#), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn derives_one_mood() {
        let now = 1_000_000;
        assert_eq!(derive(&[], now), None);
        assert_eq!(derive(&[s(Mood::Chill, now, 1)], now), Some(Mood::Chill));
        assert_eq!(
            derive(&[s(Mood::Chill, now, 1), s(Mood::Lockin, now - 50_000, 2)], now),
            Some(Mood::Lockin)
        );
        let fresh = s(Mood::Celebration, now - 9_000, 3);
        assert_eq!(derive(&[s(Mood::Lockin, now, 1), fresh], now), Some(Mood::Celebration));
        assert_eq!(
            newest_celebration(&[fresh, s(Mood::Celebration, now - 2_000, 4)], now),
            Some(now - 2_000)
        );
        // Older than ten seconds, a celebration no longer counts, and is not lock in.
        let old = s(Mood::Celebration, now - 11_000, 3);
        assert_eq!(derive(&[old], now), Some(Mood::Chill));
        assert_eq!(derive(&[old, s(Mood::Lockin, now, 1)], now), Some(Mood::Lockin));
    }

    #[test]
    fn stale_files_are_left_out() {
        let now = 50_000_000;
        let files = [
            s(Mood::Lockin, now - 1_000, 111),    // Claude Code is gone
            s(Mood::Chill, now - 1_000, 222),     // still running
            s(Mood::Lockin, now - NO_PID_MS, 0),  // no pid, too old
            s(Mood::Celebration, now - 500, 333), // gone
        ];
        let alive = |pid| pid == 222;
        let l = live(&files, now, alive);
        assert_eq!(l, vec![s(Mood::Chill, now - 1_000, 222)]);
        assert_eq!(derive(&l, now), Some(Mood::Chill));
        // A recent file without a pid counts.
        let l = live(&[s(Mood::Lockin, now - 60_000, 0)], now, alive);
        assert_eq!(derive(&l, now), Some(Mood::Lockin));
        // Everything stale: no mood at all.
        assert_eq!(derive(&live(&files[..1], now, alive), now), None);
    }

    #[test]
    fn mood_leans() {
        for a in LOCKIN_ACTS.iter().chain(CHILL_ACTS.iter()) {
            crate::sprites::act_index(a);
        }
        assert_eq!(lean_names(Some(Mood::Lockin)), &LOCKIN_ACTS);
        assert_eq!(lean_names(Some(Mood::Chill)), &CHILL_ACTS);
        assert!(lean_names(Some(Mood::Celebration)).is_empty());
        assert!(lean_names(None).is_empty());
    }
}
