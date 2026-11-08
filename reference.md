# Remora — reference

Depth for `CLAUDE.md`: what Jellyfin was observed to do, why the program works as
it does, and what is still unverified. Remora was called "Jellyfin Portable"
until 2026-11-08; older notes say "the launcher" and mean the same program.

## The Jellyfin API, as checked against a real server

Checked 2026-11-07 and 2026-11-08 against **Jellyfin 12.4.0**, run on a scratch
data folder bound to 127.0.0.1:8097 and deleted afterwards (so no real install was
touched): by hand, then through the program's own client. The fake server in
`src/jellyfin.rs`'s tests mimics exactly this; change the two together.

| Call | What 12.4.0 did |
|---|---|
| `GET /health` | **200 with the body `Degraded`** while starting (other calls 503), then 200 `Healthy`. Ready means the body, not the status |
| `GET /System/Info/Public` | No auth. `StartupWizardCompleted` says whether first-run setup is needed |
| `GET`/`POST /Startup/Configuration` | 200 / 204, no auth, until the wizard completes |
| `GET /Startup/User` | **Not a read: it creates the first user** (named after the OS account). `POST /Startup/User {Name, Password}` is a **404 until that GET has run**, then 204 |
| `POST /Library/VirtualFolders?name=&collectionType=mixed` with `LibraryOptions.PathInfos` | 204 with no auth **during** the wizard, so the library goes in before Complete |
| `POST /Startup/RemoteAccess`, `POST /Startup/Complete` | 204; after Complete every `/Startup/*` call is 401 |
| `POST /Users/AuthenticateByName {Username, Pw}` | 200 with `AccessToken`; a wrong password is 401 |
| `POST /Auth/Keys?app=<name>` | **204, no body.** A second call with the same app name makes a **second** key |
| `GET /Auth/Keys` | `{Items: [{AccessToken, AppName, DateCreated, ...}]}`. Because names repeat, the new key is the one absent from the list taken just before creating it |
| `POST /Sessions/Logout` | 204; the session token is 401 from then on |
| `GET /Sessions?activeWithinSeconds=960` with `Authorization: MediaBrowser ..., Token="<key>"` | 200 |
| the same with **`X-Emby-Token: <key>`** | **401.** Only the `Authorization` header works for an API key on 12.4 |
| `GET /Users` with the key | 200: `[{Id, Name, HasPassword, Policy: {IsAdministrator}, ...}]` |
| `GET /Users` after an empty-password user | **`HasPassword` and `HasConfiguredPassword` are both `true` even for a user created with `""`**, so the list cannot tell who has no password; the Users page does not try |
| `POST /Users/New {Name, Password}` with the key | 200 with the new user, **never an administrator**. A name that differs only in case from an existing one is **400** |
| `POST /Users/{id}/Password {NewPw}` with the key | **204 with no current password**, for the admin itself too. The old password is then 401. **Every session of that user is signed out** (the admin's own session went 401); the API key keeps working. `{NewPw: ""}` and `{ResetPassword: true}` both leave no password; no token is 401 |
| `GET /Items/Latest?userId=<admin id>&limit=8` with the key | 200: `[{Name, ProductionYear, Type, ...}]`, newest first. **Without `userId` it is 400** |
| `GET /Library/VirtualFolders` with the key | 200: `[{Name, Locations: [paths], CollectionType, LibraryOptions, ...}]` |
| `POST /Library/VirtualFolders?name=&collectionType=&refreshLibrary=` with the key, after setup | 204. **A second library of the same name is accepted and silently named "Name2"**, so the program refuses a duplicate itself |
| `POST /Library/VirtualFolders/Paths {Name, PathInfo: {Path}}` | 204; **a folder that does not exist is 404** "Error processing request." |
| `DELETE /Library/VirtualFolders/Paths?name=&path=` | 204. Name and path percent-encoded |
| `DELETE /Library/VirtualFolders?name=` | 204; an unknown name is 404. **The files stay on disk** (checked) |
| `POST /Library/Refresh` with the key | 204; starts "Scan Media Library". No token is 401 |
| `GET /ScheduledTasks?isHidden=false` with the key | 200; the scan is the task with `Key: "RefreshLibrary"`: `State` "Running" with `CurrentProgressPercentage` (0, 48, 97...), then "Idle"; `LastExecutionResult.Status` "Completed" |

A second Jellyfin on a port already held exits at startup (Kestrel bind
failure), so the program checks port 8096 first and says what holds it.

## The folder

```
<top>\                  jellyfin_dir; the program sits here
  jellyfin\             the release, unzipped as it comes (Linux: the tarball, with
                        jellyfin-ffmpeg\ beside it)
  data\ cache\ logs\     passed as --datadir, --cachedir, --logdir
```

The program is the only thing at the top, and an upgrade replaces `jellyfin\`
whole. **`data\` must never move:** Jellyfin's database holds hundreds of absolute
paths into `<top>\data\` (510 in one real install). Its one other stored path,
`EncoderAppPathDisplay` in `encoding.xml`, is a display copy Jellyfin rewrites from
`--ffmpeg` on start.

## Keep-awake and the idle clock

**Playing means an item loaded and not paused.** Only that renews the hold, every 5
minutes; a hold lasts 30, so a paused film lets the machine sleep 30 minutes later
and a late poll never lets it sleep mid-film. Without Caffeine Party the program
holds the machine itself (`platform::awake`); with it, through Caffeine's command
line, so Caffeine's launcher row shows and can pause the hold.

**The idle auto-stop is a rolling clock on what is loaded.** It starts with 30
minutes for something to begin playing. Each poll pushes the stop to *time left in
any loaded item + margin*, paused items included: the server stays up for a paused
film so the TV keeps its place, while the machine may still sleep. It never moves
earlier. The window being visible (on screen, not minimised) pushes it by the
margin. A missing or nonsense runtime counts as zero. The window shows it as time
left ("in 1 h 30 min"): the standard library cannot format local clock time, and no
crate was added for it.

### Modern Standby: the hold does not stop standby; the network setting does

**On a Modern Standby laptop (S0 low power idle) the screen timing out puts the
machine into standby, whatever is held.** The hold is `ES_SYSTEM_REQUIRED` without
the display, on purpose: the screen should not stay on through a film. Observed
2026-11-07 on battery: Kernel-Power 506 "entering Modern Standby", reason 12 (idle
timeout), with a hold active; Jellyfin kept serving in standby for an hour, then
Kernel-Power 172 "Connectivity state in standby: Disconnected, Reason: Adaptive
Connected Standby" and the film stopped.

**What stops it is the power plan's "Networking connectivity in Standby"
(`CONNECTIVITYINSTANDBY`).** On AC it is usually Enable; on battery "Managed by
Windows", which disconnects when it chooses. `src/standby.rs` sets the battery
value to Enable while Jellyfin runs and puts the old value back when it stops, so
real sleep still drops the Wi-Fi. powercfg writes the user's own plan without
elevation. The value to restore is written before the switch, so a crash leaves it
on disk and the next launch restores it. `SCHEME_CURRENT` is used both ways:
switching plans mid-film restores into the new plan.

## Firewall

| System | What happens |
|---|---|
| Windows | Two rules, `Jellyfin HTTP (Private)` and `Jellyfin Discovery (Private)`, for this `jellyfin.exe` only. Missing → added on first run through one UAC prompt (an elevated re-entry of the program). A Public network that is not a virtual adapter → the window offers to mark it Private |
| Fedora, firewalld | The zone of the default-route interface. `public` → nothing opened, said in the window. Another zone → asked the first time, then opened automatically (`approved_zones`), through `pkexec firewall-cmd`, runtime and permanent |
| ufw | Reported, never edited |
| None (SteamOS default) | The window says anyone on the network can reach Jellyfin |

## Choices forced by a small crate set

- **Interfaces come from the OS**, not a crate: `Get-NetIPAddress` on Windows and
  `ip -4 -o addr` on Linux (present on Fedora and SteamOS), parsed by tested
  functions. The default route is the local address of a UDP "connect" to
  TEST-NET-1, which sends nothing.
- **Stopping Jellyfin on Linux** sends SIGTERM through the `kill` command and waits
  15 s; there is no libc binding. For the same reason Jellyfin is **not** told to
  die with the program: a normal quit and Steam's Exit Game both stop it, but a
  `kill -9` of the program leaves Jellyfin running.
- **The Windows window is Win32 through `windows-sys`**, in the machine's light or
  dark theme (`AppsUseLightTheme`), switching live on `WM_SETTINGCHANGE`
  "ImmersiveColorSet". Win32 has no supported dark mode, so: the documented dark
  title bar (`DwmSetWindowAttribute`), Windows' own `DarkMode_Explorer` /
  `DarkMode_CFD` control themes (undocumented names, widely used), and painted
  colours for the rest. **Group boxes fill their inside white under the dark
  theme**, so sections are bold headings without frames; checkbox text stays
  black, so the checkbox has a separate label (clicking it toggles); the tab
  control has no dark theme, so pages are switched by an owner-drawn selector bar
  with an accent underline. `build.rs` compiles a manifest in for visual styles
  (Common Controls 6) and per-monitor DPI. `JELLYFIN_PORTABLE_THEME=light` (or
  `dark`) forces a theme, to check one without changing the machine's setting.
- **Page switches left stale, light pixels until the window was minimised**, and
  three things fix it together: each page is a container window, so a switch
  shows one and hides one; labels paint opaque; and everything painting needs
  (colours, brushes, the selected page) lives in a `Cell` snapshot, because
  Windows asks for backgrounds and colours *during* a switch, while the UI state
  is borrowed, and a failed borrow fell back to Windows' light defaults.
  **Check a window from the real screen** (always-on-top, then copy the screen):
  `PrintWindow` makes the window repaint itself fully, which hides exactly these
  bugs, and passed a build that was visibly broken.

## The icon

A carry case with a play triangle, in a purple-to-blue gradient that echoes
Jellyfin's colours without its logo. `assets/make_icon.py` (Pillow, build-time
only) writes `assets/icon.png` and `assets/icon.ico`; never edit either by hand,
the next run overwrites them. The PNG is the window icon on every OS, compiled in
by the Linux windows. The ICO goes into the Windows .exe (and so the window)
through `build.rs`, which runs the SDK's `rc.exe` (no crate) so Explorer, the
taskbar and Steam show it; without `rc.exe` the build still succeeds and warns.

## Building per OS

| OS | Needs |
|---|---|
| Windows | rustup (stable, MSVC) and the Visual Studio C++ build tools with a Windows SDK |
| Fedora | rustup, and GTK 4's development package (`gtk4-devel`) from dnf, for the GTK window. **Not yet built there** |
| Steam Deck | **Open.** The read-only system cannot take development packages; build inside a distrobox (SteamOS ships it) or on another Linux with a glibc no newer than the Deck's |

## Known issues

- **A WireGuard tunnel can still be offered as the TV's address.** Names miss
  VPNs: OpenVPN's TAP adapter can be called "Local Area Connection", while the
  descriptions say "TAP-Windows Adapter", "OpenVPN Data Channel Offload" and
  "WireGuard Tunnel". Likely fix: judge an adapter by its description as well as
  its name. To diagnose, with the tunnel on: `Get-NetAdapter`,
  `Get-NetIPAddress -AddressFamily IPv4` and `Get-NetConnectionProfile`.

## Unverified, and where it can be verified

| Claim | Where |
|---|---|
| The window on Linux (Wayland/X11) and in Game Mode, full-screen, driven by Steam Input | a Fedora laptop; a Steam Deck |
| `steam://addnonsteamgame/<path>` adds silently on Windows | a Windows machine with Steam |
| `steamos-add-to-steam` adds without a restart | a Steam Deck, Desktop Mode |
| Steam's idle suspend honours a logind inhibitor in Game Mode | a Steam Deck; if not, the window must say keep-awake is not guaranteed there |
| firewalld zone, pkexec prompt | a Fedora laptop |
| A whole film plays through Modern Standby on battery with the network setting switched on | a Modern Standby laptop, on battery, screen left to time out |
| The built-in Linux hold (`systemd-inhibit`) | a Fedora laptop |
