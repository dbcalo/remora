//! The window, native to each OS (decided 2026-11-07): Win32 controls on Windows,
//! GTK 4 on Fedora (GNOME), Qt 6 on the Steam Deck (KDE Plasma), chosen with
//! `--features qt`. The two Linux ones are not built yet. All of them only paint
//! `view::View` and send `view::Action` through the `Controller`. This file and
//! `src/sys` are the only places allowed a platform `cfg`.

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::{error_window, run};

#[cfg(all(not(windows), not(feature = "qt")))]
mod gtk;
#[cfg(all(not(windows), not(feature = "qt")))]
pub use gtk::{error_window, run};

#[cfg(all(not(windows), feature = "qt"))]
mod qt;
#[cfg(all(not(windows), feature = "qt"))]
pub use qt::{error_window, run};
