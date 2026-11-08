//! Moves an install made under the program's old name, "Jellyfin Portable"
//! (until 2026-11-08), to Remora's names on first start, so nobody sets up or
//! signs in again: the settings file, the stored key, and the standby marker.
//! The old Caffeine claim is left to expire by itself: Caffeine is reached
//! through its command line only, which has no release.

use std::path::{Path, PathBuf};

use crate::keystore::Store;

/// Where each moved thing was, and where it goes.
pub struct Moves {
    pub old_config: PathBuf,
    pub new_config: PathBuf,
    pub old_marker: PathBuf,
    pub new_marker: PathBuf,
}

/// Moves what exists under the old names and is not already there under the
/// new ones; never overwrites, never fails. Returns one sentence per thing moved.
pub fn migrate(m: &Moves, old_keys: &dyn Store, new_keys: &dyn Store) -> Vec<String> {
    let mut said = vec![];
    if move_file(&m.old_config, &m.new_config) {
        said.push("Moved the settings to Remora's name.".to_string());
    }
    if let (Ok(None), Ok(Some(key))) = (new_keys.get(), old_keys.get())
        && new_keys.set(&key).is_ok()
    {
        let _ = old_keys.delete();
        said.push("Moved this machine's Jellyfin key to Remora's name.".to_string());
    }
    if move_file(&m.old_marker, &m.new_marker) {
        said.push("Moved the standby setting's note to Remora's name.".to_string());
    }
    said
}

/// Renames `from` to `to` when `from` exists and `to` does not.
fn move_file(from: &Path, to: &Path) -> bool {
    if !from.exists() || to.exists() {
        return false;
    }
    if let Some(dir) = to.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::rename(from, to).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;

    struct Mem(Mutex<Option<String>>, bool);

    impl Store for Mem {
        fn get(&self) -> io::Result<Option<String>> {
            if self.1 {
                return Err(io::Error::other("unreadable"));
            }
            Ok(self.0.lock().unwrap().clone())
        }
        fn set(&self, k: &str) -> io::Result<()> {
            *self.0.lock().unwrap() = Some(k.into());
            Ok(())
        }
        fn delete(&self) -> io::Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    fn mem(k: Option<&str>) -> Mem {
        Mem(Mutex::new(k.map(String::from)), false)
    }

    fn moves(name: &str) -> Moves {
        let d = std::env::temp_dir().join(format!("remora-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("old")).unwrap();
        Moves {
            old_config: d.join("jellyfin-portable.json"),
            new_config: d.join("remora.json"),
            old_marker: d.join("old").join("standby-network.txt"),
            new_marker: d.join("new").join("standby-network.txt"),
        }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn moves_the_settings_key_and_marker() {
        let m = moves("all");
        std::fs::write(&m.old_config, "{}").unwrap();
        std::fs::write(&m.old_marker, "2").unwrap();
        let (old, new) = (mem(Some("k")), mem(None));
        let said = migrate(&m, &old, &new);
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(read(&m.new_config), "{}");
        assert_eq!(read(&m.new_marker), "2");
        assert!(!m.old_config.exists() && !m.old_marker.exists());
        assert_eq!(new.get().unwrap().as_deref(), Some("k"));
        assert_eq!(old.get().unwrap(), None);
    }

    #[test]
    fn never_overwrites_what_remora_already_has() {
        let m = moves("keep");
        std::fs::write(&m.old_config, "old").unwrap();
        std::fs::write(&m.new_config, "new").unwrap();
        let (old, new) = (mem(Some("old-key")), mem(Some("new-key")));
        assert!(migrate(&m, &old, &new).is_empty());
        assert_eq!(read(&m.new_config), "new");
        assert!(m.old_config.exists(), "the old one stays where it was");
        assert_eq!(new.get().unwrap().as_deref(), Some("new-key"));
        assert_eq!(old.get().unwrap().as_deref(), Some("old-key"));
    }

    #[test]
    fn an_unreadable_old_key_is_skipped() {
        let m = moves("unreadable");
        std::fs::write(&m.old_config, "{}").unwrap();
        let old = Mem(Mutex::new(None), true);
        let new = mem(None);
        let said = migrate(&m, &old, &new);
        assert_eq!(said.len(), 1, "the settings still move: {said:?}");
        assert_eq!(new.get().unwrap(), None);
    }

    #[test]
    fn nothing_old_means_nothing_done() {
        assert!(migrate(&moves("none"), &mem(None), &mem(None)).is_empty());
    }
}
