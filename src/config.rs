//! The settings file, remora.json, kept beside the binary so the
//! whole install travels as one folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::decide::IdleConfig;

pub const FILE_NAME: &str = "remora.json";

/// Defaults for the idle auto-stop (decided 2026-11-07): 30 min for something to
/// start playing, and half an hour
/// past the end of whatever was loaded.
const DEFAULT_START_MINUTES: u64 = 30;
const DEFAULT_MARGIN_MINUTES: u64 = 30;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IdleStop {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_minutes: Option<u64>,
}

/// The file's contents. Every field is optional.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    /// The portable Jellyfin folder; relative to the file. Absent means the
    /// file's own folder, where the launcher is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jellyfin_dir: Option<String>,
    #[serde(default)]
    pub idle_stop: IdleStop,
    /// firewalld zones the user has agreed to open the ports in; after the first
    /// time, they are opened without asking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved_zones: Vec<String>,

    #[serde(skip)]
    path: PathBuf,
}

impl Config {
    /// Reads `path`. A missing file is the defaults; a broken one is an error,
    /// because running on defaults would silently use the wrong folder.
    pub fn load(path: &Path) -> io::Result<Config> {
        let mut c: Config = match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(e),
        };
        c.path = path.to_path_buf();
        Ok(c)
    }

    /// The Jellyfin folder, resolved against the file's own folder.
    pub fn jellyfin_dir(&self) -> PathBuf {
        let base = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        match &self.jellyfin_dir {
            None => base,
            Some(d) if Path::new(d).is_absolute() => PathBuf::from(d),
            Some(d) => base.join(d),
        }
    }

    /// The auto-stop clock; the default (zero) is off.
    pub fn idle(&self) -> IdleConfig {
        if self.idle_stop.off {
            return IdleConfig::default();
        }
        let minutes = |v: Option<u64>, d| Duration::from_secs(v.filter(|&m| m > 0).unwrap_or(d) * 60);
        IdleConfig {
            start_window: minutes(self.idle_stop.start_minutes, DEFAULT_START_MINUTES),
            margin: minutes(self.idle_stop.margin_minutes, DEFAULT_MARGIN_MINUTES),
        }
    }

    pub fn zone_approved(&self, zone: &str) -> bool {
        zone != "public" && self.approved_zones.iter().any(|z| z == zone)
    }

    /// Records the agreement and saves. The public zone is refused here too.
    pub fn approve_zone(&mut self, zone: &str) -> io::Result<()> {
        if zone.is_empty() || zone == "public" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the public zone is never opened"));
        }
        if !self.approved_zones.iter().any(|z| z == zone) {
            self.approved_zones.push(zone.into());
            self.save()?;
        }
        Ok(())
    }

    /// Saves new auto-stop settings. Minutes of 0 mean the default.
    pub fn set_idle(&mut self, off: bool, start_minutes: u64, margin_minutes: u64) -> io::Result<()> {
        self.idle_stop = IdleStop {
            off,
            start_minutes: Some(start_minutes).filter(|&m| m > 0),
            margin_minutes: Some(margin_minutes).filter(|&m| m > 0),
        };
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json + "\n")?;
        fs::rename(&tmp, &self.path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jp-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_when_there_is_no_file() {
        let d = dir("defaults");
        assert_eq!(FILE_NAME, "remora.json");
        let c = Config::load(&d.join(FILE_NAME)).unwrap();
        assert_eq!(c.jellyfin_dir(), d);
        assert_eq!(c.idle(), IdleConfig { start_window: Duration::from_secs(1800), margin: Duration::from_secs(1800) });
    }

    #[test]
    fn the_file_overrides_and_a_relative_folder_is_beside_it() {
        let d = dir("override");
        fs::write(d.join(FILE_NAME), r#"{"jellyfin_dir": "server", "idle_stop": {"start_minutes": 60, "margin_minutes": 15}}"#).unwrap();
        let c = Config::load(&d.join(FILE_NAME)).unwrap();
        assert_eq!(c.jellyfin_dir(), d.join("server"));
        assert_eq!(c.idle(), IdleConfig { start_window: Duration::from_secs(3600), margin: Duration::from_secs(900) });
    }

    #[test]
    fn idle_stop_can_be_turned_off() {
        let d = dir("off");
        fs::write(d.join(FILE_NAME), r#"{"idle_stop": {"off": true}}"#).unwrap();
        assert_eq!(Config::load(&d.join(FILE_NAME)).unwrap().idle(), IdleConfig::default());
    }

    #[test]
    fn a_broken_file_is_an_error() {
        let d = dir("broken");
        fs::write(d.join(FILE_NAME), r#"{"jellyfin_dir": "#).unwrap();
        assert!(Config::load(&d.join(FILE_NAME)).is_err());
    }

    #[test]
    fn approving_a_zone_is_remembered_once_and_keeps_a_relative_folder_relative() {
        let d = dir("zone");
        fs::write(d.join(FILE_NAME), r#"{"jellyfin_dir": "server"}"#).unwrap();
        let mut c = Config::load(&d.join(FILE_NAME)).unwrap();
        assert!(!c.zone_approved("home"));
        c.approve_zone("home").unwrap();
        c.approve_zone("home").unwrap();
        let again = Config::load(&d.join(FILE_NAME)).unwrap();
        assert!(again.zone_approved("home"));
        assert_eq!(again.approved_zones, ["home"]);
        assert_eq!(again.jellyfin_dir.as_deref(), Some("server"));
    }

    #[test]
    fn auto_stop_settings_are_saved() {
        let d = dir("idle-save");
        fs::write(d.join(FILE_NAME), r#"{"jellyfin_dir": "server"}"#).unwrap();
        let mut c = Config::load(&d.join(FILE_NAME)).unwrap();
        c.set_idle(false, 45, 20).unwrap();
        let again = Config::load(&d.join(FILE_NAME)).unwrap();
        assert_eq!(again.idle(), IdleConfig { start_window: Duration::from_secs(45 * 60), margin: Duration::from_secs(20 * 60) });
        assert_eq!(again.jellyfin_dir.as_deref(), Some("server"));
        c.set_idle(true, 45, 20).unwrap();
        assert_eq!(Config::load(&d.join(FILE_NAME)).unwrap().idle(), IdleConfig::default());
    }

    #[test]
    fn the_public_zone_can_never_be_approved() {
        let d = dir("public");
        let mut c = Config::load(&d.join(FILE_NAME)).unwrap();
        assert!(c.approve_zone("public").is_err());
        assert!(!c.zone_approved("public"));
    }
}
