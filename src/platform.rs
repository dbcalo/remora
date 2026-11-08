//! What the launcher observes and changes on the machine: interfaces, the
//! firewall, Steam, the Jellyfin process. It branches on `decide::Os` at run
//! time and runs commands; choices are made in `decide`.

use std::io;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::decide::{self, Firewall, Iface, Net, Os};
use crate::sys;

/// Runs argv quietly (no window) and returns its exit code (None if it could not
/// start) and stdout.
pub fn run(argv: &[&str]) -> (Option<i32>, String) {
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stderr(Stdio::null());
    match sys::no_window(&mut cmd).output() {
        Ok(out) => (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned()),
        Err(_) => (None, String::new()),
    }
}

fn powershell(script: &str) -> (Option<i32>, String) {
    run(&["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script])
}

// --- interfaces ---------------------------------------------------------------------------

/// The local address the OS would send from to an outside address: a UDP
/// "connect" picks a route and sends nothing. That is the default-route address,
/// the same way on every OS.
fn default_route_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET-1: never a real host
    Some(s.local_addr().ok()?.ip().to_string())
}

const WINDOWS_ADDRS: &str =
    "Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { \"$($_.InterfaceAlias)`t$($_.IPAddress)\" }";

/// `Get-NetIPAddress` lines: "alias<TAB>address".
fn parse_windows_addrs(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.trim_end().split_once('\t'))
        .map(|(a, ip)| (a.to_string(), ip.to_string()))
        .collect()
}

/// `ip -4 -o addr show` lines: "2: wlp2s0    inet 192.168.1.5/24 brd ...".
fn parse_ip_addr(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let i = f.iter().position(|w| *w == "inet")?;
            Some((f.get(1)?.to_string(), f.get(i + 1)?.split('/').next()?.to_string()))
        })
        .collect()
}

/// Builds interfaces from (name, address) pairs, marking the default route.
fn ifaces_from(pairs: Vec<(String, String)>, route_ip: Option<&str>) -> Vec<Iface> {
    let mut out: Vec<Iface> = vec![];
    for (name, ip) in pairs {
        if ip.starts_with("169.254.") {
            continue; // link-local: never what a TV reaches
        }
        let idx = match out.iter().position(|f| f.name == name) {
            Some(i) => i,
            None => {
                out.push(Iface {
                    loopback: name == "lo" || name.starts_with("Loopback") || ip.starts_with("127."),
                    virtual_: decide::virtual_name(&name),
                    name: name.clone(),
                    ..Default::default()
                });
                out.len() - 1
            }
        };
        if Some(ip.as_str()) == route_ip {
            out[idx].default_route = true;
        }
        out[idx].ipv4.push(ip);
    }
    out
}

/// Every interface with an IPv4 address, the default route marked.
pub fn ifaces(os: Os) -> Vec<Iface> {
    let pairs = match os {
        Os::Windows => parse_windows_addrs(&powershell(WINDOWS_ADDRS).1),
        Os::Linux => parse_ip_addr(&run(&["ip", "-4", "-o", "addr", "show"]).1),
    };
    ifaces_from(pairs, default_route_ip().as_deref())
}

// --- firewall -------------------------------------------------------------------------------

/// A Windows network connection, by interface index and name.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub index: u32,
    pub name: String,
}

/// Connected Public networks as "index<TAB>alias<TAB>name". Which adapters to
/// skip is decided in Rust, by the same name test the TV address uses, not by a
/// pattern here: WireGuard's alias is "wg<digits>", not "WireGuard".
const PUBLIC_PROFILES: &str = "Get-NetConnectionProfile | Where-Object { $_.NetworkCategory -eq 'Public' } | \
     ForEach-Object { \"$($_.InterfaceIndex)`t$($_.InterfaceAlias)`t$($_.Name)\" }";

/// Public networks a TV could be on: VPN, container, hypervisor and loopback
/// adapters never are, so offering to mark one Private would be wrong.
fn parse_profiles(out: &str) -> Vec<Profile> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.trim_end().splitn(3, '\t');
            let index: u32 = f.next()?.parse().ok().filter(|&i| i > 0)?;
            let alias = f.next()?;
            let name = f.next()?;
            let skip = decide::virtual_name(alias) || alias.starts_with("Loopback");
            (!skip).then(|| Profile { index, name: name.to_string() })
        })
        .collect()
}

fn windows_rules_present() -> bool {
    decide::WINDOWS_RULE_NAMES
        .iter()
        .all(|name| run(&["netsh", "advfirewall", "firewall", "show", "rule", &format!("name={name}")]).0 == Some(0))
}

/// Re-enters the binary as the elevated firewall step.
pub const ELEVATED_FLAG: &str = "--elevated-firewall";

