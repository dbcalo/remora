# Remora

Remora is an application that manages portable Jellyfin installs on Windows. Linux and
Steam Deck support is in progress.

I needed an easy way to manage a portable jellyfin install while visiting my parents, and remora is the result. Remora helps you setup a portable jellyfin server, and has the following features:

- shows you the server address with Copy and Open buttons
- shows what's playing, per device
- **keeps the machine awake while something is playing**
- **stops the server by itself once unused**
- sets up firewall rules for **Private networks**
- helps set up a fresh copy of Jellyfin
- includes an **Add to Steam** button
- includes a **Library** management page
- includes a **User** management page

Closing the window stops the server.

Remora is not made by or affiliated with the Jellyfin project.

## Install

One folder includes all program files for the portable install.

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

   On Linux (work in progress: not yet built or tested there):

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

## Steam Deck (work in progress)

Not working yet: the Steam Deck window is still to be built. When it is:

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
