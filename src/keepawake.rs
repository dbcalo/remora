//! Holds the machine awake through Caffeine's command line, per its integration
//! contract (Caffeine Party's docs/contract.md, version 2). It never writes
//! Caffeine's files: on Windows only the program can start the helper that
//! actually holds the machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::decide::{self, Keep, Os};
use crate::sys;

/// The name Caffeine shows on its row: "Remora is holding the machine awake".
pub const HOLDER: &str = crate::decide::APP_ID;

/// Runs argv and returns its exit code, None if it could not run.
pub type Runner = Box<dyn FnMut(&[String]) -> Option<i32> + Send>;

pub struct Caffeine {
    pub state_dir: PathBuf,
    pub os: Os,
    pub run: Runner,
    /// A Caffeine without `hold`: use the hook from then on.
    use_hook: bool,
}

/// Caffeine's state directory, resolved as Caffeine resolves it:
/// CAFFEINE_LEASE_FILE's directory when set, else ~/.caffeine on every OS.
pub fn state_dir(lease_override: Option<&str>, home: &Path) -> PathBuf {
    match lease_override {
        Some(f) if !f.is_empty() => Path::new(f).parent().map(Path::to_path_buf).unwrap_or_default(),
        _ => home.join(".caffeine"),
    }
}

impl Caffeine {
    pub fn new(state_dir: PathBuf, os: Os, run: Runner) -> Caffeine {
        Caffeine { state_dir, os, run, use_hook: false }
    }

    fn exe(&self) -> String {
        let name = if self.os == Os::Windows { "caffeine.exe" } else { "caffeine" };
        self.state_dir.join("bin").join(name).to_string_lossy().into_owned()
    }

    /// Renews this machine's hold under the name Jellyfin. With no state
    /// directory Caffeine is not installed, and nothing is run or created.
    ///
    /// A Caffeine older than contract 2 answers `hold` with 64; the hook holds
    /// the same way (its row says "agent", which is why it is only the
    /// fallback), and is used from then on.
    pub fn hold(&mut self) -> Keep {
        if !self.state_dir.is_dir() {
            return Keep::NotInstalled;
        }
        let exe = self.exe();
        if !self.use_hook {
            let keep = decide::keep_from_exit((self.run)(&[exe.clone(), "hold".into(), "--holder".into(), HOLDER.into()]));
            if keep != Keep::TooOld {
                return keep;
            }
            self.use_hook = true;
        }
        (self.run)(&[exe, "hook".into()]); // exits 0 whatever happens, by design
        Keep::TooOld
    }
}

/// Holds the machine for a while; the platform's own way (`platform::awake`).
/// Returns whether it is held.
pub type Hold = Box<dyn FnMut(std::time::Duration) -> bool + Send>;

/// Caffeine when it is installed, else the built-in hold (decided 2026-11-08).
/// A Caffeine
/// that turns out not to be installed (exit 4) hands over for good.
pub struct Auto {
    caffeine: Option<Caffeine>,
    builtin: Hold,
}

impl Auto {
    pub fn new(caffeine: Caffeine, builtin: Hold) -> Auto {
        let caffeine = caffeine.state_dir.is_dir().then_some(caffeine);
        Auto { caffeine, builtin }
    }

    /// Renews the hold for `decide::CAFFEINE_GRACE`.
    pub fn hold(&mut self) -> Keep {
        if let Some(c) = self.caffeine.as_mut() {
            match c.hold() {
                Keep::NotInstalled => self.caffeine = None,
                // A leftover folder without the program: hold ourselves this
                // renewal, and try Caffeine again on the next.
                Keep::Failed => {}
                k => return k,
            }
        }
        if (self.builtin)(decide::CAFFEINE_GRACE) { Keep::Held } else { Keep::Failed }
    }
}

