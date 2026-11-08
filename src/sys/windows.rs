//! Windows: Credential Manager through advapi32, and commands without a console.

use std::io;
use std::path::Path;
use std::process::Command;
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The generic credential's secret, or None when there is none. Stored as
/// UTF-16, as the PowerShell and Go launchers stored it, so their key still reads.
pub fn cred_read(target: &str) -> io::Result<Option<String>> {
    let t = wide(target);
    let mut p: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: t is NUL-terminated; p receives a CREDENTIALW that CredFree frees.
    let ok = unsafe { CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) };
    if ok == 0 {
        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(ERROR_NOT_FOUND as i32) { Ok(None) } else { Err(err) };
    }
    // SAFETY: CredReadW succeeded, so p is a valid credential until CredFree.
    let secret = unsafe {
        let c = &*p;
        let n = (c.CredentialBlobSize / 2) as usize;
        let s = if n == 0 || c.CredentialBlob.is_null() {
            None
        } else {
            let u = std::slice::from_raw_parts(c.CredentialBlob as *const u16, n);
            Some(String::from_utf16_lossy(u))
        };
        CredFree(p as *const _);
        s
    };
    Ok(secret)
}

/// Creates or replaces the generic credential, for this user on this machine
/// only (never roamed).
pub fn cred_write(target: &str, user: &str, secret: &str) -> io::Result<()> {
    if secret.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty secret"));
    }
    let mut t = wide(target);
    let mut u = wide(user);
    let blob: Vec<u16> = secret.encode_utf16().collect();
    let c = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: t.as_mut_ptr(),
        Comment: ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() },
        CredentialBlobSize: (blob.len() * 2) as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: ptr::null_mut(),
        TargetAlias: ptr::null_mut(),
        UserName: u.as_mut_ptr(),
    };
    // SAFETY: every pointer in c outlives the call.
    if unsafe { CredWriteW(&c, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the credential. None there is not an error.
pub fn cred_delete(target: &str) -> io::Result<()> {
    let t = wide(target);
    // SAFETY: t is NUL-terminated.
    if unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
            return Err(err);
        }
    }
    Ok(())
}

/// Windows has no mode bits; the key there lives in Credential Manager.
pub fn make_private(_path: &Path, _is_dir: bool) -> io::Result<()> {
    Ok(())
}

/// CREATE_NO_WINDOW: the launcher has no console, so every command it runs
/// (netsh, powershell, reg) would otherwise flash one.
/// The built-in Windows hold. SetThreadExecutionState is per thread, so one
/// thread owns the request for the program's life: it takes it while a renewal
/// is live and drops it when the renewal runs out (`decide::hold_step`). The
/// display is never held. The request ends with the process too.
pub fn execution_hold() -> Box<dyn FnMut(std::time::Duration) -> bool + Send> {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    let (tx, rx) = mpsc::channel::<Instant>();
    std::thread::spawn(move || {
        let (mut held, mut until) = (false, None);
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(u) => until = Some(u),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if let Some(take) = crate::decide::hold_step(held, until, Instant::now()) {
                // SAFETY: a plain call on this thread, which owns the request.
                unsafe { SetThreadExecutionState(if take { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS }) };
                held = take;
            }
        }
    });
    Box::new(move |d| {
        tx.send(Instant::now() + d).is_ok()
    })
}

pub fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target unique to this run, deleted afterwards: the real Credential
    /// Manager is used, never the launcher's own entry.
    fn target() -> String {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        format!("remora-test/{}-{n}", std::process::id())
    }

    #[test]
    fn credential_round_trip() {
        let t = target();
        assert_eq!(cred_read(&t).unwrap(), None);
        cred_write(&t, "jellyfin", "0123456789abcdef-Zoë").unwrap();
        assert_eq!(cred_read(&t).unwrap().as_deref(), Some("0123456789abcdef-Zoë"));
        cred_write(&t, "jellyfin", "replaced").unwrap();
        assert_eq!(cred_read(&t).unwrap().as_deref(), Some("replaced"));
        cred_delete(&t).unwrap();
        assert_eq!(cred_read(&t).unwrap(), None);
        cred_delete(&t).unwrap();
    }

    #[test]
    fn an_empty_secret_is_refused() {
        let t = target();
        assert!(cred_write(&t, "jellyfin", "").is_err());
        cred_delete(&t).unwrap();
    }
}
