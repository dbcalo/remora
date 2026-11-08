//! The Steam Deck window: Qt 6, KDE Plasma's own toolkit (decided 2026-11-07:
//! "If steam deck uses something else, use that"), NOT YET BUILT.
//!
//! Qt can only be built where its development files are, which on the Deck's
//! read-only system means a distrobox; it is written there, with cxx-qt added to
//! Cargo.toml then, and built with `--features qt`. It will paint `view::View`
//! and send `view::Action`s through the `Controller`, as `win32.rs` does.

use std::sync::Arc;

use crate::controller::Controller;

pub fn run(_ctrl: Arc<Controller>) {
    error_window("The Steam Deck window (Qt) is not built yet; see reference.md.");
}

pub fn error_window(message: &str) {
    eprintln!("remora: {message}");
}
