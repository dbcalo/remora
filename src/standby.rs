//! Keeps Wi-Fi connected in Modern Standby, on battery, only while Jellyfin
//! runs. A Modern Standby laptop (S0 low power idle, as most current laptops) goes
//! into standby when its screen times out, whatever Caffeine holds; on battery
//! Windows then manages the network itself and cut it an hour into a film
//! (2026-11-07, "Adaptive Connected Standby"). The power plan's "Networking
//! connectivity in Standby" setting, on battery, is set to Enable while the
//! server runs and put back afterwards, so real sleep still drops the Wi-Fi.
//!
//! The value to put back is written to a file BEFORE the setting changes, so a
//! crash leaves it behind and the next launch restores it. No administrator
//! rights are needed: powercfg writes the user's own plan.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::decide::Os;

/// Runs argv and returns its exit code and stdout.
pub type Runner = Arc<dyn Fn(&[&str]) -> (Option<i32>, String) + Send + Sync>;

/// The setting's index for Enable.
pub const ENABLE: u32 = 1;

/// The battery ("DC") index in `powercfg /qh SCHEME_CURRENT SUB_NONE
/// CONNECTIVITYINSTANDBY`. None when the line is absent: a machine without
/// Modern Standby has no such setting.
pub fn parse_dc_index(out: &str) -> Option<u32> {
    let line = out.lines().find(|l| l.contains("Current DC Power Setting Index:"))?;
    let hex = line.rsplit(':').next()?.trim().strip_prefix("0x")?;
    u32::from_str_radix(hex, 16).ok()
}

const SETTING: [&str; 3] = ["SCHEME_CURRENT", "SUB_NONE", "CONNECTIVITYINSTANDBY"];

/// Sets the battery value and makes the plan take it. True when both worked.
fn set(run: &Runner, index: u32) -> bool {
    let i = index.to_string();
    let ok = run(&["powercfg", "/setdcvalueindex", SETTING[0], SETTING[1], SETTING[2], &i]).0 == Some(0);
    ok && run(&["powercfg", "/setactive", SETTING[0]]).0 == Some(0)
}

/// Puts the setting back while it lives; dropping it restores.
pub struct Guard {
    marker: PathBuf,
    original: u32,
    run: Runner,
}

/// Switches the battery setting to Enable, if this is Windows, the setting
/// exists and is not Enable already. Returns what restores it.
pub fn engage(os: Os, marker: &Path, run: Runner) -> Option<Guard> {
    if os != Os::Windows {
        return None;
    }
    let original = parse_dc_index(&run(&["powercfg", "/qh", SETTING[0], SETTING[1], SETTING[2]]).1)?;
    if original == ENABLE {
        return None;
    }
    // On disk first: a crash after the switch must leave the way back behind.
    std::fs::create_dir_all(marker.parent()?).ok()?;
    std::fs::write(marker, original.to_string()).ok()?;
    if !set(&run, ENABLE) {
        set(&run, original); // half-applied: put it back as it was
        let _ = std::fs::remove_file(marker);
        return None;
    }
    Some(Guard { marker: marker.to_path_buf(), original, run })
}

