//! This install's Jellyfin API key, and nothing else: the admin password is never
//! stored anywhere.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::decide::Os;
use crate::sys;

/// Holds one secret.
pub trait Store: Send {
    fn get(&self) -> io::Result<Option<String>>;
    fn set(&self, key: &str) -> io::Result<()>;
    fn delete(&self) -> io::Result<()>;
}

/// The Credential Manager entry.
pub const CRED_TARGET: &str = "remora/api-key";

/// The store for an OS: Credential Manager on Windows; on Linux a private file
/// under $XDG_CONFIG_HOME, because the Secret Service needs D-Bus and SteamOS has
/// no secret-tool.
pub fn for_os(os: Os, xdg_config_home: Option<&str>, home: &Path) -> Box<dyn Store> {
    store(os, CRED_TARGET, &file_path(xdg_config_home, home))
}

/// The store the program used before it was Remora, read only by `migrate`.
pub fn old_for_os(os: Os, xdg_config_home: Option<&str>, home: &Path) -> Box<dyn Store> {
    store(os, &format!("{}/api-key", crate::decide::OLD_APP_ID), &old_file_path(xdg_config_home, home))
}

fn old_file_path(xdg_config_home: Option<&str>, home: &Path) -> PathBuf {
    let new = file_path(xdg_config_home, home);
    new.parent().and_then(Path::parent).unwrap_or(Path::new("")).join(crate::decide::OLD_APP_ID).join("api-key")
}

fn store(os: Os, target: &str, path: &Path) -> Box<dyn Store> {
    match os {
        Os::Windows => Box::new(CredStore { target: target.into() }),
        Os::Linux => Box::new(FileStore { path: path.to_path_buf() }),
    }
}

fn file_path(xdg_config_home: Option<&str>, home: &Path) -> PathBuf {
    let base = match xdg_config_home {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => home.join(".config"),
    };
    base.join(crate::decide::APP_ID).join("api-key")
}

/// Windows Credential Manager.
pub struct CredStore {
    pub target: String,
}

impl Store for CredStore {
    fn get(&self) -> io::Result<Option<String>> {
        sys::cred_read(&self.target)
    }
    fn set(&self, key: &str) -> io::Result<()> {
        sys::cred_write(&self.target, crate::decide::APP_ID, key)
    }
    fn delete(&self) -> io::Result<()> {
        sys::cred_delete(&self.target)
    }
}

/// One file, mode 0600 in a 0700 directory, written through a rename so a crash
/// leaves the old key or the new one, never half of either.
pub struct FileStore {
    pub path: PathBuf,
}

impl Store for FileStore {
    fn get(&self) -> io::Result<Option<String>> {
        match fs::read_to_string(&self.path) {
            Ok(s) => Ok(Some(s.trim().to_string()).filter(|k| !k.is_empty())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn set(&self, key: &str) -> io::Result<()> {
        let dir = self.path.parent().ok_or_else(|| io::Error::other("no parent directory"))?;
        fs::create_dir_all(dir)?;
        sys::make_private(dir, true)?;
        let tmp = dir.join(format!(".api-key-{}.tmp", std::process::id()));
        let result = (|| {
            fs::write(&tmp, format!("{key}\n"))?;
            sys::make_private(&tmp, false)?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    fn delete(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jp-keystore-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn file_store_round_trip() {
        let dir = temp("rt");
        let s = FileStore { path: dir.join("cfg").join("remora").join("api-key") };
        assert_eq!(s.get().unwrap(), None);
        s.set("abc123").unwrap();
        assert_eq!(s.get().unwrap().as_deref(), Some("abc123"));
        s.delete().unwrap();
        assert_eq!(s.get().unwrap(), None);
        s.delete().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_store_ignores_surrounding_whitespace() {
        let dir = temp("ws");
        fs::create_dir_all(&dir).unwrap();
        let s = FileStore { path: dir.join("k") };
        fs::write(&s.path, "abc123\n").unwrap();
        assert_eq!(s.get().unwrap().as_deref(), Some("abc123"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_linux_path_follows_xdg_config_home() {
        let home = Path::new("/home/u");
        assert_eq!(file_path(Some("/xdg"), home), Path::new("/xdg").join("remora").join("api-key"));
        assert_eq!(file_path(None, home), home.join(".config").join("remora").join("api-key"));
        assert_eq!(file_path(Some(""), home), home.join(".config").join("remora").join("api-key"));
        assert_eq!(CRED_TARGET, "remora/api-key");
        assert_eq!(old_file_path(Some("/xdg"), home), Path::new("/xdg").join("jellyfin-portable").join("api-key"));
        assert_eq!(old_file_path(None, home), home.join(".config").join("jellyfin-portable").join("api-key"));
    }
}