/// The elevated re-entry's argv: the flag and the interface indexes to mark
/// Private, as integers only. It runs as administrator, so nothing a network
/// name contains may reach it; it re-derives jellyfin.exe's path itself.
pub fn elevated_args(private: &[Profile]) -> Vec<String> {
    let mut args = vec![ELEVATED_FLAG.to_string()];
    if !private.is_empty() {
        let ids: Vec<String> = private.iter().map(|p| p.index.to_string()).collect();
        args.push(format!("--private={}", ids.join(",")));
    }
    args
}

/// Reads elevated_args back (after the flag). Anything but positive integers is
/// refused.
pub fn parse_elevated_args(args: &[String]) -> Option<Vec<u32>> {
    let mut ids = vec![];
    for a in args {
        for s in a.strip_prefix("--private=")?.split(',') {
            ids.push(s.parse::<u32>().ok().filter(|&n| n > 0)?);
        }
    }
    Some(ids)
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn run_as_script(exe: &str, args: &[String]) -> String {
    let quoted: Vec<String> = args.iter().map(|a| ps_quote(a)).collect();
    format!("Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait", ps_quote(exe), quoted.join(","))
}

/// Runs the elevated step through one UAC prompt.
pub fn elevate(private: &[Profile]) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    match powershell(&run_as_script(&exe.to_string_lossy(), &elevated_args(private))).0 {
        Some(0) => Ok(()),
        _ => Err(io::Error::other("the administrator prompt was cancelled; the TV may not be able to connect")),
    }
}

/// The elevated step itself: replace the two rules (Private profile only, for
/// this jellyfin.exe) and mark the given interfaces Private.
pub fn run_elevated(jellyfin_exe: &str, private: &[u32]) -> io::Result<()> {
    let mut errs = vec![];
    for name in decide::WINDOWS_RULE_NAMES {
        run(&["netsh", "advfirewall", "firewall", "delete", "rule", &format!("name={name}")]);
    }
    for argv in decide::windows_rule_argvs(jellyfin_exe) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        if run(&argv).0 != Some(0) {
            errs.push(argv[5].to_string());
        }
    }
    for idx in private {
        if powershell(&format!("Set-NetConnectionProfile -InterfaceIndex {idx} -NetworkCategory Private")).0 != Some(0) {
            errs.push(format!("interface {idx}"));
        }
    }
    if errs.is_empty() { Ok(()) } else { Err(io::Error::other(format!("failed: {}", errs.join(", ")))) }
}

fn pick_zone(of_interface: &str, default_zone: &str) -> String {
    let z = of_interface.trim();
    if z.is_empty() { default_zone.trim().to_string() } else { z.to_string() }
}

/// Opens Jellyfin's ports in `zone` now and permanently, through pkexec: no
/// password is ever seen or kept here.
pub fn open_zone(zone: &str) -> io::Result<()> {
    if zone.is_empty() || zone == "public" {
        return Err(io::Error::other("the public zone is never opened"));
    }
    for permanent in [false, true] {
        let argv = decide::firewalld_open_argv(zone, permanent);
        let mut full = vec!["pkexec"];
        full.extend(argv.iter().map(String::as_str));
        if run(&full).0 != Some(0) {
            return Err(io::Error::other("firewall-cmd failed or the password prompt was cancelled"));
        }
    }
    Ok(())
}

/// Observes the firewall and network for `decide::firewall_plan`. `approved`
/// says whether a firewalld zone was agreed to before. The profiles are the
/// Windows Public networks, for the elevated step.
pub fn probe(os: Os, approved: impl Fn(&str) -> bool) -> (Net, Vec<Profile>) {
    let mut n = Net::new(os);
    if os == Os::Windows {
        n.rules_present = windows_rules_present();
        let profiles = parse_profiles(&powershell(PUBLIC_PROFILES).1);
        n.public_networks = profiles.iter().map(|p| p.name.clone()).collect();
        return (n, profiles);
    }
    if run(&["firewall-cmd", "--state"]).0 == Some(0) {
        n.firewall = Firewall::Firewalld;
        let iface = ifaces(os).into_iter().find(|f| f.default_route).map(|f| f.name).unwrap_or_default();
        let of = if iface.is_empty() { String::new() } else { run(&["firewall-cmd", &format!("--get-zone-of-interface={iface}")]).1 };
        n.zone = pick_zone(&of, &run(&["firewall-cmd", "--get-default-zone"]).1);
        n.ports_open = !n.zone.is_empty()
            && [format!("{}/tcp", decide::HTTP_PORT), format!("{}/udp", decide::DISCOVERY_PORT)]
                .iter()
                .all(|p| run(&["firewall-cmd", &format!("--zone={}", n.zone), &format!("--query-port={p}")]).0 == Some(0));
        n.zone_approved = approved(&n.zone);
    } else if run(&["systemctl", "is-active", "--quiet", "ufw"]).0 == Some(0) {
        n.firewall = Firewall::Ufw;
    }
    (n, vec![])
}

