//! Remora: starts a portable Jellyfin, shows one window with the
//! address to type into the TV, holds the machine awake through Caffeine while
//! something plays, and stops the server when it is closed or goes unused.

// No console window on Windows: the program window is the running indicator.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod config;
mod controller;
mod decide;
mod jellyfin;
mod keepawake;
mod keystore;
mod migrate;
mod platform;
mod standby;
mod sys;
mod view;
#[cfg(test)]
mod testserver;
mod ui;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::app::{App, Deps};
use crate::config::Config;
use crate::controller::{Controller, PlatState};
use crate::decide::Os;

const POLL_EVERY: Duration = Duration::from_secs(30);

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let os = sys::os();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Without Windows' \\?\ prefix, which canonicalize adds: every path below is
    // built from this one, and Jellyfin dies at startup on a \\?\ --datadir
    // ("Drive name must be a root directory"), found on a first run in testing.
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => PathBuf::from(plain_path(&p)),
        Err(e) => return error_window(&format!("Cannot find this program's own path: {e}")),
    };
    let exe_str = plain_path(&exe);
    let cfg_path = exe.parent().unwrap_or(Path::new(".")).join(config::FILE_NAME);
    let home = home_dir(os);
    // Moves an install made under the old name to Remora's names, before the
    // settings are read. Never in the elevated firewall step, which runs as
    // administrator and must not touch the user's key store.
    let moved = if args.first().map(String::as_str) == Some(platform::ELEVATED_FLAG) {
        vec![]
    } else {
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let moves = migrate::Moves {
            old_config: exe.parent().unwrap_or(Path::new(".")).join(format!("{}.json", decide::OLD_APP_ID)),
            new_config: cfg_path.clone(),
            old_marker: state_dir(os, &home, decide::OLD_APP_ID).join(STANDBY_MARKER),
            new_marker: state_dir(os, &home, decide::APP_ID).join(STANDBY_MARKER),
        };
        migrate::migrate(&moves, &*keystore::old_for_os(os, xdg.as_deref(), &home), &*keystore::for_os(os, xdg.as_deref(), &home))
    };
    let cfg = match Config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => return error_window(&format!("{}: {e}", cfg_path.display())),
    };
    let jf_dir = cfg.jellyfin_dir();
    let jf_argv = decide::jellyfin_argv(os, &jf_dir);

    // The elevated firewall step: a re-entry of this binary through UAC.
    if args.first().map(String::as_str) == Some(platform::ELEVATED_FLAG) {
        return match platform::parse_elevated_args(&args[1..]) {
            None => 64,
            Some(ids) => match platform::run_elevated(&plain_path(&jf_argv[0]), &ids) {
                Ok(()) => 0,
                Err(_) => 1,
            },
        };
    }
    if !jf_argv[0].exists() {
        return error_window(&format!(
            "No Jellyfin at {}. Set jellyfin_dir in {}.",
            jf_argv[0].display(),
            cfg_path.display()
        ));
    }

    let running_path = running_path(os, &home);
    if signal_running(&running_path) {
        return 0; // the first launcher's window has been raised
    }
    // Only now: while another launcher runs, a value on disk is its live one.
    let standby_marker = running_path.with_file_name(STANDBY_MARKER);
    let powercfg: standby::Runner = Arc::new(platform::run);
    standby::recover(&standby_marker, &powercfg);

    let game = args.iter().any(|a| a == "--fullscreen") || decide::game_mode(|k| std::env::var(k).ok());
    let start_time = Instant::now();
    let client = jellyfin::Client::new(&format!("http://127.0.0.1:{}", decide::HTTP_PORT), &hostname(os));
    let local_url = client.base.clone();
    // Caffeine when it is installed, else the built-in hold.
    let mut caffeine = keepawake::Auto::new(
        keepawake::Caffeine::new(
            keepawake::state_dir(std::env::var("CAFFEINE_LEASE_FILE").ok().as_deref(), &home),
            os,
            Box::new(keepawake::exec),
        ),
        platform::awake(os),
    );
    let tv_cache: Arc<Mutex<Vec<String>>> = Arc::default();
    let tv_for_app = tv_cache.clone();
    let jf_dir_start = jf_dir.clone();
    let app = Arc::new(App::new(Deps {
        now: Box::new(move || start_time.elapsed()),
        start: Box::new(move || {
            if !platform::port_free(decide::HTTP_PORT) {
                return Err(std::io::Error::other(format!(
                    "port {} is already in use, probably by another Jellyfin; stop it first",
                    decide::HTTP_PORT
                )));
            }
            // Before Jellyfin, so a failed start drops the guard and restores.
            let standby = standby::engage(os, &standby_marker, powercfg.clone());
            let jf = platform::Jellyfin::start(os, &jf_argv, &jf_dir_start)?;
            Ok(Box::new(Served { jf, _standby: standby }) as Box<dyn app::Proc>)
        }),
        server: Box::new(client),
        keys: Mutex::new(keystore::for_os(os, std::env::var("XDG_CONFIG_HOME").ok().as_deref(), &home)),
        caffeine: Mutex::new(Box::new(move || caffeine.hold())),
        idle: cfg.idle(),
        tv: Box::new(move || tv_for_app.lock().unwrap().clone()),
        ready_timeout: Duration::from_secs(90),
        ready_poll: Duration::from_secs(1),
    }));
    let plat = Arc::new(Mutex::new(PlatState::new(os, game, exe_str.clone(), cfg)));
    let ctrl = Arc::new(Controller::new(os, app.clone(), plat.clone(), local_url));
    let quitting = Arc::new(AtomicBool::new(false));

    // A second launch asks this one to come to the front.
    let token = random_token();
    if let Ok(listener) = TcpListener::bind("127.0.0.1:0") {
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let _ = write_running(&running_path, &Running { pid: std::process::id(), port, token: token.clone() });
        let ctrl = ctrl.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                if BufReader::new(&stream).read_line(&mut line).is_ok() && line.trim() == token {
                    ctrl.request_raise();
                }
            }
        });
    }

    // The background thread: the network step, then Jellyfin, then a poll every
    // 30 seconds until the window closes.
    {
        let (app, plat, quitting, tv_cache, home) = (app.clone(), plat.clone(), quitting.clone(), tv_cache.clone(), home.clone());
        let moved = moved.clone();
        std::thread::spawn(move || {
            PlatState::refresh(&plat, &home);
            PlatState::apply_automatic(&plat, &home);
            *tv_cache.lock().unwrap() = decide::tv_addresses(&platform::ifaces(os), decide::HTTP_PORT);
            let _ = app.start(); // a failure is in the window's "why" line
            if !moved.is_empty() {
                app.note(&moved.join(" "));
            }
            let mut last = Instant::now();
            while !quitting.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                if last.elapsed() >= POLL_EVERY {
                    last = Instant::now();
                    *tv_cache.lock().unwrap() = decide::tv_addresses(&platform::ifaces(os), decide::HTTP_PORT);
                    app.tick();
                }
            }
        });
    }

    ui::run(ctrl);
    quitting.store(true, Ordering::SeqCst);
    app.stop("Quit.");
    remove_running(&running_path);
    0
}

