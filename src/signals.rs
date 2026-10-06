//! What the leans read from the system: the local hour, the battery, and
//! whether media is playing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// Local hour, 0 to 23.
pub fn local_hour() -> u32 {
    unsafe { GetLocalTime().wHour as u32 }
}

/// On battery power with under 20 percent left. Unknown counts as no.
pub fn low_battery() -> bool {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return false;
    }
    is_low(s.ACLineStatus, s.BatteryLifePercent)
}

/// `ac` 0 is on battery; `percent` 255 is unknown.
pub fn is_low(ac: u8, percent: u8) -> bool {
    ac == 0 && percent < 20
}

static MEDIA_PLAYING: AtomicBool = AtomicBool::new(false);
/// How often the media worker looks.
const MEDIA_EVERY: Duration = Duration::from_secs(5);

pub fn media_playing() -> bool {
    MEDIA_PLAYING.load(Ordering::Relaxed)
}

/// Starts a worker thread that asks the system media transport controls
/// every five seconds whether any session is playing. Lives until the
/// process exits.
pub fn start_media_watch() {
    let r = std::thread::Builder::new().name("media".into()).spawn(|| {
        use windows::Media::Control::{
            GlobalSystemMediaTransportControlsSessionManager as Manager,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
        };
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("media controls unavailable: {e}");
                return;
            }
        };
        log::info!("watching media playback every {} s", MEDIA_EVERY.as_secs());
        let mut was = false;
        loop {
            let playing = manager.GetSessions().is_ok_and(|list| {
                (0..list.Size().unwrap_or(0)).any(|i| {
                    list.GetAt(i)
                        .and_then(|s| s.GetPlaybackInfo())
                        .and_then(|p| p.PlaybackStatus())
                        .is_ok_and(|st| st == Status::Playing)
                })
            });
            if playing != was {
                log::info!("media {}", if playing { "playing" } else { "stopped" });
                was = playing;
            }
            MEDIA_PLAYING.store(playing, Ordering::Relaxed);
            std::thread::sleep(MEDIA_EVERY);
        }
    });
    if let Err(e) = r {
        log::warn!("media thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_rule() {
        assert!(is_low(0, 19));
        assert!(!is_low(0, 20));
        assert!(!is_low(1, 5), "plugged in");
        assert!(!is_low(255, 5), "unknown power");
        assert!(!is_low(0, 255), "unknown percent");
    }
}