/// Restores a value a crashed launcher left behind. Called at launch, after
/// making sure no other launcher is running (its value would be live).
pub fn recover(marker: &Path, run: &Runner) {
    let Some(original) = std::fs::read_to_string(marker).ok().and_then(|s| s.trim().parse().ok()) else {
        return;
    };
    if set(run, original) {
        let _ = std::fs::remove_file(marker);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Kept on failure, so the next launch tries again.
        if set(&self.run, self.original) {
            let _ = std::fs::remove_file(&self.marker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const QH: &str = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)
  GUID Alias: SCHEME_BALANCED
  Subgroup GUID: fea3413e-7e05-4911-9a71-700331f1c294  (Settings belonging to no subgroup)
    GUID Alias: SUB_NONE
    Power Setting GUID: f15576e8-98b7-4186-b944-eafa664402d9  (Networking connectivity in Standby)
      GUID Alias: CONNECTIVITYINSTANDBY
      Possible Setting Index: 000
      Possible Setting Friendly Name: Disable
      Possible Setting Index: 001
      Possible Setting Friendly Name: Enable
      Possible Setting Index: 002
      Possible Setting Friendly Name: Managed by Windows
    Current AC Power Setting Index: 0x00000001
    Current DC Power Setting Index: 0x00000002
";
    const NO_SETTING: &str = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\n  GUID Alias: SCHEME_BALANCED\n";

    /// A fake powercfg: answers queries with `dc`, records every call.
    fn fake(dc: &'static str) -> (Runner, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let c = calls.clone();
        let run: Runner = Arc::new(move |argv: &[&str]| {
            c.lock().unwrap().push(argv.join(" "));
            let out = if argv.contains(&"/qh") { dc.to_string() } else { String::new() };
            (Some(0), out)
        });
        (run, calls)
    }

    fn marker(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jp-standby-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("standby-network.json")
    }

    fn sets(calls: &[String]) -> Vec<String> {
        calls.iter().filter(|c| !c.contains("/qh")).cloned().collect()
    }

    #[test]
    fn reads_the_battery_value() {
        assert_eq!(parse_dc_index(QH), Some(2));
        assert_eq!(parse_dc_index(&QH.replace("0x00000002", "0x00000000")), Some(0));
        assert_eq!(parse_dc_index(NO_SETTING), None);
        assert_eq!(parse_dc_index("Current DC Power Setting Index: junk"), None);
    }

    #[test]
    fn enables_while_running_and_restores_on_drop() {
        let (run, calls) = fake(QH);
        let m = marker("cycle");
        let g = engage(Os::Windows, &m, run).expect("engaged");
        assert!(m.exists(), "the value to restore is on disk while engaged");
        assert_eq!(
            sets(&calls.lock().unwrap()),
            [
                "powercfg /setdcvalueindex SCHEME_CURRENT SUB_NONE CONNECTIVITYINSTANDBY 1",
                "powercfg /setactive SCHEME_CURRENT"
            ]
        );
        calls.lock().unwrap().clear();
        drop(g);
        assert_eq!(
            sets(&calls.lock().unwrap()),
            [
                "powercfg /setdcvalueindex SCHEME_CURRENT SUB_NONE CONNECTIVITYINSTANDBY 2",
                "powercfg /setactive SCHEME_CURRENT"
            ]
        );
        assert!(!m.exists(), "nothing left to recover after a clean stop");
    }

    #[test]
    fn already_enabled_changes_nothing() {
        let (run, calls) = fake("Current DC Power Setting Index: 0x00000001\n");
        let m = marker("already");
        assert!(engage(Os::Windows, &m, run).is_none());
        assert!(sets(&calls.lock().unwrap()).is_empty());
        assert!(!m.exists());
    }

    #[test]
    fn no_modern_standby_changes_nothing() {
        let (run, calls) = fake(NO_SETTING);
        assert!(engage(Os::Windows, &marker("none"), run).is_none());
        assert!(sets(&calls.lock().unwrap()).is_empty());
    }

    #[test]
    fn linux_runs_nothing() {
        let (run, calls) = fake(QH);
        assert!(engage(Os::Linux, &marker("linux"), run).is_none());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_crash_is_recovered_at_the_next_launch() {
        let (run, calls) = fake(QH);
        let m = marker("crash");
        std::mem::forget(engage(Os::Windows, &m, run.clone()).unwrap()); // no Drop: a crash
        calls.lock().unwrap().clear();
        recover(&m, &run);
        assert_eq!(
            sets(&calls.lock().unwrap()),
            [
                "powercfg /setdcvalueindex SCHEME_CURRENT SUB_NONE CONNECTIVITYINSTANDBY 2",
                "powercfg /setactive SCHEME_CURRENT"
            ]
        );
        assert!(!m.exists());
    }

    #[test]
    fn recover_without_a_leftover_runs_nothing() {
        let (run, calls) = fake(QH);
        recover(&marker("clean"), &run);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_switch_leaves_nothing_to_restore() {
        let calls = Arc::new(Mutex::new(vec![]));
        let c = calls.clone();
        let run: Runner = Arc::new(move |argv: &[&str]| {
            c.lock().unwrap().push(argv.join(" "));
            if argv.contains(&"/qh") { (Some(0), QH.into()) } else { (Some(1), String::new()) }
        });
        let m = marker("fail");
        assert!(engage(Os::Windows, &m, run).is_none());
        assert!(!m.exists());
    }
}
