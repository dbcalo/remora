//! The one object a window talks to: it owns the app, the platform state and
//! the busy flag, turns them into a `View`, and carries out `Action`s on their
//! own threads so no window ever waits. Shared by every front-end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app::{App, Phase};
use crate::config::Config;
use crate::decide::{self, FwAction, Os};
use crate::platform;
use crate::view::{self, Action, Platform, View};

pub struct Controller {
    pub os: Os,
    app: Arc<App>,
    plat: Arc<Mutex<PlatState>>,
    busy: Arc<AtomicBool>,
    local_url: String,
    /// Set by a second launch; the window raises itself and clears it.
    raise: AtomicBool,
}

impl Controller {
    pub fn new(os: Os, app: Arc<App>, plat: Arc<Mutex<PlatState>>, local_url: String) -> Controller {
        Controller { os, app, plat, busy: Arc::default(), local_url, raise: AtomicBool::new(false) }
    }

    /// One frame. Also notices Jellyfin exiting, and records that the window is
    /// visible, which keeps the auto-stop from firing.
    pub fn view(&self, window_visible: bool) -> View {
        self.app.check_process();
        if window_visible {
            self.app.seen();
        }
        let s = self.app.state();
        let p = self.plat.lock().unwrap();
        view::view(&s, &p.view(), self.busy.load(Ordering::SeqCst))
    }

    pub fn quit_needs_confirm(&self) -> bool {
        view::quit_needs_confirm(&self.app.state())
    }

    /// The auto-stop settings the window's controls start from.
    pub fn idle_settings(&self) -> (bool, u64, u64) {
        let p = self.plat.lock().unwrap();
        let idle = p.cfg.idle();
        let minutes = |d: Duration| if d.is_zero() { 30 } else { d.as_secs() / 60 };
        (!p.cfg.idle_stop.off, minutes(idle.start_window), minutes(idle.margin))
    }

    /// Re-reads the Library page without the busy flag: polled while a scan
    /// runs, so the progress moves without "Working..." flashing.
    pub fn refresh_library(&self) {
        let app = self.app.clone();
        std::thread::spawn(move || {
            let _ = app.load_library();
        });
    }

    pub fn request_raise(&self) {
        self.raise.store(true, Ordering::SeqCst);
    }

    pub fn take_raise(&self) -> bool {
        self.raise.swap(false, Ordering::SeqCst)
    }

    /// Carries out one action. Slow ones run on their own thread with `busy`
    /// set; an error lands in the window's message line.
    pub fn act(&self, action: Action) {
        let (app, plat) = (self.app.clone(), self.plat.clone());
        let work: Box<dyn FnOnce() -> Result<(), String> + Send> = match action {
            Action::OpenJellyfin => {
                let _ = platform::open_url(self.os, &self.local_url);
                return;
            }
            Action::OpenLogs => {
                let dir = plat.lock().unwrap().cfg.jellyfin_dir().join("logs");
                if let Err(e) = platform::open_folder(self.os, &dir) {
                    app.set_error(Some(format!("Could not open {}: {e}", dir.display())));
                }
                return;
            }
            Action::OpenAddress => {
                if let Some(url) = app.state().tv.first() {
                    let _ = platform::open_url(self.os, url);
                }
                return;
            }
            Action::SaveIdle { on, start_minutes, margin_minutes } => {
                let saved = plat.lock().unwrap().cfg.set_idle(!on, start_minutes.max(1), margin_minutes.max(1));
                let idle = plat.lock().unwrap().cfg.idle();
                app.set_idle(idle);
                app.set_error(saved.err().map(|e| format!("Auto-stop applies now, but could not be saved: {e}")));
                return;
            }
            Action::StartStop => Box::new(move || {
                // A failed start explains itself in the state's "why".
                if app.state().phase == Phase::Stopped {
                    let _ = app.start();
                } else {
                    app.stop("Stopped from the window.");
                }
                Ok(())
            }),
            Action::Setup { user, password, media } => Box::new(move || {
                if app.state().needs_setup { app.setup(&user, &password, &media) } else { app.make_key(&user, &password) }
            }),
            Action::SignInAgain => Box::new(move || {
                app.forget_key();
                Ok(())
            }),
            Action::OpenZone => Box::new(move || PlatState::act(&plat, "open-zone")),
            Action::MarkPrivate => Box::new(move || PlatState::act(&plat, "mark-private")),
            Action::AddToSteam => Box::new(move || PlatState::act(&plat, "add-to-steam")),
            Action::LoadUsers => Box::new(move || app.load_users()),
            Action::AddUser { name, password, confirm } => Box::new(move || app.add_user(&name, &password, &confirm)),
            Action::Restart => Box::new(move || app.restart()),
            Action::Scan => Box::new(move || app.scan()),
            Action::LoadLibrary => Box::new(move || app.load_library()),
            Action::AddLibrary { name, kind, folder } => Box::new(move || app.add_library(&name, &kind, &folder)),
            Action::AddFolder { library, folder } => Box::new(move || app.add_folder(&library, &folder)),
            Action::RemoveFolder { library, folder } => Box::new(move || app.remove_folder(&library, &folder)),
            Action::RemoveLibrary { library } => Box::new(move || app.remove_library(&library)),
            Action::SetPassword { user_id, password, confirm } => Box::new(move || app.set_password(&user_id, &password, &confirm)),
        };
        let (app, busy) = (self.app.clone(), self.busy.clone());
        busy.store(true, Ordering::SeqCst);
        app.set_error(None);
        std::thread::spawn(move || {
            let r = work();
            app.set_error(r.err());
            busy.store(false, Ordering::SeqCst);
        });
    }
}

