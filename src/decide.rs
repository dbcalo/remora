//! Every decision the launcher makes, as pure functions: no clock, network,
//! process or file access. Callers feed in what they observed and carry out what
//! these return. Compiled and tested the same on every OS: the OS is a value
//! (`Os`), never a `cfg`.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which operating system a decision is for. A value, so both branches of every
/// decision are tested on every machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

/// Jellyfin's ports: the web/API port the TV connects to, and client discovery.
pub const HTTP_PORT: u16 = 8096;
pub const DISCOVERY_PORT: u16 = 7359;

// --- names ---------------------------------------------------------------------------------------

/// The program's name, as people see it. Jellyfin's branding rules reserve
/// "Jellyfin" for the project's own apps, so it never appears here.
pub const APP_NAME: &str = "Remora";
/// The name in files, the key store, Caffeine and Jellyfin's device list.
pub const APP_ID: &str = "remora";
/// The name before 2026-11-08, read only to move an existing install over.
pub const OLD_APP_ID: &str = "jellyfin-portable";

// --- users -----------------------------------------------------------------------------------

/// A Jellyfin user, as the Users page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub name: String,
    pub admin: bool,
    pub has_password: bool,
}

impl User {
    /// The line the Users page shows.
    pub fn label(&self) -> String {
        if self.admin { format!("{} (administrator)", self.name) } else { self.name.clone() }
    }
}

/// Why a new user cannot be added, or None. Jellyfin refuses a name that
/// differs only in case (400 on 12.4), so that is caught here first.
pub fn new_user_problem(name: &str, password: &str, confirm: &str, existing: &[User]) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        Some("Give the user a name.")
    } else if existing.iter().any(|u| u.name.eq_ignore_ascii_case(name)) {
        Some("A user already has that name.")
    } else if password != confirm {
        Some("The two passwords differ.")
    } else {
        None
    }
}

/// Why a new password cannot be set, or None. Blank removes a password, which
/// suits a family member's TV profile but never an administrator: every Jellyfin
/// admin can reach the whole server from the network the TV is on.
pub fn password_problem(admin: bool, password: &str, confirm: &str) -> Option<&'static str> {
    if password != confirm {
        Some("The two passwords differ.")
    } else if admin && password.is_empty() {
        Some("An administrator needs a password.")
    } else {
        None
    }
}

// --- libraries ---------------------------------------------------------------------------------

/// A Jellyfin library and its folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub paths: Vec<String>,
}

/// One row of the Library page: (line, library, folder). One row per folder,
/// so choosing a row chooses both; a library without folders gets one row with
/// an empty folder.
pub fn library_rows(libraries: &[Library]) -> Vec<(String, String, String)> {
    let mut rows = vec![];
    for l in libraries {
        if l.paths.is_empty() {
            rows.push((format!("{}: no folder", l.name), l.name.clone(), String::new()));
        }
        for p in &l.paths {
            rows.push((format!("{}: {p}", l.name), l.name.clone(), p.clone()));
        }
    }
    rows
}

/// The library types offered when adding one: (shown, Jellyfin's collectionType).
pub const LIBRARY_KINDS: [(&str, &str); 5] =
    [("Films", "movies"), ("TV shows", "tvshows"), ("Music", "music"), ("Home videos and photos", "homevideos"), ("Mixed", "mixed")];

/// Why a new library cannot be added, or None. Jellyfin itself accepts a second
/// library of the same name and silently calls it "Name2" (checked on 12.4).
pub fn library_problem(name: &str, folder: &str, existing: &[Library]) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        Some("Give the library a name.")
    } else if folder.trim().is_empty() {
        Some("Choose the library's folder.")
    } else if existing.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
        Some("A library already has that name.")
    } else {
        None
    }
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
pub fn encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// An item Jellyfin added lately, for the Library page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub name: String,
    pub year: Option<u32>,
}

/// The Library page's "Recently added" line.
pub fn recent_line(items: &[Recent]) -> String {
    if items.is_empty() {
        return "Nothing added yet.".into();
    }
    let names: Vec<String> = items
        .iter()
        .map(|r| match r.year {
            Some(y) => format!("{} ({y})", r.name),
            None => r.name.clone(),
        })
        .collect();
    names.join(", ")
}

/// Jellyfin's "Scan Media Library" task, as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    pub running: bool,
    pub percent: Option<f64>,
    /// The last finished run's status: "Completed", "Failed", "Cancelled"...
    pub last: Option<String>,
}

/// What the Library page says about scanning.
pub fn scan_sentence(s: &Scan) -> String {
    if s.running {
        return match s.percent {
            Some(p) => format!("Scanning: {}% done.", p.floor() as u32),
            None => "Scanning: starting.".into(),
        };
    }
    match s.last.as_deref() {
        Some("Completed") => "Not scanning. The last scan finished.".into(),
        Some("Failed") => "Not scanning. The last scan failed; Jellyfin's logs say why.".into(),
        _ => "Not scanning.".into(),
    }
}

