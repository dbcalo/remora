//! The Linux and Steam Deck window: GTK 4 (decided 2026-11-07), NOT YET BUILT.
//!
//! GTK can only be built and tested on Linux, so it is written on the Fedora
//! laptop, with the gtk4 crate (gtk4-rs, approved) added to Cargo.toml then. It
//! will paint `view::View` and send `view::Action`s through the `Controller`,
//! exactly as `win32.rs` does. Until then this says so instead of pretending.

use std::sync::Arc;

use crate::controller::Controller;

pub fn run(_ctrl: Arc<Controller>) {
    error_window("The Linux window (GTK 4) is not built yet; see reference.md.");
}

pub fn error_window(message: &str) {
    eprintln!("remora: {message}");
}
