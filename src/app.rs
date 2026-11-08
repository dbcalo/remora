//! The launcher's state machine: start Jellyfin, watch playback, hold the
//! machine awake through Caffeine while something plays, stop the server when it
//! has gone unused, and answer the window. Every effect is a field of `Deps`, so
//! all of it runs in tests over fakes; `decide` makes the choices.

use std::io;
use std::sync::Mutex;
use std::time::Duration;

use crate::decide::{self, IdleConfig, Keep, Library, Recent, Scan, Session, User};
use crate::jellyfin;
use crate::keystore::Store;

/// The Jellyfin the launcher talks to.
pub trait Server: Send + Sync {
    fn healthy(&self) -> bool;
    fn sessions(&self, key: &str) -> Result<Vec<Session>, jellyfin::Error>;
    fn needs_setup(&self) -> Result<bool, jellyfin::Error>;
    fn setup(&self, user: &str, password: &str, media: &str) -> Result<(), jellyfin::Error>;
    fn make_key(&self, user: &str, password: &str) -> Result<String, jellyfin::Error>;
    fn users(&self, key: &str) -> Result<Vec<User>, jellyfin::Error>;
    fn set_password(&self, key: &str, user_id: &str, password: &str) -> Result<(), jellyfin::Error>;
    fn add_user(&self, key: &str, name: &str, password: &str) -> Result<User, jellyfin::Error>;
    fn recently_added(&self, key: &str, user_id: &str) -> Result<Vec<Recent>, jellyfin::Error>;
    fn libraries(&self, key: &str) -> Result<Vec<Library>, jellyfin::Error>;
    fn scan(&self, key: &str) -> Result<(), jellyfin::Error>;
    fn scan_state(&self, key: &str) -> Result<Scan, jellyfin::Error>;
    fn add_library(&self, key: &str, name: &str, kind: &str, folder: &str) -> Result<(), jellyfin::Error>;
    fn add_folder(&self, key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error>;
    fn remove_folder(&self, key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error>;
    fn remove_library(&self, key: &str, library: &str) -> Result<(), jellyfin::Error>;
}

impl Server for jellyfin::Client {
    fn healthy(&self) -> bool {
        jellyfin::Client::healthy(self)
    }
    fn sessions(&self, key: &str) -> Result<Vec<Session>, jellyfin::Error> {
        jellyfin::Client::sessions(self, key)
    }
    fn needs_setup(&self) -> Result<bool, jellyfin::Error> {
        jellyfin::Client::needs_setup(self)
    }
    fn setup(&self, user: &str, password: &str, media: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::setup(self, user, password, media)
    }
    fn make_key(&self, user: &str, password: &str) -> Result<String, jellyfin::Error> {
        jellyfin::Client::make_key(self, user, password)
    }
    fn users(&self, key: &str) -> Result<Vec<User>, jellyfin::Error> {
        jellyfin::Client::users(self, key)
    }
    fn set_password(&self, key: &str, user_id: &str, password: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::set_password(self, key, user_id, password)
    }
    fn add_user(&self, key: &str, name: &str, password: &str) -> Result<User, jellyfin::Error> {
        jellyfin::Client::add_user(self, key, name, password)
    }
    fn recently_added(&self, key: &str, user_id: &str) -> Result<Vec<Recent>, jellyfin::Error> {
        jellyfin::Client::recently_added(self, key, user_id)
    }
    fn libraries(&self, key: &str) -> Result<Vec<Library>, jellyfin::Error> {
        jellyfin::Client::libraries(self, key)
    }
    fn scan(&self, key: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::scan(self, key)
    }
    fn scan_state(&self, key: &str) -> Result<Scan, jellyfin::Error> {
        jellyfin::Client::scan_state(self, key)
    }
    fn add_library(&self, key: &str, name: &str, kind: &str, folder: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::add_library(self, key, name, kind, folder)
    }
    fn add_folder(&self, key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::add_folder(self, key, library, folder)
    }
    fn remove_folder(&self, key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::remove_folder(self, key, library, folder)
    }
    fn remove_library(&self, key: &str, library: &str) -> Result<(), jellyfin::Error> {
        jellyfin::Client::remove_library(self, key, library)
    }
}

/// The running Jellyfin.
pub trait Proc: Send {
    fn exited(&mut self) -> bool;
    fn stop(&mut self);
}

impl Proc for crate::platform::Jellyfin {
    fn exited(&mut self) -> bool {
        crate::platform::Jellyfin::exited(self)
    }
    fn stop(&mut self) {
        crate::platform::Jellyfin::stop(self)
    }
}

/// Every effect the app has.
pub struct Deps {
    /// A monotonic clock, as time since some fixed start.
    pub now: Box<dyn Fn() -> Duration + Send + Sync>,
    pub start: Box<dyn Fn() -> io::Result<Box<dyn Proc>> + Send + Sync>,
    pub server: Box<dyn Server>,
    pub keys: Mutex<Box<dyn Store>>,
    pub caffeine: Mutex<Box<dyn FnMut() -> Keep + Send>>,
    /// The starting auto-stop settings; `App::set_idle` changes them later.
    pub idle: IdleConfig,
    pub tv: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    /// How long to wait for /health after starting: 90 s, as the PowerShell
    /// launcher allowed a first start migrating its database.
    pub ready_timeout: Duration,
    pub ready_poll: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Stopped => "Stopped",
            Phase::Starting => "Starting\u{2026}",
            Phase::Running => "Running",
        }
    }
}

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub phase: Phase,
    pub why: String,
    pub error: String,
    pub tv: Vec<String>,
    pub playing: Vec<String>,
    pub keep: String,
    pub idle_stop: String,
    pub needs_setup: bool,
    pub needs_password: bool,
    /// Whether this install has a working API key stored.
    pub has_key: bool,
    /// Jellyfin's users, as last loaded for the Users page.
    pub users: Vec<User>,
    /// The libraries and the scan, as last loaded for the Library page.
    pub libraries: Vec<Library>,
    pub scan: Scan,
    /// The newest items, for the Library page.
    pub recent: Vec<Recent>,
}