/// The real runner: no window, no streams, the exit code.
pub fn exec(argv: &[String]) -> Option<i32> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    sys::no_window(&mut cmd).status().ok().and_then(|s| s.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recorder(exits: Vec<Option<i32>>) -> (Runner, Arc<Mutex<Vec<Vec<String>>>>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let c = calls.clone();
        let run: Runner = Box::new(move |argv: &[String]| {
            let mut c = c.lock().unwrap();
            c.push(argv.to_vec());
            *exits.get(c.len() - 1).unwrap_or(exits.last().unwrap())
        });
        (run, calls)
    }

    fn installed(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jp-caffeine-{name}-{}", std::process::id())).join(".caffeine");
        std::fs::create_dir_all(d.join("bin")).unwrap();
        d
    }

    #[test]
    fn hold_names_remora() {
        let (run, calls) = recorder(vec![Some(0)]);
        let dir = installed("names");
        let mut c = Caffeine::new(dir.clone(), Os::Linux, run);
        assert_eq!(c.hold(), Keep::Held);
        let want = vec![dir.join("bin").join("caffeine").to_string_lossy().into_owned(), "hold".into(), "--holder".into(), "remora".into()];
        assert_eq!(*calls.lock().unwrap(), vec![want]);
    }

    #[test]
    fn hold_uses_the_exe_on_windows() {
        let (run, calls) = recorder(vec![Some(0)]);
        let dir = installed("exe");
        let mut c = Caffeine::new(dir.clone(), Os::Windows, run);
        c.hold();
        assert_eq!(calls.lock().unwrap()[0][0], dir.join("bin").join("caffeine.exe").to_string_lossy());
    }

    #[test]
    fn an_old_caffeine_falls_back_to_the_hook_and_keeps_using_it() {
        let (run, calls) = recorder(vec![Some(64), Some(0)]);
        let mut c = Caffeine::new(installed("old"), Os::Linux, run);
        assert_eq!(c.hold(), Keep::TooOld);
        c.hold();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1][1], "hook");
        assert_eq!(calls[2][1], "hook");
    }

    #[test]
    fn not_installed_runs_nothing() {
        let (run, calls) = recorder(vec![Some(0)]);
        let mut c = Caffeine::new(std::env::temp_dir().join("jp-absent-caffeine-dir"), Os::Linux, run);
        assert_eq!(c.hold(), Keep::NotInstalled);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn paused_and_failed_pass_through() {
        for (code, want) in [(Some(3), Keep::Paused), (Some(1), Keep::Failed), (None, Keep::Failed)] {
            let (run, _) = recorder(vec![code]);
            let mut c = Caffeine::new(installed("pass"), Os::Linux, run);
            assert_eq!(c.hold(), want);
        }
    }

    fn builtin() -> (Hold, Arc<Mutex<Vec<std::time::Duration>>>) {
        let held = Arc::new(Mutex::new(vec![]));
        let h = held.clone();
        (
            Box::new(move |d| {
                h.lock().unwrap().push(d);
                true
            }),
            held,
        )
    }

    #[test]
    fn caffeine_when_installed_builtin_otherwise() {
        let (run, calls) = recorder(vec![Some(0)]);
        let (hold, held) = builtin();
        let mut a = Auto::new(Caffeine::new(installed("auto-yes"), Os::Linux, run), hold);
        assert_eq!(a.hold(), Keep::Held);
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(held.lock().unwrap().is_empty(), "Caffeine holds; the built-in stays out");

        let (run, calls) = recorder(vec![Some(0)]);
        let (hold, held) = builtin();
        let mut a = Auto::new(Caffeine::new(std::env::temp_dir().join("remora-no-caffeine"), Os::Linux, run), hold);
        assert_eq!(a.hold(), Keep::Held);
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(*held.lock().unwrap(), vec![crate::decide::CAFFEINE_GRACE]);
    }

    #[test]
    fn a_caffeine_that_says_not_installed_hands_over() {
        let (run, calls) = recorder(vec![Some(4)]);
        let (hold, held) = builtin();
        let mut a = Auto::new(Caffeine::new(installed("auto-gone"), Os::Linux, run), hold);
        assert_eq!(a.hold(), Keep::Held);
        assert_eq!(a.hold(), Keep::Held);
        assert_eq!(calls.lock().unwrap().len(), 1, "Caffeine is not asked again");
        assert_eq!(held.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_failing_caffeine_falls_back_to_the_builtin_hold() {
        let (run, calls) = recorder(vec![None]);
        let (hold, held) = builtin();
        let mut a = Auto::new(Caffeine::new(installed("auto-failing"), Os::Linux, run), hold);
        assert_eq!(a.hold(), Keep::Held);
        assert_eq!(held.lock().unwrap().len(), 1, "the built-in hold ran once");
        assert_eq!(a.hold(), Keep::Held);
        assert_eq!(calls.lock().unwrap().len(), 2, "Caffeine is still tried on the next renewal");
    }

    #[test]
    fn a_builtin_hold_that_fails_says_failed() {
        let (run, calls) = recorder(vec![Some(0)]);
        let mut a = Auto::new(Caffeine::new(std::env::temp_dir().join("remora-no-caffeine"), Os::Linux, run), Box::new(|_| false));
        assert_eq!(a.hold(), Keep::Failed);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn state_dir_follows_caffeines_override() {
        let home = Path::new("/home/u");
        assert_eq!(state_dir(Some("/x/y/lease.json"), home), Path::new("/x/y"));
        assert_eq!(state_dir(None, home), home.join(".caffeine"));
    }
}
