# Remora — invariants

Remora, a native manager for a portable Jellyfin (named in `decide::APP_NAME` and
`decide::APP_ID`; "Jellyfin" never in its name, per Jellyfin's branding rules): one Rust program, with the window
native to each OS (Win32 on Windows, GTK 4 on Fedora, Qt 6 on the Steam Deck with
`--features qt`; the two Linux ones are not built yet). Every window only paints
`view::View` and sends `view::Action` through the `Controller`. Decisions are
pure functions in `src/decide.rs` over an `Os` value; only `src/sys/` and
`src/ui/mod.rs` may use a platform `cfg` (a test enforces it). One suite,
`cargo test`, nothing ignored, on Windows and Linux.

**The crate set is small on purpose, and each one is approved**: ureq, serde,
serde_json, windows-sys, getrandom; and, for the Linux windows when they are
built, gtk4 (gtk4-rs) and cxx-qt. Nothing else, direct or by swapping one out,
without the maintainer's approval. `Cargo.lock` is committed.

Depth — the Jellyfin 12.4 API as observed, why the idle clock works as it does,
what each OS needs to build, what is still unverified and where — is in
`reference.md` beside this file. Read it before touching setup, the API key,
Steam or the firewall.

## Each of these fails silently

- **Strip Windows' `\\?\` prefix from the program's own path** (`plain_path` in
  `main.rs`). `canonicalize` adds it, every path is built from that one, and
  Jellyfin dies at startup on a `\\?\` `--datadir` with nothing on screen but
  "exited".
- **Never write Steam's `shortcuts.vdf`.** The running client rewrites it on exit
  and discards an outside edit. It is read, as bytes, to see whether the launcher
  is already added; adding goes through Steam, and only on the button.
- **Never store the admin password,** and never use a fixed or shared API key.
  Every Jellyfin key is admin-level. The key is random per install and is the
  only secret kept: Credential Manager on Windows, a 0600 file on Linux.
- **Reach Caffeine through its command line only** (`caffeine hold --holder
  remora`, per Caffeine Party's `docs/contract.md`). Writing its files
  directly holds nothing on Windows. Without Caffeine, the built-in hold
  (`platform::awake`) is used; on Windows its request is per thread, so one
  thread owns it (`sys::execution_hold`).
- **The firewall is Private-only and never widened.** Windows rules are
  `profile=private` for `jellyfin.exe` alone; firewalld opens only a non-`public`
  zone. Never offer a VPN or virtual adapter as the TV's network: filter by
  `decide::virtual_name`, not by a vendor word (WireGuard's alias is `wg<digits>`).
- **On Windows, the battery value of "Networking connectivity in Standby" is
  switched on only while Jellyfin runs** (`src/standby.rs`). Its old value is
  written to disk before the switch; `standby::recover` runs only after the
  one-instance check, since while another launcher runs that value is live.
- **Modal dialogs run outside the UI borrow** (`Do::Confirm`, `Do::AddFolder`,
  `Do::Browse` in `ui/win32.rs`): a message box or the folder picker pumps
  messages, and a timer tick then finds the UI borrowed and skips painting.
- **The elevated re-entry takes integers only** (interface indexes); it runs as
  administrator.
- **Every command runs through `sys::no_window`.** The program has no console
  (`windows_subsystem`), so a bare `Command` for netsh, PowerShell or Jellyfin
  flashes a console window, or leaves one open.

## Tests must never reach the real machine

No test starts Jellyfin, runs netsh, PowerShell elevation, `firewall-cmd` or
Steam. The Credential Manager test uses a target unique to the run and deletes
it.
