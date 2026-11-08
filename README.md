# Remora

Remora is a manager for portable Jellyfin servers: a program that runs a
portable Jellyfin server on whatever machine is travelling — a Windows laptop, a
Linux laptop, a Steam Deck — to serve a TV on someone else's network. Each
install stands alone, with its own library, users and key.

It began with the frustration of trying to stream films while visiting family:
the laptop had the films, the TV had a Jellyfin app, and everything in between —
starting the server, the firewall, the laptop going to sleep mid-film, finding
the address to type in — was a fight. Remora is a portable install of Jellyfin
that is easy to manage, with no extra hardware.

One window does everything:

- the server address to type into the TV, large, with Copy and Open buttons
- what is playing, per device
- **keeps the machine awake while something is playing**, by itself or through
  Caffeine Party when installed; a pause of more than 30 minutes lets it sleep
- **stops the server by itself once unused**: each time something is loaded the
  stop moves to the end of it plus 30 minutes; with nothing played it stops 30
  minutes after starting; the window being open and on screen keeps it running
- network safety before starting: firewall rules for **Private networks only** on
  Windows (one UAC prompt, the first time), the current zone on Fedora if it is not
  `public` (asked once per zone), and a plain statement when there is no firewall
- first-time setup of a fresh Jellyfin, or a one-time admin password to make this
  machine's API key; the password is never stored
- **Add to Steam**, as a button, on a machine with Steam
- a **Library** page: add and remove libraries and their folders (the files
  are never touched), and scan them all with progress; **Restart** on the
  Status page
- a **Users** page: set or remove any user's password, the administrator's too,
  without the old one. Jellyfin then signs that user out everywhere, so the TV
  signs in again; an administrator always keeps a password
- on a Windows laptop with Modern Standby, **keeps Wi-Fi connected in standby
  on battery while the server runs**, so the screen can turn off mid-film; the
  previous setting comes back when the server stops

Closing the window stops the server.

Remora is not made by or affiliated with the Jellyfin project. Its name stays
clear of "Jellyfin", as Jellyfin's branding rules ask of third-party programs. Starting the program again while it runs
brings the window back instead of starting a second server.

## Install

One folder holds everything; the program is the only thing at its top:

```
Jellyfin\
  Remora.exe                the program: double-click this
  jellyfin\                 the Jellyfin release, unzipped as it comes
  data\  cache\  logs\      Jellyfin's state, created on first start
```

1. Unzip the Jellyfin release into `jellyfin\`. On Windows that is the release zip
   (it carries `jellyfin.exe` and `ffmpeg.exe`). On Linux it is the generic
   `jellyfin_<v>-amd64.tar.xz`, with the portable `jellyfin-ffmpeg` tarball
   unpacked beside it as `jellyfin-ffmpeg/`.
2. Build the program and copy it to the top of the folder:

   ```powershell
   cargo build --release; Copy-Item target\release\remora.exe 'C:\Jellyfin\Remora.exe'
   ```

   ```bash
   cargo build --release && cp target/release/remora ~/Jellyfin/remora
   ```

**To upgrade Jellyfin,** quit the program, delete `jellyfin\` and unzip the new
release in its place. `data\` stays where it is: it is the library, the users and
the key, and Jellyfin's database stores paths into it, so it must not be moved.

Build and copy the program again after every pull. What each OS needs to build,
including the Deck, is in `reference.md`.

## Settings

Optional, in `remora.json` beside the program:

```json
{
  "jellyfin_dir": "D:\\Jellyfin",
  "idle_stop": { "start_minutes": 30, "margin_minutes": 30 }
}
```

`jellyfin_dir` is the top folder (the release is in its `jellyfin\`), and
defaults to the program's own folder. `"idle_stop": {"off": true}`
turns the auto-stop off. The program adds `approved_zones` itself.

## Steam Deck

In Desktop Mode, start it and press **Add to Steam**. In Game Mode, launch it from
the library: it detects Game Mode and fills the screen. Exiting the game stops the
server.

## Keep-awake

Remora holds the machine awake itself while something plays, and lets it go 30
minutes after playback stops or pauses; it never keeps the screen on. With
Caffeine Party 2.1 or later installed (`~/.caffeine`), it holds through Caffeine
instead: Caffeine's launcher row then says *Remora is holding the machine
awake*, and stopping that row pauses only Remora's hold for an hour. Closing a
laptop's lid sleeps it regardless.

## Tests

```bash
cargo test
```

The window is native to each OS: Win32 on Windows; GTK 4 on Fedora and Qt 6 on
the Steam Deck (`cargo build --release --features qt`), both still to be built on
those machines.