/// The running server and the standby setting switched on for it. Fields drop
/// in order: Jellyfin stops, then the setting is put back.
struct Served {
    jf: platform::Jellyfin,
    _standby: Option<standby::Guard>,
}

impl app::Proc for Served {
    fn exited(&mut self) -> bool {
        self.jf.exited()
    }
    fn stop(&mut self) {
        self.jf.stop()
    }
}

/// Says what went wrong before the window can work, and the exit code.
fn error_window(message: &str) -> i32 {
    ui::error_window(message);
    1
}

/// A path without Windows' \\?\ prefix, which canonicalize adds and which other
/// programs (netsh, Steam) do not expect.
fn plain_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

pub(crate) fn home_dir(os: Os) -> PathBuf {
    let var = if os == Os::Windows { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).unwrap_or_default()
}

fn hostname(os: Os) -> String {
    let from_env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    match os {
        Os::Windows => from_env("COMPUTERNAME"),
        Os::Linux => std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).or_else(|| from_env("HOSTNAME")),
    }
    .unwrap_or_else(|| "unknown".into())
}

fn random_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness means no safe token; a value nothing can match.
        return String::from("\u{0}");
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// --- one launcher per machine -------------------------------------------------------------

/// Which launcher owns the server, and how to reach it. Holds a token, so it is
/// private to the user.
#[derive(Serialize, Deserialize)]
struct Running {
    pid: u32,
    port: u16,
    token: String,
}