// --- playback and keep-awake ----------------------------------------------------------

/// The part of a Jellyfin /Sessions entry the launcher reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub device: String,
    pub client: String,
    pub item: Option<Item>,
    pub position_ticks: i64,
    pub paused: bool,
}

/// A session's NowPlayingItem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub name: String,
    /// 0 when unknown (live TV, a broken item).
    pub runtime_ticks: i64,
}

/// Jellyfin's time unit is 100 ns.
const TICK_NANOS: i64 = 100;

/// Every session with something loaded and not paused. Only these hold the
/// machine awake: a paused film lets it sleep once Caffeine's grace runs out,
/// by design (paused for 30 minutes clears the hold).
pub fn playing(sessions: &[Session]) -> Vec<&Session> {
    sessions.iter().filter(|s| s.item.is_some() && !s.paused).collect()
}

/// How often the hold is renewed while playing, and how long Caffeine keeps one
/// after the last renewal (its contract's grace). The gap lets a late poll miss a
/// renewal without the machine sleeping mid-film.
pub const HOLD_EVERY: Duration = Duration::from_secs(5 * 60);
pub const CAFFEINE_GRACE: Duration = Duration::from_secs(30 * 60);

/// Whether to call `caffeine hold` now. `since_last_hold` is None before the
/// first. Nothing playing means no renewal, so the claim lapses CAFFEINE_GRACE
/// after the last one.
pub fn hold_due(playing: bool, since_last_hold: Option<Duration>) -> bool {
    playing && since_last_hold.is_none_or(|d| d >= HOLD_EVERY)
}

/// What the built-in Windows hold should do now: `Some(true)` to take the
/// request, `Some(false)` to drop it, `None` to leave it. `held` is whether the
/// request is taken; `until` when the last renewal runs out.
pub fn hold_step(held: bool, until: Option<std::time::Instant>, now: std::time::Instant) -> Option<bool> {
    let wanted = until.is_some_and(|u| now < u);
    (wanted != held).then_some(wanted)
}

/// The built-in Linux hold: a logind inhibitor that ends by itself after
/// `seconds`, renewed by starting another.
pub fn linux_inhibit_argv(seconds: u64) -> Vec<String> {
    ["systemd-inhibit", "--what=idle:sleep", "--who=Remora", "--why=playing to a TV", "--mode=block", "sleep"]
        .map(String::from)
        .into_iter()
        .chain([seconds.to_string()])
        .collect()
}

/// What the last `caffeine hold` said about keep-awake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Keep {
    #[default]
    Unknown,
    /// exit 0
    Held,
    /// exit 3: stopped from the Caffeine row
    Paused,
    /// exit 4: no ~/.caffeine
    NotInstalled,
    /// exit 64: a Caffeine without `hold`
    TooOld,
    /// anything else, including not runnable
    Failed,
}

/// `caffeine hold`'s exit code (docs/contract.md, version 2). None means the
/// program could not be run at all.
pub fn keep_from_exit(code: Option<i32>) -> Keep {
    match code {
        Some(0) => Keep::Held,
        Some(3) => Keep::Paused,
        Some(4) => Keep::NotInstalled,
        Some(64) => Keep::TooOld,
        _ => Keep::Failed,
    }
}

// --- the idle auto-stop ---------------------------------------------------------------

/// The "forgot it was running" clock. The default is off.
///
/// A ROLLING CLOCK ON WHAT IS LOADED (decided 2026-11-07): every poll that sees an
/// item loaded, playing or paused, pushes the stop out to the time left in that
/// item plus `margin`. A paused film keeps the SERVER up for as long as it could
/// still be finished, so the TV keeps its place; the MACHINE may still sleep,
/// because keep-awake follows only unpaused playback.
///
/// Times are seconds on the caller's monotonic clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IdleConfig {
    /// From start, before anything has played.
    pub start_window: Duration,
    /// Added to the time left, and the window an open (visible) launcher buys.
    pub margin: Duration,
}

impl IdleConfig {
    fn off(&self) -> bool {
        self.start_window.is_zero() && self.margin.is_zero()
    }

    /// The stop deadline at start, as an offset from start. None means never.
    pub fn initial(&self, now: Duration) -> Option<Duration> {
        (!self.off()).then(|| now + self.start_window)
    }

    /// The deadline after one poll. It never moves earlier: a short episode
    /// after a long film must not cut the film's window short.
    pub fn next(&self, prev: Option<Duration>, now: Duration, sessions: &[Session], visible: bool) -> Option<Duration> {
        if self.off() {
            return None;
        }
        let mut next = prev;
        let mut push = |t: Duration| {
            if next.is_none_or(|n| t > n) {
                next = Some(t);
            }
        };
        if visible {
            push(now + self.margin);
        }
        for s in sessions.iter().filter(|s| s.item.is_some()) {
            push(now + time_left(s) + self.margin);
        }
        next
    }

