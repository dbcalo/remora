//! Linux (and anything not Windows): no Credential Manager, and mode bits.

use std::io;
use std::path::Path;
use std::process::Command;

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "Credential Manager is Windows-only")
}

pub fn cred_read(_target: &str) -> io::Result<Option<String>> {
    Err(unsupported())
}

pub fn cred_write(_target: &str, _user: &str, _secret: &str) -> io::Result<()> {
    Err(unsupported())
}

pub fn cred_delete(_target: &str) -> io::Result<()> {
    Err(unsupported())
}

/// 0600 for the key file, 0700 for its directory: the usual practice for a Linux
/// secret.
pub fn make_private(path: &Path, is_dir: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(if is_dir { 0o700 } else { 0o600 }))
}

/// The Windows hold does not exist here; Linux holds through logind
/// (`platform::awake`).
pub fn execution_hold() -> Box<dyn FnMut(std::time::Duration) -> bool + Send> {
    Box::new(|_| false)
}

/// Nothing to hide off Windows.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_manager_is_unsupported_off_windows() {
        assert_eq!(cred_read("x").unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(cred_write("x", "u", "s").unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(cred_delete("x").unwrap_err().kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn private_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("jp-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("k");
        std::fs::write(&file, "k").unwrap();
        make_private(&dir, true).unwrap();
        make_private(&file, false).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