/// The note of the standby value to restore, beside the running file.
const STANDBY_MARKER: &str = "standby-network.txt";

/// The per-user folder for a program `id`: %APPDATA%\<id>, or <config>/<id>.
fn state_dir(os: Os, home: &Path, id: &str) -> PathBuf {
    let base = match os {
        Os::Windows => std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Roaming")),
        Os::Linux => std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")),
    };
    base.join(id)
}

fn running_path(os: Os, home: &Path) -> PathBuf {
    state_dir(os, home, decide::APP_ID).join("running.json")
}

fn write_running(path: &Path, r: &Running) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no directory"))?;
    std::fs::create_dir_all(dir)?;
    sys::make_private(dir, true)?;
    std::fs::write(path, serde_json::to_vec(r).map_err(std::io::Error::other)?)?;
    sys::make_private(path, false)
}

/// Asks a running launcher to show its window. True means one answered.
fn signal_running(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let Ok(r) = serde_json::from_str::<Running>(&text) else { return false };
    let Ok(mut s) = TcpStream::connect_timeout(&([127, 0, 0, 1], r.port).into(), Duration::from_secs(2)) else {
        return false;
    };
    writeln!(s, "{}", r.token).is_ok()
}

fn remove_running(path: &Path) {
    if let Ok(text) = std::fs::read_to_string(path)
        && serde_json::from_str::<Running>(&text).is_ok_and(|r| r.pid == std::process::id())
    {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only src/sys may be platform-specific: every other module compiles and is
    /// tested the same on every OS, so one `cargo test` has nothing to skip.
    /// `cfg(test)` is the one exception.
    #[test]
    fn only_sys_is_platform_specific() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = vec![];
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "sys") {
                        stack.push(p);
                    }
                    continue;
                }
                // ui/mod.rs picks the front-end per OS; it is the other allowed place.
                if p.ends_with(Path::new("ui").join("mod.rs")) {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let l = line.trim();
                    if l.starts_with("//") {
                        continue;
                    }
                    // Built at run time, so this test's own source is not a match.
                    let marks = ["#[", "#![", ""].map(|p| format!("{p}{}", "cfg"));
                    let cfg = marks.iter().any(|m| l.contains(&format!("{m}(")) || l.contains(&format!("{m}_attr(")))
                        || l.contains(&format!("{}!(", "cfg"));
                    let allowed = l.contains("cfg(test)") || l.contains("cfg_attr(not(test)");
                    if cfg && !allowed {
                        offenders.push(format!("{}:{}: {l}", p.display(), n + 1));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "platform cfg outside src/sys:\n{}", offenders.join("\n"));
    }

    #[test]
    fn the_running_file_lives_under_remora() {
        let p = running_path(Os::Linux, Path::new("/home/u"));
        assert!(p.ends_with(Path::new("remora").join("running.json")), "{}", p.display());
    }

    #[test]
    fn the_running_file_round_trips_and_is_removed_only_by_its_owner() {
        let dir = std::env::temp_dir().join(format!("jp-running-{}", std::process::id()));
        let path = dir.join("remora").join("running.json");
        write_running(&path, &Running { pid: std::process::id(), port: 1, token: "t".into() }).unwrap();
        assert!(path.exists());
        // Nothing listens on port 1: no launcher answers.
        assert!(!signal_running(&path));
        std::fs::write(&path, r#"{"pid":1,"port":1,"token":"t"}"#).unwrap();
        remove_running(&path);
        assert!(path.exists(), "removed another launcher's file");
        write_running(&path, &Running { pid: std::process::id(), port: 1, token: "t".into() }).unwrap();
        remove_running(&path);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_running_launcher_is_signalled_with_its_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let got = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).unwrap();
            line
        });
        let dir = std::env::temp_dir().join(format!("jp-signal-{}", std::process::id()));
        let path = dir.join("running.json");
        write_running(&path, &Running { pid: 7, port, token: "s3cret".into() }).unwrap();
        assert!(signal_running(&path));
        assert_eq!(got.join().unwrap().trim(), "s3cret");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plain_paths_lose_the_verbatim_prefix() {
        assert_eq!(plain_path(Path::new(r"\\?\C:\Portable\remora.exe")), r"C:\Portable\remora.exe");
        assert_eq!(plain_path(Path::new("/home/deck/jf")), "/home/deck/jf");
    }

    #[test]
    fn tokens_are_random() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
