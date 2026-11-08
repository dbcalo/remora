//! What any window shows, and what it can ask for: pure, so both front-ends
//! (native Win32 on Windows, the Linux UI elsewhere) draw the same thing and are
//! tested once, here. A front-end only paints a `View` and sends `Action`s.
//!
//! Every field always has a value: the windows have fixed layouts, and nothing in
//! them appears or disappears (decided 2026-11-07).

use crate::app::{Phase, State};
use crate::decide::{Fw, FwAction, Os};

/// What the platform knows that the app does not: the firewall plan and Steam.
#[derive(Clone, Debug, PartialEq)]
pub struct Platform {
    pub os: Os,
    pub game: bool,
    pub fw: Fw,
    /// None: Steam is not installed. Some(added).
    pub steam: Option<bool>,
}

/// One frame of the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub phase: String,
    pub running: bool,
    pub start_stop_label: String,
    pub tv: String,
    pub play1: String,
    pub play2: String,
    pub keep: String,
    pub idle_stop: String,
    pub message: String,
    pub message_is_error: bool,
    pub busy: bool,

    pub setup_wanted: bool,
    pub fresh: bool,
    pub setup_button: String,
    pub has_key: bool,
    pub account: String,
    pub account_button: String,

    pub firewall: String,
    pub can_open_zone: bool,
    pub can_mark_private: bool,
    pub steam: String,
    pub can_add_to_steam: bool,

    /// The Users page: one line per user, and their ids in the same order.
    pub users: Vec<String>,
    pub user_ids: Vec<String>,
    pub users_note: String,
    pub can_manage_users: bool,

    /// The Library page: one line per folder, the (library, folder) of each
    /// line in the same order, and the scan.
    pub libraries: Vec<String>,
    pub library_rows: Vec<(String, String)>,
    pub can_edit_libraries: bool,
    pub scan: String,
    pub scanning: bool,
    pub recent: String,
    pub can_scan: bool,
    pub can_restart: bool,
}

/// Everything a window can ask for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    StartStop,
    OpenJellyfin,
    /// Opens the server address, as the TV would, in the default browser.
    OpenAddress,
    /// Opens Jellyfin's logs folder.
    OpenLogs,
    Setup { user: String, password: String, media: String },
    SignInAgain,
    SaveIdle { on: bool, start_minutes: u64, margin_minutes: u64 },
    OpenZone,
    MarkPrivate,
    AddToSteam,
    AddLibrary { name: String, kind: String, folder: String },
    AddFolder { library: String, folder: String },
    RemoveFolder { library: String, folder: String },
    RemoveLibrary { library: String },
    LoadUsers,
    AddUser { name: String, password: String, confirm: String },
    Restart,
    Scan,
    LoadLibrary,
    SetPassword { user_id: String, password: String, confirm: String },
}

pub fn view(s: &State, p: &Platform, busy: bool) -> View {
    let running = s.phase == Phase::Running;
    let busy = busy || s.phase == Phase::Starting;
    let (message, message_is_error) = if !s.error.is_empty() {
        (s.error.clone(), true)
    } else if busy {
        ("Working\u{2026}".to_string(), false)
    } else {
        (s.why.clone(), false)
    };
    let (steam, can_add) = match p.steam {
        None => ("Steam is not installed on this machine.".to_string(), false),
        Some(true) => ("Already added to Steam.".to_string(), false),
        Some(false) if p.os == Os::Windows => {
            ("Not added yet. If Steam opens a window instead, choose Add Selected Programs there.".to_string(), true)
        }
        Some(false) => ("Not added yet: adds this program as a non-Steam game.".to_string(), true),
    };
    View {
        phase: s.phase.label().into(),
        running,
        start_stop_label: if s.phase == Phase::Stopped { "Start server" } else { "Stop server" }.into(),
        tv: s.tv.first().cloned().unwrap_or_else(|| "Not running".into()),
        play1: s.playing.first().cloned().unwrap_or_else(|| "Nothing is playing.".into()),
        play2: match s.playing.len() {
            0 | 1 => String::new(),
            2 => s.playing[1].clone(),
            n => format!("{} and {} more", s.playing[1], n - 2),
        },
        keep: if running { s.keep.clone() } else { "Not holding: the server is not running.".into() },
        idle_stop: match (running, s.idle_stop.is_empty()) {
            (false, _) => "Applies while the server runs.".into(),
            (true, true) => "Off: the server runs until you stop it.".into(),
            (true, false) => s.idle_stop.clone(),
        },
        message,
        message_is_error,
        busy,
        setup_wanted: running && (s.needs_setup || s.needs_password),
        fresh: s.needs_setup,
        setup_button: if s.needs_setup { "Set up Jellyfin" } else { "Sign in" }.into(),
        has_key: s.has_key,
        account: if s.has_key {
            "Signed in: this machine has its own Jellyfin API key."
        } else {
            "Not signed in: keep-awake is off until you sign in."
        }
        .into(),
        account_button: if s.has_key { "Sign in again" } else { "Sign in" }.into(),
        firewall: if p.fw.message.is_empty() { "Nothing needs changing on this network.".into() } else { p.fw.message.clone() },
        can_open_zone: matches!(p.fw.action, FwAction::AskZone(_)) && !busy,
        can_mark_private: p.fw.action == FwAction::OfferPrivate && !busy,
        steam,
        can_add_to_steam: can_add && !p.game && !busy,
        users: s.users.iter().map(|u| u.label()).collect(),
        user_ids: s.users.iter().map(|u| u.id.clone()).collect(),
        users_note: if !running {
            "Start the server to manage its users.".into()
        } else if !s.has_key {
            "Sign in first: managing users needs this machine's key.".into()
        } else {
            "Choose a user and type the new password twice. Jellyfin then signs them out on every device, \
             so sign in again on the TV."
                .into()
        },
        can_manage_users: running && s.has_key && !busy,
        libraries: crate::decide::library_rows(&s.libraries).into_iter().map(|r| r.0).collect(),
        library_rows: crate::decide::library_rows(&s.libraries).into_iter().map(|r| (r.1, r.2)).collect(),
        can_edit_libraries: running && s.has_key && !busy,
        scan: if !running {
            "Start the server to scan its libraries.".into()
        } else if !s.has_key {
            "Sign in first: scanning needs this machine's key.".into()
        } else {
            crate::decide::scan_sentence(&s.scan)
        },
        scanning: running && s.scan.running,
        recent: if running && s.has_key { crate::decide::recent_line(&s.recent) } else { String::new() },
        can_scan: running && s.has_key && !busy && !s.scan.running,
        can_restart: running && !busy,
    }
}