/// What the window shows about the firewall and Steam, refreshed after every
/// action that could change it.
pub struct PlatState {
    os: Os,
    game: bool,
    exe: String,
    pub cfg: Config,
    plan: decide::Fw,
    profiles: Vec<platform::Profile>,
    steam: Option<PathBuf>,
    added: bool,
}

impl PlatState {
    pub fn new(os: Os, game: bool, exe: String, cfg: Config) -> PlatState {
        PlatState {
            os,
            game,
            exe,
            cfg,
            plan: decide::Fw { action: FwAction::Nothing, message: String::new() },
            profiles: vec![],
            steam: None,
            added: false,
        }
    }

    fn view(&self) -> Platform {
        Platform { os: self.os, game: self.game, fw: self.plan.clone(), steam: self.steam.as_ref().map(|_| self.added) }
    }

    /// Probes without holding the lock: the probes run commands.
    pub fn refresh(me: &Mutex<PlatState>, home: &Path) {
        let (os, exe, cfg) = {
            let p = me.lock().unwrap();
            (p.os, p.exe.clone(), p.cfg.clone())
        };
        let (net, profiles) = platform::probe(os, |z| cfg.zone_approved(z));
        let steam = platform::steam_dir(os, home);
        let added = steam.as_deref().is_some_and(|s| decide::added_to_steam(os, &platform::read_shortcuts(s), &exe));
        let mut p = me.lock().unwrap();
        p.plan = decide::firewall_plan(&net);
        p.profiles = profiles;
        p.steam = steam;
        p.added = added;
    }

    /// What needs no question: the Windows rules on first run (one UAC prompt)
    /// and an already approved firewalld zone.
    pub fn apply_automatic(me: &Mutex<PlatState>, home: &Path) {
        let action = me.lock().unwrap().plan.action.clone();
        match action {
            FwAction::AddWindowsRules => drop(platform::elevate(&[])),
            FwAction::OpenZone(zone) => drop(platform::open_zone(&zone)),
            _ => return,
        }
        PlatState::refresh(me, home);
    }

    fn act(me: &Mutex<PlatState>, action: &str) -> Result<(), String> {
        let (plan, profiles, os, exe) = {
            let p = me.lock().unwrap();
            (p.plan.action.clone(), p.profiles.clone(), p.os, p.exe.clone())
        };
        let r = match (action, plan) {
            ("open-zone", FwAction::AskZone(zone)) => {
                me.lock().unwrap().cfg.approve_zone(&zone).and_then(|_| platform::open_zone(&zone))
            }
            ("mark-private", FwAction::OfferPrivate) => platform::elevate(&profiles),
            ("add-to-steam", _) => platform::add_to_steam(os, &exe),
            _ => Ok(()),
        };
        PlatState::refresh(me, &crate::home_dir(os));
        r.map_err(|e| e.to_string())
    }
}