/// A visible window keeps the server for this long after it was last seen.
const VISIBLE_WINDOW: Duration = Duration::from_secs(10);

struct Inner {
    phase: Phase,
    why: String,
    error: String,
    key: Option<String>,
    needs_setup: bool,
    needs_password: bool,
    sessions: Vec<Session>,
    users: Vec<User>,
    libraries: Vec<Library>,
    scan: Scan,
    recent: Vec<Recent>,
    keep: Keep,
    last_hold: Option<Duration>,
    idle_deadline: Option<Duration>,
    last_seen: Option<Duration>,
    /// Bumped on every start, so a slow start never overwrites a later one.
    run: u64,
    idle: IdleConfig,
}

pub struct App {
    d: Deps,
    inner: Mutex<Inner>,
    proc: Mutex<Option<Box<dyn Proc>>>,
}

impl App {
    pub fn new(d: Deps) -> App {
        let idle = d.idle;
        App {
            d,
            inner: Mutex::new(Inner {
                phase: Phase::Stopped,
                why: String::new(),
                error: String::new(),
                key: None,
                needs_setup: false,
                needs_password: false,
                sessions: vec![],
                users: vec![],
                libraries: vec![],
                scan: Scan::default(),
                recent: vec![],
                keep: Keep::Unknown,
                last_hold: None,
                idle_deadline: None,
                last_seen: None,
                run: 0,
                idle,
            }),
            proc: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Launches Jellyfin and waits until it answers, then polls once so the
    /// window knows at once whether setup or the sign-in is needed. A no-op while
    /// one is starting or running.
    pub fn start(&self) -> Result<(), String> {
        let run = {
            let mut i = self.lock();
            if i.phase != Phase::Stopped {
                return Ok(());
            }
            i.phase = Phase::Starting;
            i.why.clear();
            i.error.clear();
            i.run += 1;
            i.run
        };
        match (self.d.start)() {
            Ok(p) => *self.proc.lock().unwrap() = Some(p),
            Err(e) => {
                let why = format!("Jellyfin could not start: {e}");
                let mut i = self.lock();
                i.phase = Phase::Stopped;
                i.why = why.clone();
                return Err(why);
            }
        }

        let waited = std::time::Instant::now();
        while !self.d.server.healthy() {
            if self.proc_exited() {
                self.stop("Jellyfin exited while starting; its logs folder says why."); // drops it, and what it holds
                return Err("jellyfin exited".into());
            }
            if waited.elapsed() >= self.d.ready_timeout {
                self.stop("Jellyfin did not answer within 90 seconds; see its logs folder.");
                return Err("jellyfin did not become ready".into());
            }
            std::thread::sleep(self.d.ready_poll);
        }

        let key = self.d.keys.lock().unwrap().get().ok().flatten();
        {
            let mut i = self.lock();
            if i.run != run || i.phase != Phase::Starting {
                return Ok(()); // stopped while starting
            }
            i.phase = Phase::Running;
            i.idle_deadline = i.idle.initial((self.d.now)());
            i.key = key;
        }
        self.tick();
        Ok(())
    }

    fn proc_exited(&self) -> bool {
        self.proc.lock().unwrap().as_mut().is_none_or(|p| p.exited())
    }

    fn finish_stop(&self, why: &str) {
        let mut i = self.lock();
        i.phase = Phase::Stopped;
        i.why = why.into();
        i.sessions.clear();
    }

    /// Ends the server, if one runs.
    pub fn stop(&self, why: &str) {
        let p = self.proc.lock().unwrap().take();
        self.finish_stop(why);
        if let Some(mut p) = p {
            p.stop();
        }
    }

    /// Records that the window is on screen.
    pub fn seen(&self) {
        let now = (self.d.now)();
        self.lock().last_seen = Some(now);
    }

    /// Notices Jellyfin exiting on its own. Cheap: called every second.
    pub fn check_process(&self) {
        let running = matches!(self.lock().phase, Phase::Running);
        if running && self.proc_exited() {
            self.proc.lock().unwrap().take();
            self.finish_stop("Jellyfin exited; its logs folder says why.");
        }
    }

    /// One poll: read playback, renew the hold, roll and check the idle clock.
    /// Network calls happen outside the lock, so the window never waits on them.
    pub fn tick(&self) {
        let (running, key) = {
            let i = self.lock();
            (i.phase == Phase::Running, i.key.clone())
        };
        if !running {
            return;
        }
        let mut sessions = None;
        let mut fresh = false;
        let mut key = key;
        match &key {
            Some(k) => match self.d.server.sessions(k) {
                Ok(s) => sessions = Some(s),
                Err(jellyfin::Error::Unauthorized) => {
                    // Revoked in the Dashboard, or the data folder was replaced;
                    // in the second case Jellyfin is fresh and needs setup.
                    key = None;
                    let _ = self.d.keys.lock().unwrap().delete();
                    fresh = self.d.server.needs_setup().unwrap_or(false);
                }
                Err(_) => {}
            },
            None => fresh = self.d.server.needs_setup().unwrap_or(false),
        }

        let now = (self.d.now)();
        let playing = sessions.as_deref().is_some_and(|s| !decide::playing(s).is_empty());
        let due = decide::hold_due(playing, self.lock().last_hold.map(|t| now.saturating_sub(t)));
        let keep = if due { Some((self.d.caffeine.lock().unwrap())()) } else { None };

        let stop_now = {
            let mut i = self.lock();
            if i.phase != Phase::Running {
                return;
            }
            if let Some(k) = keep {
                i.keep = k;
                i.last_hold = Some(now);
            }
            i.needs_setup = key.is_none() && fresh;
            i.needs_password = key.is_none() && !fresh;
            i.key = key;
            if let Some(s) = sessions {
                i.sessions = s;
            }
            let visible = i.last_seen.is_some_and(|t| now.saturating_sub(t) < VISIBLE_WINDOW);
            i.idle_deadline = i.idle.next(i.idle_deadline, now, &i.sessions, visible);
            i.idle.due(i.idle_deadline, now)
        };
        if stop_now {
            self.stop("Stopped itself after going unused. Press Start to run it again.");
        }
    }

    /// First-time setup, then this install's key.
    pub fn setup(&self, user: &str, password: &str, media: &str) -> Result<(), String> {
        self.d.server.setup(user, password, media).map_err(|e| format!("Setup failed: {e}"))?;
        self.make_key(user, password)
    }

    /// The one-time sign-in that makes this install's key. The password goes no
    /// further than the call.
    pub fn make_key(&self, user: &str, password: &str) -> Result<(), String> {
        let key = self.d.server.make_key(user, password).map_err(|e| e.to_string())?;
        self.d
            .keys
            .lock()
            .unwrap()
            .set(&key)
            .map_err(|e| format!("The key was made but could not be stored: {e}"))?;
        let mut i = self.lock();
        i.key = Some(key);
        i.needs_setup = false;
        i.needs_password = false;
        Ok(())
    }

    /// New auto-stop settings, applied at once: a running server's clock starts
    /// again from now under them, and off clears it.
    pub fn set_idle(&self, idle: IdleConfig) {
        let now = (self.d.now)();
        let mut i = self.lock();
        i.idle = idle;
        i.idle_deadline = if i.phase == Phase::Running { idle.initial(now) } else { None };
    }

    /// "Sign in again": drops this install's key, so the window asks for the
    /// password (or setup, on a fresh Jellyfin) and makes a new one.
    pub fn forget_key(&self) {
        let _ = self.d.keys.lock().unwrap().delete();
        {
            let mut i = self.lock();
            i.key = None;
            i.needs_password = true;
        }
        self.tick(); // settles password versus setup
    }

    /// Says something in the window's message line, unless an error is showing.
    pub fn note(&self, why: &str) {
        let mut i = self.lock();
        if i.why.is_empty() {
            i.why = why.into();
        }
    }

    /// Records an action's error for the window, or clears it.
    pub fn set_error(&self, e: Option<String>) {
        self.lock().error = e.unwrap_or_default();
    }

    /// What the window shows now.
    pub fn state(&self) -> State {
        let now = (self.d.now)();
        let i = self.lock();
        let running = i.phase == Phase::Running;
        State {
            phase: i.phase,
            why: i.why.clone(),
            error: i.error.clone(),
            tv: if running { (self.d.tv)() } else { vec![] },
            playing: i
                .sessions
                .iter()
                .filter_map(|s| {
                    let item = s.item.as_ref()?;
                    Some(format!("{} on {}{}", item.name, s.device, if s.paused { " (paused)" } else { "" }))
                })
                .collect(),
            keep: keep_sentence(&i),
            idle_stop: match (running, i.idle_deadline) {
                (true, Some(d)) => format!(
                    "Stops itself in {} unless something is loaded or this window is open.",
                    human(d.saturating_sub(now))
                ),
                _ => String::new(),
            },
            needs_setup: i.needs_setup,
            needs_password: i.needs_password,
            has_key: i.key.is_some(),
            users: if running { i.users.clone() } else { vec![] },
            libraries: if running { i.libraries.clone() } else { vec![] },
            scan: if running { i.scan.clone() } else { Scan::default() },
            recent: if running { i.recent.clone() } else { vec![] },
        }
    }

    /// Reads the libraries and the scan's state for the Library page.
    pub fn load_library(&self) -> Result<(), String> {
        let key = self.running_key()?;
        let libraries = self.d.server.libraries(&key).map_err(|e| format!("Could not read the libraries: {e}"))?;
        let scan = self.d.server.scan_state(&key).map_err(|e| format!("Could not read the scan: {e}"))?;
        // Recently added needs a user id (12.4: 400 without); the administrator's.
        let mut admin = self.lock().users.iter().find(|u| u.admin).map(|u| u.id.clone());
        if admin.is_none() {
            admin = self.d.server.users(&key).ok().and_then(|us| us.into_iter().find(|u| u.admin)).map(|u| u.id);
        }
        let recent = admin.and_then(|id| self.d.server.recently_added(&key, &id).ok()).unwrap_or_default();
        let mut i = self.lock();
        i.libraries = libraries;
        i.scan = scan;
        i.recent = recent;
        Ok(())
    }

    /// Adds a library of `kind` (a Jellyfin collectionType) with one folder.
    pub fn add_library(&self, name: &str, kind: &str, folder: &str) -> Result<(), String> {
        let key = self.running_key()?;
        let (name, folder) = (name.trim(), folder.trim());
        if let Some(problem) = decide::library_problem(name, folder, &self.lock().libraries) {
            return Err(problem.into());
        }
        self.d.server.add_library(&key, name, kind, folder).map_err(|e| folder_error(e, "The library was not added"))?;
        self.changed(format!("Added the library {name}; Jellyfin is scanning it."))
    }

    /// Adds a folder to a library.
    pub fn add_folder(&self, library: &str, folder: &str) -> Result<(), String> {
        let key = self.running_key()?;
        if library.is_empty() {
            return Err("Choose a library first.".into());
        }
        self.d.server.add_folder(&key, library, folder.trim()).map_err(|e| folder_error(e, "The folder was not added"))?;
        self.changed(format!("Added the folder to {library}; Jellyfin is scanning it."))
    }

    /// Removes a folder from a library; its last folder is refused, since a
    /// library without one only confuses (remove the library instead).
    pub fn remove_folder(&self, library: &str, folder: &str) -> Result<(), String> {
        let key = self.running_key()?;
        let folders = self.lock().libraries.iter().find(|l| l.name == library).map(|l| l.paths.len());
        match folders {
            None => return Err("Choose a library first.".into()),
            Some(n) if n <= 1 => return Err(format!("That is the only folder of {library}: remove the library instead.")),
            _ => {}
        }
        self.d.server.remove_folder(&key, library, folder).map_err(|e| format!("The folder was not removed: {e}"))?;
        self.changed(format!("Removed the folder from {library}. Its files stay where they are."))
    }

    /// Removes a library. The files stay where they are.
    pub fn remove_library(&self, library: &str) -> Result<(), String> {
        let key = self.running_key()?;
        if library.is_empty() {
            return Err("Choose a library first.".into());
        }
        self.d.server.remove_library(&key, library).map_err(|e| format!("The library was not removed: {e}"))?;
        self.changed(format!("Removed the library {library}. Its files stay where they are."))
    }

    /// Says what changed and reloads the Library page.
    fn changed(&self, why: String) -> Result<(), String> {
        self.lock().why = why;
        self.load_library()
    }

    /// Starts a scan of every library.
    pub fn scan(&self) -> Result<(), String> {
        let key = self.running_key()?;
        self.d.server.scan(&key).map_err(|e| format!("The scan did not start: {e}"))?;
        // Shown at once; the next read brings the progress.
        self.lock().scan.running = true;
        let _ = self.load_library();
        Ok(())
    }

    /// Stops the server and starts it again, through the same paths as Stop and
    /// Start, so keep-awake, the auto-stop clock and the standby switch follow.
    pub fn restart(&self) -> Result<(), String> {
        self.stop("Restarting\u{2026}");
        self.start()?;
        self.lock().why = "Restarted.".into();
        Ok(())
    }

    /// Reads Jellyfin's users for the Users page. Needs the server and the key.
    pub fn load_users(&self) -> Result<(), String> {
        let key = self.running_key()?;
        let users = self.d.server.users(&key).map_err(|e| format!("Could not read the users: {e}"))?;
        self.lock().users = users;
        Ok(())
    }

    /// The key, while the server runs; otherwise what the window should say.
    fn running_key(&self) -> Result<String, String> {
        let i = self.lock();
        if i.phase != Phase::Running {
            return Err("Start the server first.".into());
        }
        i.key.clone().ok_or_else(|| "Sign in first: managing users needs this machine's key.".into())
    }

    /// Adds a user (never an administrator), then reloads the list.
    pub fn add_user(&self, name: &str, password: &str, confirm: &str) -> Result<(), String> {
        let key = self.running_key()?;
        let name = name.trim();
        if let Some(problem) = decide::new_user_problem(name, password, confirm, &self.lock().users) {
            return Err(problem.into());
        }
        self.d.server.add_user(&key, name, password).map_err(|e| format!("The user was not added: {e}"))?;
        self.lock().why = format!("Added the user {name}.");
        self.load_users()
    }

    /// Sets (or, blank, removes) a user's password with the key, then reloads
    /// the list. The password goes no further than the call.
    pub fn set_password(&self, user_id: &str, password: &str, confirm: &str) -> Result<(), String> {
        let key = self.running_key()?;
        let user = self.lock().users.iter().find(|u| u.id == user_id).cloned().ok_or("Choose a user first.")?;
        if let Some(problem) = decide::password_problem(user.admin, password, confirm) {
            return Err(problem.into());
        }
        self.d.server.set_password(&key, &user.id, password).map_err(|e| format!("The password was not changed: {e}"))?;
        let done = if password.is_empty() { "removed" } else { "set" };
        self.lock().why = format!(
            "Password {done} for {}. Jellyfin signed them out everywhere: sign in again on the TV.",
            user.name
        );
        self.load_users()
    }
}

/// A failed library change, worded for the window: Jellyfin answers 404 for a
/// folder it cannot see.
fn folder_error(e: jellyfin::Error, what: &str) -> String {
    match e {
        jellyfin::Error::Status { status: 404, .. } => format!("{what}: that folder does not exist, or Jellyfin cannot see it."),
        e => format!("{what}: {e}"),
    }
}

fn human(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

fn keep_sentence(i: &Inner) -> String {
    if i.phase != Phase::Running {
        return String::new();
    }
    if i.key.is_none() {
        return "Keep-awake needs the one-time sign-in below.".into();
    }
    match i.keep {
        Keep::Paused => "Caffeine's Jellyfin row is stopped, so this machine is not held awake until it resumes.".into(),
        Keep::TooOld => "Held awake through an older Caffeine, whose row says an agent is holding.".into(),
        Keep::Failed => "Caffeine could not be run, so this machine may sleep during playback.".into(),
        _ if !decide::playing(&i.sessions).is_empty() => "Holding this machine awake while it plays.".into(),
        _ => format!(
            "Nothing is playing: this machine may sleep {} after playback stops or pauses.",
            human(decide::CAFFEINE_GRACE)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::Item;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeServer {
        sessions: Mutex<Vec<Session>>,
        unauthorized: AtomicBool,
        fresh: AtomicBool,
        auth_fails: AtomicBool,
        /// (user, password)
        users: Mutex<Vec<(User, String)>>,
        pw_calls: AtomicUsize,
        libraries: Mutex<Vec<Library>>,
        scanning: AtomicBool,
        lib_calls: AtomicUsize,
    }

    impl Server for Arc<FakeServer> {
        fn healthy(&self) -> bool {
            true
        }
        fn sessions(&self, _key: &str) -> Result<Vec<Session>, jellyfin::Error> {
            if self.unauthorized.load(Ordering::SeqCst) {
                return Err(jellyfin::Error::Unauthorized);
            }
            Ok(self.sessions.lock().unwrap().clone())
        }
        fn needs_setup(&self) -> Result<bool, jellyfin::Error> {
            Ok(self.fresh.load(Ordering::SeqCst))
        }
        fn setup(&self, _u: &str, _p: &str, _m: &str) -> Result<(), jellyfin::Error> {
            self.fresh.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn make_key(&self, _u: &str, _p: &str) -> Result<String, jellyfin::Error> {
            if self.auth_fails.load(Ordering::SeqCst) {
                return Err(jellyfin::Error::BadPassword);
            }
            Ok("new-key".into())
        }
        fn users(&self, _key: &str) -> Result<Vec<User>, jellyfin::Error> {
            Ok(self.users.lock().unwrap().iter().map(|(u, pw)| User { has_password: !pw.is_empty(), ..u.clone() }).collect())
        }
        fn set_password(&self, _key: &str, user_id: &str, password: &str) -> Result<(), jellyfin::Error> {
            self.pw_calls.fetch_add(1, Ordering::SeqCst);
            let mut users = self.users.lock().unwrap();
            let u = users.iter_mut().find(|(u, _)| u.id == user_id).ok_or(jellyfin::Error::Status { call: "pw".into(), status: 404 })?;
            u.1 = password.into();
            Ok(())
        }
        fn recently_added(&self, _key: &str, user_id: &str) -> Result<Vec<Recent>, jellyfin::Error> {
            assert_eq!(user_id, "a1", "the administrator's id");
            Ok(vec![Recent { name: "Moana".into(), year: Some(2016) }])
        }
        fn add_user(&self, _key: &str, name: &str, password: &str) -> Result<User, jellyfin::Error> {
            self.pw_calls.fetch_add(1, Ordering::SeqCst);
            let u = User { id: format!("n-{name}"), name: name.into(), admin: false, has_password: true };
            self.users.lock().unwrap().push((u.clone(), password.into()));
            Ok(u)
        }
        fn libraries(&self, _key: &str) -> Result<Vec<Library>, jellyfin::Error> {
            Ok(self.libraries.lock().unwrap().clone())
        }
        fn scan(&self, _key: &str) -> Result<(), jellyfin::Error> {
            self.scanning.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn scan_state(&self, _key: &str) -> Result<Scan, jellyfin::Error> {
            Ok(Scan { running: self.scanning.load(Ordering::SeqCst), percent: None, last: None })
        }
        fn add_library(&self, _key: &str, name: &str, _kind: &str, folder: &str) -> Result<(), jellyfin::Error> {
            self.lib_calls.fetch_add(1, Ordering::SeqCst);
            self.libraries.lock().unwrap().push(Library { name: name.into(), paths: vec![folder.into()] });
            Ok(())
        }
        fn add_folder(&self, _key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error> {
            self.lib_calls.fetch_add(1, Ordering::SeqCst);
            if folder.contains("missing") {
                return Err(jellyfin::Error::Status { call: "POST /Library/VirtualFolders/Paths".into(), status: 404 });
            }
            let mut libs = self.libraries.lock().unwrap();
            libs.iter_mut().find(|l| l.name == library).unwrap().paths.push(folder.into());
            Ok(())
        }
        fn remove_folder(&self, _key: &str, library: &str, folder: &str) -> Result<(), jellyfin::Error> {
            self.lib_calls.fetch_add(1, Ordering::SeqCst);
            let mut libs = self.libraries.lock().unwrap();
            libs.iter_mut().find(|l| l.name == library).unwrap().paths.retain(|p| p != folder);
            Ok(())
        }
        fn remove_library(&self, _key: &str, library: &str) -> Result<(), jellyfin::Error> {
            self.lib_calls.fetch_add(1, Ordering::SeqCst);
            self.libraries.lock().unwrap().retain(|l| l.name != library);
            Ok(())
        }
    }

    struct FakeProc {
        dead: Arc<AtomicBool>,
    }

    impl Proc for FakeProc {
        fn exited(&mut self) -> bool {
            self.dead.load(Ordering::SeqCst)
        }
        fn stop(&mut self) {
            self.dead.store(true, Ordering::SeqCst);
        }
    }

    struct MemStore(Arc<Mutex<Option<String>>>);

    impl Store for MemStore {
        fn get(&self) -> io::Result<Option<String>> {
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

    struct H {
        app: App,
        srv: Arc<FakeServer>,
        dead: Arc<AtomicBool>,
        holds: Arc<AtomicUsize>,
        keep: Arc<Mutex<Keep>>,
        key: Arc<Mutex<Option<String>>>,
        now: Arc<AtomicU64>,
    }

    impl H {
        fn advance(&self, d: Duration) {
            self.now.fetch_add(d.as_secs(), Ordering::SeqCst);
        }
    }

    fn harness(key: Option<&str>, fresh: bool) -> H {
        let srv = Arc::new(FakeServer::default());
        srv.fresh.store(fresh, Ordering::SeqCst);
        let dead = Arc::new(AtomicBool::new(false));
        let holds = Arc::new(AtomicUsize::new(0));
        let keep = Arc::new(Mutex::new(Keep::Held));
        let key = Arc::new(Mutex::new(key.map(String::from)));
        let now = Arc::new(AtomicU64::new(0));
        let (d2, h2, k2, n2) = (dead.clone(), holds.clone(), keep.clone(), now.clone());
        let app = App::new(Deps {
            now: Box::new(move || Duration::from_secs(n2.load(Ordering::SeqCst))),
            start: Box::new(move || {
                d2.store(false, Ordering::SeqCst);
                Ok(Box::new(FakeProc { dead: d2.clone() }) as Box<dyn Proc>)
            }),
            server: Box::new(srv.clone()),
            keys: Mutex::new(Box::new(MemStore(key.clone()))),
            caffeine: Mutex::new(Box::new(move || {
                h2.fetch_add(1, Ordering::SeqCst);
                *k2.lock().unwrap()
            })),
            idle: IdleConfig { start_window: Duration::from_secs(7200), margin: Duration::from_secs(1800) },
            tv: Box::new(|| vec!["http://192.168.1.5:8096".into()]),
            ready_timeout: Duration::from_secs(1),
            ready_poll: Duration::from_millis(10),
        });
        app.start().unwrap();
        H { app, srv, dead, holds, keep, key, now }
    }

    fn film(paused: bool) -> Vec<Session> {
        vec![Session {
            device: "TV".into(),
            paused,
            item: Some(Item { name: "Film".into(), runtime_ticks: 2 * 3600 * 10_000_000 }),
            ..Default::default()
        }]
    }

    fn with_users(h: &H) {
        let u = |id: &str, name: &str, admin| User { id: id.into(), name: name.into(), admin, has_password: true };
        *h.srv.users.lock().unwrap() = vec![(u("a1", "alex", true), "pw".into()), (u("f1", "kids", false), "k".into())];
        h.app.load_users().unwrap();
    }

    #[test]
    fn a_note_never_hides_why_jellyfin_did_not_start() {
        let h = harness(Some("k"), false);
        h.app.stop("Jellyfin exited while starting; its logs folder says why.");
        h.app.note("Moved the settings to Remora's name.");
        let why = h.app.state().why;
        assert!(why.contains("exited"), "{why}");
    }

    #[test]
    fn users_load_and_a_password_is_set_with_a_note_about_signing_in_again() {
        let h = harness(Some("k"), false);
        with_users(&h);
        assert_eq!(h.app.state().users.len(), 2);
        h.app.set_password("f1", "new", "new").unwrap();
        assert_eq!(h.srv.users.lock().unwrap()[1].1, "new");
        let why = h.app.state().why;
        assert!(why.contains("kids") && why.contains("sign in again"), "{why}");
    }

    #[test]
    fn a_blank_password_removes_it_and_the_list_shows_that() {
        let h = harness(Some("k"), false);
        with_users(&h);
        h.app.set_password("f1", "", "").unwrap();
        let s = h.app.state();
        assert!(!s.users.iter().find(|u| u.id == "f1").unwrap().has_password, "the list was not reloaded");
        assert!(s.why.contains("removed"), "{}", s.why);
    }

    #[test]
    fn a_bad_new_password_never_reaches_jellyfin() {
        let h = harness(Some("k"), false);
        with_users(&h);
        assert_eq!(h.app.set_password("f1", "a", "b"), Err("The two passwords differ.".into()));
        assert_eq!(h.app.set_password("a1", "", ""), Err("An administrator needs a password.".into()));
        assert!(h.app.set_password("nobody", "a", "a").is_err());
        assert_eq!(h.srv.pw_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_library_page_loads_and_a_scan_shows_as_running() {
        let h = harness(Some("k"), false);
        *h.srv.libraries.lock().unwrap() = vec![Library { name: "Films".into(), paths: vec!["D:\\Films".into()] }];
        h.app.load_library().unwrap();
        let s = h.app.state();
        assert_eq!(s.libraries.len(), 1);
        assert!(!s.scan.running);
        h.app.scan().unwrap();
        assert!(h.app.state().scan.running, "the page must show the scan at once");
    }

    fn with_films(h: &H) {
        *h.srv.libraries.lock().unwrap() = vec![Library { name: "Films".into(), paths: vec!["D:\\Films".into()] }];
        h.app.load_library().unwrap();
    }

    #[test]
    fn libraries_and_folders_are_added_and_removed_and_the_list_follows() {
        let h = harness(Some("k"), false);
        with_films(&h);
        h.app.add_library("Kids", "movies", "D:\\Kids").unwrap();
        assert_eq!(h.app.state().libraries.len(), 2, "the list was not reloaded");
        assert!(h.app.state().why.contains("Kids"), "{}", h.app.state().why);
        h.app.add_folder("Films", "E:\\More").unwrap();
        h.app.remove_folder("Films", "E:\\More").unwrap();
        h.app.remove_library("Kids").unwrap();
        let s = h.app.state();
        assert_eq!(s.libraries, vec![Library { name: "Films".into(), paths: vec!["D:\\Films".into()] }]);
        assert!(s.why.contains("files stay"), "{}", s.why);
    }

    #[test]
    fn bad_library_changes_never_reach_jellyfin() {
        let h = harness(Some("k"), false);
        with_films(&h);
        assert_eq!(h.app.add_library("films", "movies", "D:\\X"), Err("A library already has that name.".into()));
        assert_eq!(h.app.add_library("Kids", "movies", ""), Err("Choose the library's folder.".into()));
        assert!(h.app.remove_folder("Films", "D:\\Films").unwrap_err().contains("only folder"));
        assert!(h.app.add_folder("", "D:\\X").is_err(), "no library chosen");
        assert_eq!(h.srv.lib_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_missing_folder_says_so() {
        let h = harness(Some("k"), false);
        with_films(&h);
        let e = h.app.add_folder("Films", "D:\\missing").unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
    }

    #[test]
    fn the_library_page_shows_what_was_added_lately() {
        let h = harness(Some("k"), false);
        with_users(&h);
        with_films(&h);
        assert_eq!(h.app.state().recent, vec![Recent { name: "Moana".into(), year: Some(2016) }]);
    }

    #[test]
    fn library_calls_need_the_running_server_and_key() {
        let h = harness(None, false);
        assert!(h.app.load_library().is_err());
        assert!(h.app.scan().is_err());
        assert!(!h.srv.scanning.load(Ordering::SeqCst));
    }

    #[test]
    fn restart_stops_and_starts_again() {
        let h = harness(Some("k"), false);
        let first = h.dead.clone();
        h.app.restart().unwrap();
        assert_eq!(h.app.state().phase, Phase::Running);
        assert!(!first.load(Ordering::SeqCst), "a new process runs");
        assert!(h.app.state().why.contains("Restarted"), "{}", h.app.state().why);
    }

    #[test]
    fn adding_a_user_reloads_the_list_and_says_so() {
        let h = harness(Some("k"), false);
        with_users(&h);
        h.app.add_user("Grandma", "pw", "pw").unwrap();
        let s = h.app.state();
        assert!(s.users.iter().any(|u| u.name == "Grandma"), "the list was not reloaded");
        assert!(s.why.contains("Grandma"), "{}", s.why);
    }

    #[test]
    fn a_bad_new_user_never_reaches_jellyfin() {
        let h = harness(Some("k"), false);
        with_users(&h);
        assert_eq!(h.app.add_user("KIDS", "", ""), Err("A user already has that name.".into()));
        assert_eq!(h.app.add_user("Nan", "a", "b"), Err("The two passwords differ.".into()));
        assert_eq!(h.srv.pw_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn users_need_the_key() {
        let h = harness(None, false);
        assert!(h.app.load_users().is_err());
        assert!(h.app.state().users.is_empty());
    }

    #[test]
    fn start_runs_jellyfin_and_shows_the_tv_address() {
        let h = harness(Some("k"), false);
        let s = h.app.state();
        assert_eq!(s.phase, Phase::Running);
        assert_eq!(s.tv.len(), 1);
    }

    #[test]
    fn playing_holds_the_machine_and_renews_every_five_minutes() {
        let h = harness(Some("k"), false);
        *h.srv.sessions.lock().unwrap() = film(false);
        h.app.tick();
        h.advance(Duration::from_secs(60));
        h.app.tick();
        h.advance(Duration::from_secs(300));
        h.app.tick();
        assert_eq!(h.holds.load(Ordering::SeqCst), 2);
        let s = h.app.state();
        assert_eq!(s.playing.len(), 1);
        assert!(s.keep.contains("awake"), "{}", s.keep);
    }

    #[test]
    fn paused_does_not_hold() {
        let h = harness(Some("k"), false);
        *h.srv.sessions.lock().unwrap() = film(true);
        h.app.tick();
        assert_eq!(h.holds.load(Ordering::SeqCst), 0);
        assert!(h.app.state().playing[0].contains("paused"));
    }

    #[test]
    fn a_paused_caffeine_is_shown_and_not_fought() {
        let h = harness(Some("k"), false);
        *h.keep.lock().unwrap() = Keep::Paused;
        *h.srv.sessions.lock().unwrap() = film(false);
        h.app.tick();
        assert!(h.app.state().keep.contains("stopped"));
    }

    #[test]
    fn without_a_key_the_sign_in_is_offered_at_once() {
        let h = harness(None, false);
        let s = h.app.state();
        assert!(s.needs_password && !s.needs_setup, "{s:?}");
    }

    #[test]
    fn a_revoked_key_asks_for_the_password_once() {
        let h = harness(Some("k"), false);
        h.srv.unauthorized.store(true, Ordering::SeqCst);
        h.app.tick();
        assert!(h.app.state().needs_password);
        assert_eq!(*h.key.lock().unwrap(), None, "the rejected key must not be retried every launch");
        h.srv.unauthorized.store(false, Ordering::SeqCst);
        h.app.make_key("alex", "pw").unwrap();
        assert_eq!(h.key.lock().unwrap().as_deref(), Some("new-key"));
        assert!(!h.app.state().needs_password);
    }

    #[test]
    fn a_rejected_key_on_a_fresh_jellyfin_offers_setup() {
        let h = harness(Some("k"), false);
        h.srv.unauthorized.store(true, Ordering::SeqCst);
        h.srv.fresh.store(true, Ordering::SeqCst);
        h.app.tick();
        let s = h.app.state();
        assert!(s.needs_setup && !s.needs_password, "{s:?}");
    }

    #[test]
    fn a_fresh_jellyfin_runs_setup_from_the_window() {
        let h = harness(None, true);
        assert!(h.app.state().needs_setup);
        h.app.setup("alex", "pw", "D:/Media").unwrap();
        assert_eq!(h.key.lock().unwrap().as_deref(), Some("new-key"));
        assert!(!h.app.state().needs_setup);
    }

    #[test]
    fn a_failed_sign_in_says_why_and_keeps_asking() {
        let h = harness(None, false);
        h.srv.auth_fails.store(true, Ordering::SeqCst);
        let e = h.app.make_key("alex", "x").unwrap_err();
        assert!(e.contains("name or password"), "{e}");
        assert!(h.app.state().needs_password);
    }

    #[test]
    fn idle_stop_ends_the_server() {
        let h = harness(Some("k"), false);
        h.advance(Duration::from_secs(7201));
        h.app.tick();
        assert!(h.dead.load(Ordering::SeqCst));
        assert_eq!(h.app.state().phase, Phase::Stopped);
    }

    #[test]
    fn playback_rolls_the_idle_stop() {
        let h = harness(Some("k"), false);
        *h.srv.sessions.lock().unwrap() = film(true);
        h.advance(Duration::from_secs(3600));
        h.app.tick();
        h.srv.sessions.lock().unwrap().clear();
        h.advance(Duration::from_secs(7200));
        h.app.tick();
        assert!(!h.dead.load(Ordering::SeqCst), "stopped inside the film's window");
    }

    #[test]
    fn a_visible_window_keeps_the_server() {
        let h = harness(Some("k"), false);
        h.advance(Duration::from_secs(7201));
        h.app.seen();
        h.app.tick();
        assert!(!h.dead.load(Ordering::SeqCst));
    }

    #[test]
    fn stop_ends_the_server() {
        let h = harness(Some("k"), false);
        h.app.stop("Stopped from the window.");
        assert!(h.dead.load(Ordering::SeqCst));
        assert_eq!(h.app.state().phase, Phase::Stopped);
    }

    #[test]
    fn jellyfin_exiting_on_its_own_is_shown() {
        let h = harness(Some("k"), false);
        h.dead.store(true, Ordering::SeqCst);
        h.app.check_process();
        let s = h.app.state();
        assert_eq!(s.phase, Phase::Stopped);
        assert!(!s.why.is_empty());
    }

    #[test]
    fn new_auto_stop_settings_apply_at_once() {
        let h = harness(Some("k"), false);
        h.app.set_idle(IdleConfig { start_window: Duration::from_secs(600), margin: Duration::from_secs(1800) });
        h.advance(Duration::from_secs(601));
        h.app.tick();
        assert!(h.dead.load(Ordering::SeqCst), "the shorter window did not apply");
    }

    #[test]
    fn turning_auto_stop_off_keeps_the_server() {
        let h = harness(Some("k"), false);
        h.app.set_idle(IdleConfig::default());
        h.advance(Duration::from_secs(100_000));
        h.app.tick();
        assert!(!h.dead.load(Ordering::SeqCst));
        assert!(h.app.state().idle_stop.is_empty());
    }

    #[test]
    fn the_state_says_whether_a_key_is_stored() {
        assert!(harness(Some("k"), false).app.state().has_key);
        assert!(!harness(None, false).app.state().has_key);
    }

    #[test]
    fn signing_in_again_forgets_the_key_and_asks() {
        let h = harness(Some("k"), false);
        h.app.forget_key();
        assert_eq!(*h.key.lock().unwrap(), None);
        let s = h.app.state();
        assert!(s.needs_password && !s.has_key, "{s:?}");
    }

    #[test]
    fn the_idle_stop_reads_as_time_left() {
        let h = harness(Some("k"), false);
        h.advance(Duration::from_secs(1800));
        assert!(h.app.state().idle_stop.contains("in 1 h 30 min"), "{}", h.app.state().idle_stop);
        assert_eq!(human(Duration::from_secs(59)), "1 min");
        assert_eq!(human(Duration::from_secs(7200)), "2 h");
    }
}
