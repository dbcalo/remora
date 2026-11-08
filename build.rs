//! Compiles the Windows icon into the .exe, so Explorer, the taskbar and Steam
//! show it. No crate: it runs the Windows SDK's own resource compiler (rc.exe)
//! and hands the .res to the linker. Elsewhere it does nothing; the window icon
//! itself is set by Slint on every OS.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico = crate_dir.join("assets").join("icon.ico");
    let rc_file = out.join("app.rc");
    let res = out.join("app.res");
    // The manifest turns on visual styles (Common Controls 6, so buttons, tabs
    // and group boxes look like Windows 10/11, not Windows 95) and per-monitor
    // DPI awareness (sharp text at 150% scaling).
    let manifest = out.join("app.manifest");
    std::fs::write(&manifest, MANIFEST).unwrap();
    // rc.exe reads backslash escapes in strings, so the paths' are doubled.
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc_file, format!("1 ICON \"{}\"\n1 24 \"{}\"\n", esc(&ico), esc(&manifest))).unwrap();

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found: the .exe gets no icon (the window still has one)");
        return;
    };
    let ok = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc_file)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning=rc.exe failed: the .exe gets no icon (the window still has one)");
    }
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
                        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

/// rc.exe from $RC, else the newest Windows 10/11 SDK's x64 tools.
fn find_rc() -> Option<PathBuf> {
    if let Ok(rc) = env::var("RC") {
        return Some(PathBuf::from(rc));
    }
    let base = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("x64").join("rc.exe").is_file())
        .collect();
    versions.sort();
    versions.pop().map(|v| v.join("x64").join("rc.exe"))
}