/// Quitting stops the server, so while something plays (not paused) a window
/// asks first.
pub fn quit_needs_confirm(s: &State) -> bool {
    s.playing.iter().any(|p| !p.ends_with("(paused)"))
}

/// The Users page's buttons that submit a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsersButton {
    AddUser,
    SetPassword,
}

/// Where the keyboard focus is on the Users page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsersFocus {
    /// In a New user box (name, password, confirm).
    NewUser,
    /// In the New password or Confirm box.
    Password,
    /// Anywhere else, the users list included.
    Elsewhere,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsersEnter {
    AddUser,
    SetPassword,
    Nothing,
}

/// What a click on a Users button, or Enter (`button` is `None`), does. Enter
/// acts only from the boxes of the form it belongs to: from the list it must do
/// nothing, since a blank SetPassword removes a user's password.
pub fn users_enter(button: Option<UsersButton>, focus: UsersFocus) -> UsersEnter {
    match (button, focus) {
        (Some(UsersButton::AddUser), _) | (None, UsersFocus::NewUser) => UsersEnter::AddUser,
        (Some(UsersButton::SetPassword), _) | (None, UsersFocus::Password) => UsersEnter::SetPassword,
        (None, UsersFocus::Elsewhere) => UsersEnter::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(phase: Phase) -> State {
        State {
            phase,
            why: String::new(),
            error: String::new(),
            tv: if phase == Phase::Running { vec!["http://192.168.1.5:8096".into()] } else { vec![] },
            playing: vec![],
            keep: "Nothing is playing.".into(),
            idle_stop: "Stops itself in 30 min.".into(),
            needs_setup: false,
            needs_password: false,
            has_key: true,
            users: vec![],
            libraries: vec![],
            scan: Default::default(),
            recent: vec![],
        }
    }

    fn plat() -> Platform {
        Platform { os: Os::Windows, game: false, fw: Fw { action: FwAction::Nothing, message: String::new() }, steam: None }
    }

    #[test]
    fn every_row_has_a_value_when_stopped() {
        let v = view(&state(Phase::Stopped), &plat(), false);
        for (name, text) in [("tv", &v.tv), ("play1", &v.play1), ("keep", &v.keep), ("idle", &v.idle_stop), ("firewall", &v.firewall), ("steam", &v.steam), ("account", &v.account)] {
            assert!(!text.is_empty(), "{name} is empty");
        }
        assert_eq!(v.start_stop_label, "Start server");
        assert!(!v.running);
    }

    #[test]
    fn running_shows_the_address_and_a_stop_button() {
        let v = view(&state(Phase::Running), &plat(), false);
        assert_eq!(v.tv, "http://192.168.1.5:8096");
        assert_eq!(v.start_stop_label, "Stop server");
    }

    #[test]
    fn more_than_two_sessions_fold_into_the_second_line() {
        let mut s = state(Phase::Running);
        s.playing = vec!["A on TV".into(), "B on Phone".into(), "C on Deck".into(), "D on Laptop".into()];
        let v = view(&s, &plat(), false);
        assert_eq!((v.play1.as_str(), v.play2.as_str()), ("A on TV", "B on Phone and 2 more"));
    }

    #[test]
    fn an_error_wins_the_message_line_and_busy_says_working() {
        let mut s = state(Phase::Running);
        s.why = "why".into();
        assert_eq!(view(&s, &plat(), true).message, "Working\u{2026}");
        s.error = "bad".into();
        let v = view(&s, &plat(), true);
        assert!(v.message_is_error && v.message == "bad");
    }

    #[test]
    fn setup_is_wanted_only_while_running_without_a_key() {
        let mut s = state(Phase::Running);
        s.needs_password = true;
        let v = view(&s, &plat(), false);
        assert!(v.setup_wanted && v.setup_button == "Sign in");
        s.needs_setup = true;
        assert_eq!(view(&s, &plat(), false).setup_button, "Set up Jellyfin");
        s.phase = Phase::Stopped;
        assert!(!view(&s, &plat(), false).setup_wanted);
    }

    #[test]
    fn network_and_steam_actions_are_enabled_only_when_they_apply() {
        let mut p = plat();
        let s = state(Phase::Running);
        assert!(!view(&s, &p, false).can_open_zone && !view(&s, &p, false).can_mark_private);
        p.fw = Fw { action: FwAction::OfferPrivate, message: "Hotel is Public".into() };
        assert!(view(&s, &p, false).can_mark_private);
        assert!(!view(&s, &p, true).can_mark_private, "busy disables it");
        p.steam = Some(false);
        assert!(view(&s, &p, false).can_add_to_steam);
        p.steam = Some(true);
        let v = view(&s, &p, false);
        assert!(!v.can_add_to_steam && v.steam == "Already added to Steam.");
    }

    #[test]
    fn users_are_managed_only_while_running_signed_in_and_idle() {
        let mut s = state(Phase::Running);
        s.users = vec![crate::decide::User { id: "a1".into(), name: "alex".into(), admin: true, has_password: true }];
        let v = view(&s, &plat(), false);
        assert!(v.can_manage_users);
        assert_eq!((v.users, v.user_ids), (vec!["alex (administrator)".to_string()], vec!["a1".to_string()]));
        assert!(v.users_note.contains("every device"), "{}", v.users_note);
        assert!(!view(&s, &plat(), true).can_manage_users, "busy");
        s.has_key = false;
        let v = view(&s, &plat(), false);
        assert!(!v.can_manage_users && v.users_note.contains("Sign in"), "{}", v.users_note);
        s.phase = Phase::Stopped;
        let v = view(&s, &plat(), false);
        assert!(!v.can_manage_users && v.users_note.contains("Start"), "{}", v.users_note);
    }

    #[test]
    fn the_library_page_scans_only_when_it_can() {
        let mut s = state(Phase::Running);
        s.libraries = vec![crate::decide::Library { name: "Films".into(), paths: vec!["D:\\Films".into()] }];
        let v = view(&s, &plat(), false);
        assert_eq!(v.libraries, vec!["Films: D:\\Films".to_string()]);
        assert_eq!(v.library_rows, vec![("Films".to_string(), "D:\\Films".to_string())]);
        assert!(v.can_scan && v.can_restart && !v.scanning && v.can_edit_libraries);
        assert_eq!(v.scan, "Not scanning.");
        s.scan.running = true;
        let v = view(&s, &plat(), false);
        assert!(v.scanning && !v.can_scan, "no second scan while one runs");
        s.scan.running = false;
        s.has_key = false;
        let v = view(&s, &plat(), false);
        assert!(!v.can_scan && v.can_restart, "restart needs no key");
        assert!(!v.can_edit_libraries);
        assert!(v.scan.contains("Sign in"), "{}", v.scan);
        s.phase = Phase::Stopped;
        let v = view(&s, &plat(), false);
        assert!(!v.can_scan && !v.can_restart);
        assert!(v.scan.contains("Start"), "{}", v.scan);
        assert!(!view(&state(Phase::Running), &plat(), true).can_restart, "busy");
    }

    #[test]
    fn quitting_asks_only_while_something_plays() {
        let mut s = state(Phase::Running);
        assert!(!quit_needs_confirm(&s));
        s.playing = vec!["Film on TV (paused)".into()];
        assert!(!quit_needs_confirm(&s));
        s.playing.push("Film on Phone".into());
        assert!(quit_needs_confirm(&s));
    }

    #[test]
    fn enter_on_the_users_page_only_acts_from_its_own_boxes() {
        use UsersButton as B;
        use UsersFocus as F;
        assert_eq!(users_enter(Some(B::AddUser), F::Elsewhere), UsersEnter::AddUser);
        assert_eq!(users_enter(None, F::NewUser), UsersEnter::AddUser);
        assert_eq!(users_enter(Some(B::SetPassword), F::Elsewhere), UsersEnter::SetPassword);
        assert_eq!(users_enter(None, F::Password), UsersEnter::SetPassword);
        assert_eq!(users_enter(None, F::Elsewhere), UsersEnter::Nothing, "Enter in the list must not set a password");
    }
}