    /// Whether the server should stop now. No deadline never is.
    pub fn due(&self, deadline: Option<Duration>, now: Duration) -> bool {
        deadline.is_some_and(|d| now >= d)
    }
}

/// What remains of a session's item, or zero when that is unknown or nonsense
/// (no runtime, a position past the end).
fn time_left(s: &Session) -> Duration {
    match &s.item {
        Some(i) if i.runtime_ticks > 0 && s.position_ticks <= i.runtime_ticks => {
            Duration::from_nanos(((i.runtime_ticks - s.position_ticks.max(0)) * TICK_NANOS) as u64)
        }
        _ => Duration::ZERO,
    }
}

// --- firewall ---------------------------------------------------------------------------

/// What the launcher observed about this machine's network and firewall.
#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    pub os: Os,
    // Windows
    /// Both Private-profile rules exist.
    pub rules_present: bool,
    /// Connected networks whose category is Public.
    pub public_networks: Vec<String>,
    // Linux
    pub firewall: Firewall,
    /// firewalld: the active zone of the current connection.
    pub zone: String,
    /// firewalld: both ports already open in `zone`.
    pub ports_open: bool,
    /// firewalld: the user approved opening ports in `zone` before.
    pub zone_approved: bool,
}

impl Net {
    pub fn new(os: Os) -> Net {
        Net {
            os,
            rules_present: false,
            public_networks: vec![],
            firewall: Firewall::None,
            zone: String::new(),
            ports_open: false,
            zone_approved: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firewall {
    None,
    Firewalld,
    Ufw,
}

/// What to do about the firewall before starting Jellyfin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FwAction {
    Nothing,
    /// One UAC prompt: Private-profile rules only.
    AddWindowsRules,
    /// Ask whether to mark a Public network Private.
    OfferPrivate,
    /// firewalld, non-public zone, first time: ask.
    AskZone(String),
    /// firewalld, non-public zone, approved before: open.
    OpenZone(String),
    /// firewalld public zone: open nothing, say why.
    RefusePublic,
    /// ufw: report, never edit (v1).
    ReportOnly,
    /// Nothing to open; say the machine has none.
    NoFirewall,
}

/// A firewall decision and the one sentence the window shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fw {
    pub action: FwAction,
    pub message: String,
}

/// The network-safety step. THE ONE RULE: nothing is ever opened on an untrusted
/// network. Windows rules are Private-profile only, so they open nothing on a
/// Public network until the user marks it Private; firewalld opens only a zone that
/// is not `public`, the Linux analogue, with no safe default to flip.
pub fn firewall_plan(n: &Net) -> Fw {
    let fw = |action, message: &str| Fw { action, message: message.to_string() };
    if n.os == Os::Windows {
        if !n.rules_present {
            return fw(
                FwAction::AddWindowsRules,
                "Jellyfin needs firewall rules for Private networks; Windows will ask for permission once.",
            );
        }
        if !n.public_networks.is_empty() {
            return Fw {
                action: FwAction::OfferPrivate,
                message: format!(
                    "{} is a Public network, so the TV cannot connect. Mark it Private only if you trust it.",
                    n.public_networks.join(", ")
                ),
            };
        }
        return fw(FwAction::Nothing, "");
    }
    match n.firewall {
        Firewall::Firewalld if n.ports_open => fw(FwAction::Nothing, ""),
        Firewall::Firewalld if n.zone.is_empty() || n.zone == "public" => fw(
            FwAction::RefusePublic,
            "This network is in firewalld's public zone, so nothing is opened and the TV cannot connect. \
             Put the connection in a trusted zone (home, work) only if you trust it.",
        ),
        Firewall::Firewalld if n.zone_approved => fw(FwAction::OpenZone(n.zone.clone()), ""),
        Firewall::Firewalld => Fw {
            action: FwAction::AskZone(n.zone.clone()),
            message: format!(
                "Open Jellyfin's ports in firewalld's \"{}\" zone? You will be asked for your password once.",
                n.zone
            ),
        },
        Firewall::Ufw => Fw {
            action: FwAction::ReportOnly,
            message: format!(
                "ufw is active; allow TCP {HTTP_PORT} and UDP {DISCOVERY_PORT} yourself if the TV cannot connect."
            ),
        },
        Firewall::None => fw(
            FwAction::NoFirewall,
            "This machine has no firewall, so anyone on this network can reach Jellyfin while it runs.",
        ),
    }
}

/// Opens Jellyfin's two ports in `zone`, and nothing else. Run twice, once
/// without and once with `permanent`, so the change applies now and survives a
/// reload; the caller runs it through pkexec.
pub fn firewalld_open_argv(zone: &str, permanent: bool) -> Vec<String> {
    let mut argv = vec!["firewall-cmd".to_string()];
    if permanent {
        argv.push("--permanent".into());
    }
    argv.push(format!("--zone={zone}"));
    argv.push(format!("--add-port={HTTP_PORT}/tcp"));
    argv.push(format!("--add-port={DISCOVERY_PORT}/udp"));
    argv
}

/// The two Windows firewall rules, named as the PowerShell launcher named them,
/// so rules it created are recognised rather than duplicated.
pub const WINDOWS_RULE_NAMES: [&str; 2] = ["Jellyfin HTTP (Private)", "Jellyfin Discovery (Private)"];

/// Adds the two rules: inbound, Private profile ONLY, and only for this
/// jellyfin.exe. Never widen them: they are the whole safety story for a server
/// left running by accident on public Wi-Fi.
pub fn windows_rule_argvs(exe: &str) -> [Vec<String>; 2] {
    let rule = |name: &str, proto: &str, port: u16| -> Vec<String> {
        [
            "netsh", "advfirewall", "firewall", "add", "rule",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain([
            format!("name={name}"),
            "dir=in".into(),
            "action=allow".into(),
            format!("program={exe}"),
            format!("protocol={proto}"),
            format!("localport={port}"),
            "profile=private".into(),
        ])
        .collect()
    };
    [
        rule(WINDOWS_RULE_NAMES[0], "TCP", HTTP_PORT),
        rule(WINDOWS_RULE_NAMES[1], "UDP", DISCOVERY_PORT),
    ]
}

// --- Steam ------------------------------------------------------------------------------

/// Whether Steam launched this binary inside a gamescope session: Game Mode on
/// the Deck, where the window goes full-screen. Steam sets SteamGameId for any
/// game it launches, Desktop Mode included, so that alone is not enough.
pub fn game_mode(getenv: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| getenv(k).is_some_and(|v| !v.is_empty());
    set("SteamGameId") && set("GAMESCOPE_WAYLAND_DISPLAY")
}

/// Whether any shortcuts.vdf already names this binary. A byte search is enough:
/// the file is read, never parsed or written (Steam rewrites it on exit,
/// discarding any outside edit). Windows paths compare case-insensitively.
pub fn added_to_steam(os: Os, vdfs: &[Vec<u8>], binary: &str) -> bool {
    let fold = |b: &[u8]| -> Vec<u8> {
        if os == Os::Windows { b.to_ascii_lowercase() } else { b.to_vec() }
    };
    let needle = fold(binary.as_bytes());
    !needle.is_empty() && vdfs.iter().any(|v| fold(v).windows(needle.len()).any(|w| w == needle.as_slice()))
}

/// SteamOS's own helper, the one behind Dolphin's "Add to Steam": it hands the
/// path to the running client without a restart.
pub fn add_to_steam_argv(binary: &str) -> Vec<String> {
    vec!["steamos-add-to-steam".into(), binary.into()]
}

/// The steam:// URL that helper uses, for every other system with Steam. NOT YET
/// VERIFIED to add silently on Windows or desktop Linux.
pub fn add_to_steam_url(binary: &str) -> String {
    let mut out = String::from("steam://addnonsteamgame/");
    for b in binary.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// --- processes and addresses ----------------------------------------------------------------

/// Jellyfin with every piece of its state inside `dir`, so the folder is the
/// whole install. The release is unpacked into `dir/jellyfin` (Windows: the zip,
/// with jellyfin.exe and ffmpeg.exe; Linux: the generic tarball, with the portable
/// jellyfin-ffmpeg tarball beside it in `dir/jellyfin-ffmpeg`). Data, cache and
/// logs sit beside the release, not in it, so an upgrade replaces `jellyfin`
/// whole and keeps them; Jellyfin's database also stores absolute paths into
/// `dir/data`, which therefore must not move.
pub fn jellyfin_argv(os: Os, dir: &Path) -> Vec<PathBuf> {
    let release = dir.join("jellyfin");
    let (server, web, ffmpeg) = match os {
        Os::Windows => (release.join("jellyfin.exe"), release.join("jellyfin-web"), release.join("ffmpeg.exe")),
        Os::Linux => (
            dir.join("jellyfin").join("jellyfin"),
            dir.join("jellyfin").join("jellyfin-web"),
            dir.join("jellyfin-ffmpeg").join("ffmpeg"),
        ),
    };
    vec![
        server,
        "--datadir".into(),
        dir.join("data"),
        "--cachedir".into(),
        dir.join("cache"),
        "--logdir".into(),
        dir.join("logs"),
        "--webdir".into(),
        web,
        "--ffmpeg".into(),
        ffmpeg,
    ]
}

/// Opens a URL (Jellyfin's own web UI) in the default browser. rundll32 rather
/// than `cmd /c start`, which reads `&` in a URL as a command separator.
pub fn open_url_argv(os: Os, url: &str) -> Vec<String> {
    match os {
        Os::Windows => vec!["rundll32".into(), "url.dll,FileProtocolHandler".into(), url.into()],
        Os::Linux => vec!["xdg-open".into(), url.into()],
    }
}

/// Opens a folder in the file manager.
pub fn open_folder_argv(os: Os, dir: &Path) -> Vec<String> {
    let manager = if os == Os::Windows { "explorer.exe" } else { "xdg-open" };
    vec![manager.into(), dir.display().to_string()]
}

/// One network interface as the launcher saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Iface {
    pub name: String,
    pub ipv4: Vec<String>,
    /// Carries the default route.
    pub default_route: bool,
    pub loopback: bool,
    /// VPN, container or hypervisor adapter.
    pub virtual_: bool,
}

/// What to type into the TV: the interface carrying the default route, the one
/// on the network the TV shares. With none, every real interface.
pub fn tv_addresses(ifs: &[Iface], port: u16) -> Vec<String> {
    let pick = |want: &dyn Fn(&Iface) -> bool| -> Vec<String> {
        ifs.iter()
            .filter(|f| !f.loopback && !f.virtual_ && want(f))
            .flat_map(|f| f.ipv4.iter().map(move |ip| format!("http://{ip}:{port}")))
            .collect()
    };
    let routed = pick(&|f| f.default_route);
    if routed.is_empty() { pick(&|_| true) } else { routed }
}

/// Prefixes naming VPN, container and hypervisor adapters, which never carry
/// the network the TV is on.
const VIRTUAL_PREFIXES: &[&str] = &[
    "wg", "wireguard", "tun", "tap", "docker", "virbr", "veth", "vethernet", "br-", "tailscale",
    "zerotier", "vmnet", "vboxnet", "utun", "lxc", "cni", "flannel",
];

/// Whether an interface name is a virtual adapter.
pub fn virtual_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    VIRTUAL_PREFIXES.iter().any(|p| n.starts_with(p))
}

// --- theme ---------------------------------------------------------------------------

/// Whether to draw dark, from Windows' "apps use light theme" setting
/// (HKCU Software/Microsoft/Windows/CurrentVersion/Themes/Personalize,
/// AppsUseLightTheme): 0 is dark. No value is Windows' default, light. GTK and Qt
/// follow their desktop's theme themselves.
pub fn dark_theme(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    fn ticks(d: Duration) -> i64 {
        (d.as_nanos() / 100) as i64
    }

    fn loaded(paused: bool, runtime: Duration, position: Duration) -> Session {
        Session {
            device: "TV".into(),
            item: Some(Item { name: "Film".into(), runtime_ticks: ticks(runtime) }),
            position_ticks: ticks(position),
            paused,
            ..Default::default()
        }
    }

    #[test]
    fn playing_is_loaded_and_not_paused() {
        let s = [Session { device: "idle".into(), ..Default::default() }, loaded(true, mins(90), mins(0)), loaded(false, mins(90), mins(0))];
        let p = playing(&s);
        assert_eq!(p.len(), 1);
        assert!(!p[0].paused);
    }

    #[test]
    fn user_labels_say_administrator_only() {
        // Jellyfin 12.4 reports HasPassword true even for an empty password, so
        // the label never claims to know.
        let u = |admin, has_password| User { id: "1".into(), name: "alex".into(), admin, has_password };
        assert_eq!(u(true, true).label(), "alex (administrator)");
        assert_eq!(u(true, false).label(), "alex (administrator)");
        assert_eq!(u(false, true).label(), "alex");
        assert_eq!(u(false, false).label(), "alex");
    }

    #[test]
    fn a_new_user_needs_a_free_name_and_matching_passwords() {
        let alex = [User { id: "1".into(), name: "alex".into(), admin: true, has_password: true }];
        assert_eq!(new_user_problem("Kids", "", "", &alex), None);
        assert_eq!(new_user_problem("  ", "", "", &alex), Some("Give the user a name."));
        assert_eq!(new_user_problem("ALEX", "a", "a", &alex), Some("A user already has that name."));
        assert_eq!(new_user_problem("Kids", "a", "b", &alex), Some("The two passwords differ."));
    }

    #[test]
    fn a_password_must_match_and_an_admin_needs_one() {
        assert_eq!(password_problem(false, "a", "a"), None);
        assert_eq!(password_problem(false, "a", "b"), Some("The two passwords differ."));
        assert_eq!(password_problem(false, "", ""), None, "blank removes a TV profile's password");
        assert_eq!(password_problem(true, "", ""), Some("An administrator needs a password."));
        assert_eq!(password_problem(true, "a", "a"), None);
    }

    fn lib(name: &str, paths: &[&str]) -> Library {
        Library { name: name.into(), paths: paths.iter().map(|p| p.to_string()).collect() }
    }

    #[test]
    fn library_rows_are_one_per_folder() {
        let rows = library_rows(&[lib("Films", &[r"D:\A", r"E:\B"]), lib("Music", &[])]);
        assert_eq!(
            rows,
            vec![
                (r"Films: D:\A".to_string(), "Films".to_string(), r"D:\A".to_string()),
                (r"Films: E:\B".to_string(), "Films".to_string(), r"E:\B".to_string()),
                ("Music: no folder".to_string(), "Music".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn a_new_library_needs_a_name_a_folder_and_a_new_name() {
        let have = [lib("Films", &[r"D:\A"])];
        assert_eq!(library_problem("Kids", r"D:\K", &have), None);
        assert_eq!(library_problem("  ", r"D:\K", &have), Some("Give the library a name."));
        assert_eq!(library_problem("Kids", " ", &have), Some("Choose the library's folder."));
        assert_eq!(library_problem("films", r"D:\K", &have), Some("A library already has that name."));
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_query("TV Shows"), "TV%20Shows");
        assert_eq!(encode_query(r"C:\Media\Kids & Co"), "C%3A%5CMedia%5CKids%20%26%20Co");
        assert_eq!(encode_query("a-b_c.d~é"), "a-b_c.d~%C3%A9");
    }

    #[test]
    fn open_folder_uses_the_file_manager() {
        let d = Path::new("/jf/logs");
        assert_eq!(open_folder_argv(Os::Windows, d), vec!["explorer.exe".to_string(), d.display().to_string()]);
        assert_eq!(open_folder_argv(Os::Linux, d), vec!["xdg-open".to_string(), d.display().to_string()]);
    }

    #[test]
    fn recent_line_lists_names_with_years() {
        assert_eq!(recent_line(&[]), "Nothing added yet.");
        let items = [Recent { name: "Moana".into(), year: Some(2016) }, Recent { name: "Clip".into(), year: None }];
        assert_eq!(recent_line(&items), "Moana (2016), Clip");
    }

    #[test]
    fn the_scan_sentence_says_progress_and_failure() {
        let s = |running, percent, last: Option<&str>| Scan { running, percent, last: last.map(String::from) };
        assert_eq!(scan_sentence(&s(true, Some(48.4), None)), "Scanning: 48% done.");
        assert_eq!(scan_sentence(&s(true, None, None)), "Scanning: starting.");
        assert_eq!(scan_sentence(&s(false, None, Some("Completed"))), "Not scanning. The last scan finished.");
        assert_eq!(scan_sentence(&s(false, None, Some("Failed"))), "Not scanning. The last scan failed; Jellyfin's logs say why.");
        assert_eq!(scan_sentence(&s(false, None, None)), "Not scanning.");
    }

    #[test]
    fn the_builtin_hold_takes_then_drops_the_request() {
        let t0 = std::time::Instant::now();
        let until = Some(t0 + CAFFEINE_GRACE);
        assert_eq!(hold_step(false, until, t0), Some(true));
        assert_eq!(hold_step(true, until, t0 + mins(29)), None);
        assert_eq!(hold_step(true, until, t0 + mins(31)), Some(false));
        assert_eq!(hold_step(false, until, t0 + mins(31)), None);
        assert_eq!(hold_step(false, None, t0), None);
        // A renewal at 20 minutes moves the end to 50.
        assert_eq!(hold_step(true, Some(t0 + mins(20) + CAFFEINE_GRACE), t0 + mins(45)), None);
    }

    #[test]
    fn linux_inhibit_argv_is_exact() {
        assert_eq!(
            linux_inhibit_argv(1800),
            ["systemd-inhibit", "--what=idle:sleep", "--who=Remora", "--why=playing to a TV", "--mode=block", "sleep", "1800"]
                .map(String::from)
                .to_vec()
        );
    }

    #[test]
    fn hold_is_renewed_every_five_minutes_while_playing() {
        assert!(hold_due(true, None));
        assert!(!hold_due(true, Some(mins(4))));
        assert!(hold_due(true, Some(mins(5))));
        assert!(!hold_due(false, None));
        assert!(!hold_due(false, Some(mins(60))));
    }

    #[test]
    fn renewal_stays_well_inside_caffeines_grace() {
        assert!(HOLD_EVERY * 3 <= CAFFEINE_GRACE);
    }

    #[test]
    fn caffeine_exit_codes() {
        assert_eq!(keep_from_exit(Some(0)), Keep::Held);
        assert_eq!(keep_from_exit(Some(3)), Keep::Paused);
        assert_eq!(keep_from_exit(Some(4)), Keep::NotInstalled);
        assert_eq!(keep_from_exit(Some(64)), Keep::TooOld);
        assert_eq!(keep_from_exit(Some(1)), Keep::Failed);
        assert_eq!(keep_from_exit(None), Keep::Failed);
    }

    const IDLE: IdleConfig = IdleConfig { start_window: Duration::from_secs(7200), margin: Duration::from_secs(1800) };

    #[test]
    fn idle_stop_starts_with_the_start_window() {
        assert_eq!(IDLE.initial(mins(1)), Some(mins(121)));
    }

    #[test]
    fn idle_stop_rolls_with_time_left_in_the_item() {
        // 100 minutes into a 3-hour film: 80 left, plus the margin.
        let s = [loaded(false, mins(180), mins(100))];
        assert_eq!(IDLE.next(Some(mins(10)), mins(0), &s, false), Some(mins(110)));
    }

    #[test]
    fn idle_stop_counts_a_paused_items_remaining_time() {
        let c = IdleConfig { start_window: mins(10), margin: mins(30) };
        let s = [loaded(true, mins(120), mins(60))];
        assert_eq!(c.next(Some(mins(10)), mins(0), &s, false), Some(mins(90)));
    }

    #[test]
    fn idle_stop_never_moves_earlier() {
        let s = [loaded(false, mins(30), mins(25))];
        assert_eq!(IDLE.next(Some(mins(200)), mins(0), &s, false), Some(mins(200)));
    }

    #[test]
    fn idle_stop_rolls_while_the_window_is_visible() {
        let c = IdleConfig { start_window: mins(10), margin: mins(30) };
        assert_eq!(c.next(Some(mins(10)), mins(0), &[], true), Some(mins(30)));
    }

    #[test]
    fn idle_stop_uses_the_margin_when_runtime_is_unknown() {
        let c = IdleConfig { start_window: mins(10), margin: mins(30) };
        for s in [loaded(false, Duration::ZERO, mins(5)), loaded(false, mins(60), mins(70))] {
            assert_eq!(c.next(Some(mins(10)), mins(0), &[s], false), Some(mins(30)));
        }
    }

    #[test]
    fn idle_stop_off() {
        let c = IdleConfig::default();
        assert_eq!(c.initial(mins(0)), None);
        assert_eq!(c.next(None, mins(0), &[], true), None);
        assert!(!c.due(None, mins(1_000_000)));
    }

    #[test]
    fn idle_stop_due_exactly_at_the_deadline() {
        assert!(!IDLE.due(Some(mins(10)), mins(9)));
        assert!(IDLE.due(Some(mins(10)), mins(10)));
    }

    fn windows(rules: bool, public: &[&str]) -> Net {
        Net { rules_present: rules, public_networks: public.iter().map(|s| s.to_string()).collect(), ..Net::new(Os::Windows) }
    }

    fn firewalld(zone: &str, approved: bool, open: bool) -> Net {
        Net { firewall: Firewall::Firewalld, zone: zone.into(), zone_approved: approved, ports_open: open, ..Net::new(Os::Linux) }
    }

    #[test]
    fn windows_firewall_adds_private_rules_once() {
        assert_eq!(firewall_plan(&windows(false, &[])).action, FwAction::AddWindowsRules);
        assert_eq!(firewall_plan(&windows(true, &[])).action, FwAction::Nothing);
    }

    #[test]
    fn windows_offers_to_mark_a_public_network_private() {
        let f = firewall_plan(&windows(true, &["Hotel"]));
        assert_eq!(f.action, FwAction::OfferPrivate);
        assert!(f.message.contains("Hotel"));
    }

    #[test]
    fn firewalld_never_opens_the_public_zone() {
        for approved in [false, true] {
            assert_eq!(firewall_plan(&firewalld("public", approved, false)).action, FwAction::RefusePublic);
        }
        assert_eq!(firewall_plan(&firewalld("", true, false)).action, FwAction::RefusePublic);
    }

    #[test]
    fn firewalld_asks_once_then_opens_automatically() {
        assert_eq!(firewall_plan(&firewalld("home", false, false)).action, FwAction::AskZone("home".into()));
        assert_eq!(firewall_plan(&firewalld("home", true, false)).action, FwAction::OpenZone("home".into()));
        assert_eq!(firewall_plan(&firewalld("home", false, true)).action, FwAction::Nothing);
    }

    #[test]
    fn ufw_is_reported_not_edited_and_no_firewall_says_so() {
        let ufw = Net { firewall: Firewall::Ufw, ..Net::new(Os::Linux) };
        assert_eq!(firewall_plan(&ufw).action, FwAction::ReportOnly);
        let none = firewall_plan(&Net::new(Os::Linux));
        assert_eq!(none.action, FwAction::NoFirewall);
        assert!(!none.message.is_empty());
    }

    #[test]
    fn firewalld_opens_only_jellyfins_ports() {
        assert_eq!(firewalld_open_argv("home", false), ["firewall-cmd", "--zone=home", "--add-port=8096/tcp", "--add-port=7359/udp"]);
        assert_eq!(
            firewalld_open_argv("home", true),
            ["firewall-cmd", "--permanent", "--zone=home", "--add-port=8096/tcp", "--add-port=7359/udp"]
        );
    }

    #[test]
    fn windows_rules_are_private_only_and_for_jellyfin_alone() {
        let exe = r"C:\Portable\Jellyfin\jellyfin.exe";
        let rules = windows_rule_argvs(exe);
        for argv in &rules {
            let joined = argv.join(" ");
            for must in ["profile=private", "dir=in", "action=allow", &format!("program={exe}")] {
                assert!(joined.contains(must), "{joined} lacks {must}");
            }
            assert!(!joined.contains("public") && !joined.contains("profile=any"), "{joined} widens the rule");
        }
        assert!(rules[0].join(" ").contains("protocol=TCP localport=8096"));
        assert!(rules[1].join(" ").contains("protocol=UDP localport=7359"));
    }

    #[test]
    fn game_mode_needs_a_steam_game_inside_gamescope() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert!(game_mode(env(&[("SteamGameId", "123"), ("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-0")])));
        assert!(!game_mode(env(&[("SteamGameId", "123")])));
        assert!(!game_mode(env(&[("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-0")])));
        assert!(!game_mode(env(&[])));
    }

    #[test]
    fn already_added_to_steam_is_a_byte_search() {
        let vdf = b"\x00shortcuts\x00\x00\x00\x01Exe\x00\"/home/deck/jf/remora\"\x00".to_vec();
        assert!(added_to_steam(Os::Linux, &[vdf.clone()], "/home/deck/jf/remora"));
        assert!(!added_to_steam(Os::Linux, &[vdf], "/home/deck/other"));
        let win = b"Exe\x00\"C:\\Portable\\JF\\remora.exe\"\x00".to_vec();
        assert!(added_to_steam(Os::Windows, &[win], r"c:\portable\jf\remora.exe"));
        assert!(!added_to_steam(Os::Linux, &[b"x".to_vec()], ""));
    }

    #[test]
    fn add_to_steam_helper_and_url() {
        assert_eq!(add_to_steam_argv("/home/deck/jf/x"), ["steamos-add-to-steam", "/home/deck/jf/x"]);
        assert_eq!(
            add_to_steam_url(r"C:\Portable\My JF\remora.exe"),
            "steam://addnonsteamgame/C:%5CPortable%5CMy%20JF%5Cremora.exe"
        );
    }

    #[test]
    fn the_release_is_in_jellyfin_and_the_state_beside_it() {
        // The release is unzipped into jellyfin\, so an upgrade replaces that folder
        // whole; data, cache and logs stay at the top and survive it.
        let dir = Path::new("C:/Portable/Jellyfin");
        let release = dir.join("jellyfin");
        let got = jellyfin_argv(Os::Windows, dir);
        let want: Vec<PathBuf> = vec![
            release.join("jellyfin.exe"),
            "--datadir".into(),
            dir.join("data"),
            "--cachedir".into(),
            dir.join("cache"),
            "--logdir".into(),
            dir.join("logs"),
            "--webdir".into(),
            release.join("jellyfin-web"),
            "--ffmpeg".into(),
            release.join("ffmpeg.exe"),
        ];
        assert_eq!(got, want);
        let linux = jellyfin_argv(Os::Linux, Path::new("/home/deck/Jellyfin"));
        assert_eq!(linux[0], Path::new("/home/deck/Jellyfin").join("jellyfin").join("jellyfin"));
        assert_eq!(linux[10], Path::new("/home/deck/Jellyfin").join("jellyfin-ffmpeg").join("ffmpeg"));
    }

    #[test]
    fn open_url_per_os() {
        let u = "http://127.0.0.1:8096/web/#/home?a=1&b=2";
        assert_eq!(open_url_argv(Os::Windows, u), ["rundll32", "url.dll,FileProtocolHandler", u]);
        assert_eq!(open_url_argv(Os::Linux, u), ["xdg-open", u]);
    }

    #[test]
    fn tv_address_is_the_default_route_interface_else_every_real_one() {
        let iface = |name: &str, ip: &str, route: bool, lo: bool, virt: bool| Iface {
            name: name.into(),
            ipv4: vec![ip.into()],
            default_route: route,
            loopback: lo,
            virtual_: virt,
        };
        let ifs = [iface("wg0", "10.5.5.2", false, false, true), iface("wlan0", "192.168.1.162", true, false, false), iface("eth0", "192.168.50.3", false, false, false)];
        assert_eq!(tv_addresses(&ifs, 8096), ["http://192.168.1.162:8096"]);
        let ifs = [iface("lo", "127.0.0.1", false, true, false), iface("wg0", "10.5.5.2", false, false, true), iface("eth0", "192.168.50.3", false, false, false)];
        assert_eq!(tv_addresses(&ifs, 8096), ["http://192.168.50.3:8096"]);
    }

    #[test]
    fn the_theme_follows_windows_apps_setting() {
        // HKCU\...\Themes\Personalize\AppsUseLightTheme: 0 is dark, 1 light.
        assert!(dark_theme(Some(0)));
        assert!(!dark_theme(Some(1)));
        // No value (older Windows, or never set) is Windows' own default: light.
        assert!(!dark_theme(None));
    }

    #[test]
    fn virtual_interfaces_are_recognised_by_name() {
        for n in ["wg0", "WireGuard Tunnel", "tun0", "tap0", "docker0", "virbr0", "veth1a2b", "vEthernet (WSL)", "br-12ab", "tailscale0", "ZeroTier One"] {
            assert!(virtual_name(n), "{n} should be virtual");
        }
        for n in ["wlan0", "eth0", "enp3s0", "wlp2s0", "Wi-Fi", "Ethernet"] {
            assert!(!virtual_name(n), "{n} should be real");
        }
    }
}
