//! Settings in %APPDATA%\clawd-desktop\config.toml.

use log::warn;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SCALES: [i32; 4] = [2, 3, 4, 6];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Screen pixels per sprite pixel.
    pub scale: i32,
    pub paused: bool,
    pub hide_fullscreen: bool,
    /// How many Clawds walk at once, 1 to 5.
    pub count: usize,
    /// His pixels take the mouse: click, double click, drag, right click.
    pub clickable: bool,
    /// Lean on Claude Code's mood from clawd-buddy's session files.
    pub follow_claude: bool,
    /// Clawds that meet chat, shove, ride each other and work side by side.
    pub crowd_antics: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scale: 4,
            paused: false,
            hide_fullscreen: true,
            count: 1,
            clickable: true,
            follow_claude: true,
            crowd_antics: true,
        }
    }
}

/// %APPDATA%\clawd-desktop, created on demand.
pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join("clawd-desktop");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn path() -> PathBuf {
    dir().join("config.toml")
}

impl Config {
    pub fn load() -> Config {
        let Ok(text) = std::fs::read_to_string(path()) else {
            return Config::default();
        };
        match toml::from_str::<Config>(&text) {
            Ok(mut c) => {
                c.scale = c.scale.clamp(1, 12);
                c.count = c.count.clamp(1, crate::crowd::MAX_COUNT);
                c
            }
            Err(e) => {
                warn!("config.toml: {e}; using defaults");
                Config::default()
            }
        }
    }

    pub fn save(&self) {
        match toml::to_string(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(path(), s) {
                    warn!("saving config: {e}");
                }
            }
            Err(e) => warn!("serializing config: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_fills_defaults() {
        let c = Config {
            scale: 6,
            paused: true,
            hide_fullscreen: false,
            count: 4,
            clickable: false,
            follow_claude: false,
            crowd_antics: false,
        };
        let s = toml::to_string(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&s).unwrap(), c);
        let partial: Config = toml::from_str("scale = 3").unwrap();
        assert_eq!(
            partial,
            Config {
                scale: 3,
                ..Config::default()
            }
        );
        assert_eq!(partial.count, 1);
        assert!(partial.clickable, "clickable by default");
        assert!(partial.follow_claude, "follows Claude Code by default");
        assert!(partial.crowd_antics, "crowd antics on by default");
        let three: Config = toml::from_str(
            "count = 3
paused = true",
        )
        .unwrap();
        assert_eq!((three.count, three.paused, three.scale), (3, true, 4));
    }
}