// --- Steam ------------------------------------------------------------------------------------

fn parse_reg_sz(out: &str, name: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 3 && f[0] == name && f[1] == "REG_SZ").then(|| l.split_once("REG_SZ").unwrap().1.trim().to_string())
    })
}

/// Steam's own folder, or None when Steam is not installed.
pub fn steam_dir(os: Os, home: &Path) -> Option<PathBuf> {
    if os == Os::Windows {
        let out = run(&["reg", "query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"]).1;
        return parse_reg_sz(&out, "SteamPath").map(PathBuf::from).filter(|p| p.is_dir());
    }
    [
        home.join(".steam").join("steam"),
        home.join(".local").join("share").join("Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .find(|p| p.join("userdata").is_dir())
}

/// Every Steam user's shortcuts.vdf, READ ONLY: the running client rewrites it
/// on exit and discards an outside edit, so it is never written.
pub fn read_shortcuts(steam: &Path) -> Vec<Vec<u8>> {
    let Ok(users) = std::fs::read_dir(steam.join("userdata")) else { return vec![] };
    users
        .flatten()
        .filter_map(|u| std::fs::read(u.path().join("config").join("shortcuts.vdf")).ok())
        .collect()
}

/// Asks the running Steam to add the launcher: SteamOS's own helper when
/// present, else the steam:// URL that helper uses. Only ever on a click.
pub fn add_to_steam(os: Os, exe: &str) -> io::Result<()> {
    if os == Os::Linux && run(&["sh", "-c", "command -v steamos-add-to-steam"]).0 == Some(0) {
        let argv = decide::add_to_steam_argv(exe);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        return match run(&argv).0 {
            Some(0) => Ok(()),
            _ => Err(io::Error::other("steamos-add-to-steam failed")),
        };
    }
    open_url(os, &decide::add_to_steam_url(exe))
}

/// Opens a URL in the default handler.
pub fn open_url(os: Os, url: &str) -> io::Result<()> {
    let argv = decide::open_url_argv(os, url);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    sys::no_window(&mut cmd).spawn().map(drop)
}

/// The built-in hold (used when Caffeine is not installed): each call holds the
/// machine for `d` from now. Windows: a thread owning a system-required
/// request, dropped by itself when `d` runs out. Linux: a logind inhibitor
/// that ends by itself after `d`; each renewal starts a new one and ends the
/// last. Neither ever holds the display.
pub fn awake(os: Os) -> crate::keepawake::Hold {
    match os {
        Os::Windows => sys::execution_hold(),
        Os::Linux => {
            let mut child: Option<Child> = None;
            Box::new(move |d: Duration| {
                if let Some(mut c) = child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                let argv = decide::linux_inhibit_argv(d.as_secs());
                let mut cmd = Command::new(&argv[0]);
                cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                child = sys::no_window(&mut cmd).spawn().ok();
                // Held only if it spawned and is still running a moment later.
                child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
            })
        }
    }
}

/// Opens `dir` in the file manager, creating it first if Jellyfin has not yet.
pub fn open_folder(os: Os, dir: &Path) -> io::Result<()> {
    open_folder_with(os, dir, &mut |argv: &[String]| {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        sys::no_window(&mut cmd).spawn().map(drop)
    })
}

fn open_folder_with(os: Os, dir: &Path, launch: &mut dyn FnMut(&[String]) -> io::Result<()>) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    launch(&decide::open_folder_argv(os, dir))
}

// --- the Jellyfin process ------------------------------------------------------------------------

/// The running server.
pub struct Jellyfin {
    child: Child,
    os: Os,
}

impl Jellyfin {
    /// Starts argv in `dir`, with no console window and no streams.
    pub fn start(os: Os, argv: &[PathBuf], dir: &Path) -> io::Result<Jellyfin> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        Ok(Jellyfin { child: sys::no_window(&mut cmd).spawn()?, os })
    }

    /// Whether it has exited on its own.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Stops it and waits: SIGTERM and up to 15 s to shut down cleanly on Linux
    /// (through `kill`, so no libc binding is needed); ended at once on Windows,
    /// which has no such signal.
    pub fn stop(&mut self) {
        if self.exited() {
            return;
        }
        if self.os == Os::Linux {
            run(&["kill", "-TERM", &self.child.id().to_string()]);
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if self.exited() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Jellyfin {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether nothing else holds Jellyfin's port. Another Jellyfin there makes this
/// one die at startup with nothing to show but "exited".
pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_folder_creates_a_missing_folder() {
        let d = std::env::temp_dir().join(format!("remora-logs-{}", std::process::id())).join("logs");
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
        let mut seen = vec![];
        open_folder_with(Os::Windows, &d, &mut |argv: &[String]| {
            seen = argv.to_vec();
            Ok(())
        })
        .unwrap();
        assert!(d.is_dir(), "the folder was not created");
        assert_eq!(seen, decide::open_folder_argv(Os::Windows, &d));
    }

    #[test]
    fn windows_addresses_are_parsed() {
        let out = "Wi-Fi\t192.168.1.162\r\nLoopback Pseudo-Interface 1\t127.0.0.1\r\nEthernet\t169.254.3.4\r\n";
        let got = ifaces_from(parse_windows_addrs(out), Some("192.168.1.162"));
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].default_route && !got[0].loopback);
        assert!(got[1].loopback);
        assert_eq!(decide::tv_addresses(&got, 8096), ["http://192.168.1.162:8096"]);
    }

    #[test]
    fn linux_addresses_are_parsed() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
                   3: wlp2s0    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic wlp2s0\\  valid_lft 1\n\
                   5: wg0    inet 10.5.5.3/24 scope global wg0\\  valid_lft forever\n";
        let got = ifaces_from(parse_ip_addr(out), Some("10.5.5.3"));
        assert_eq!(got.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["lo", "wlp2s0", "wg0"]);
        assert!(got[2].virtual_ && got[2].default_route);
        // A full-tunnel VPN carries the default route: fall back to the real one.
        assert_eq!(decide::tv_addresses(&got, 8096), ["http://192.168.1.20:8096"]);
    }

    #[test]
    fn public_profiles_are_parsed_and_virtual_adapters_skipped() {
        // "wg1234567890123" is the form of a WireGuard tunnel's alias. It was
        // offered as the TV's network because the old filter looked for the word
        // WireGuard; every adapter now goes through decide::virtual_name instead.
        let out = "12\tWi-Fi\tHotel Wi-Fi\r\n31\twg1234567890123\twg1234567890123\r\n\
                   9\tvEthernet (WSL)\tUnidentified network\r\n7\tEthernet 2\tOffice\r\n\r\ngarbage\nx\ty\tz\n";
        assert_eq!(
            parse_profiles(out),
            [Profile { index: 12, name: "Hotel Wi-Fi".into() }, Profile { index: 7, name: "Office".into() }]
        );
    }

    #[test]
    fn elevated_args_carry_only_indexes() {
        let p = [Profile { index: 12, name: "x'; rm".into() }, Profile { index: 7, name: String::new() }];
        assert_eq!(elevated_args(&p), [ELEVATED_FLAG, "--private=12,7"]);
        assert_eq!(elevated_args(&[]), [ELEVATED_FLAG]);
        assert_eq!(parse_elevated_args(&["--private=12,7".into()]), Some(vec![12, 7]));
        assert_eq!(parse_elevated_args(&["--private=12;whoami".into()]), None);
        assert_eq!(parse_elevated_args(&["--other".into()]), None);
        assert_eq!(parse_elevated_args(&[]), Some(vec![]));
    }

    #[test]
    fn run_as_script_quotes_the_path() {
        let s = run_as_script(r"C:\Users\O'Brien\remora.exe", &[ELEVATED_FLAG.into(), "--private=3".into()]);
        assert!(s.contains(r"-FilePath 'C:\Users\O''Brien\remora.exe'"), "{s}");
        assert!(s.contains("-ArgumentList '--elevated-firewall','--private=3'") && s.contains("-Verb RunAs") && s.contains("-Wait"), "{s}");
    }

    #[test]
    fn firewalld_zone_falls_back_to_the_default() {
        assert_eq!(pick_zone("", "public\n"), "public");
        assert_eq!(pick_zone("home\n", "public\n"), "home");
    }

    #[test]
    fn steam_path_from_the_registry_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    c:/program files (x86)/steam\r\n\r\n";
        assert_eq!(parse_reg_sz(out, "SteamPath").as_deref(), Some("c:/program files (x86)/steam"));
        assert_eq!(parse_reg_sz("ERROR: not found", "SteamPath"), None);
    }

    #[test]
    fn shortcut_files_are_found_per_steam_user() {
        let steam = std::env::temp_dir().join(format!("jp-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&steam);
        for user in ["111", "222"] {
            let d = steam.join("userdata").join(user).join("config");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("shortcuts.vdf"), user).unwrap();
        }
        assert_eq!(read_shortcuts(&steam).len(), 2);
        std::fs::remove_dir_all(&steam).unwrap();
    }

    #[test]
    fn port_free_says_when_something_holds_the_port() {
        let l = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_free(port));
        drop(l);
        assert!(port_free(port));
    }
}
