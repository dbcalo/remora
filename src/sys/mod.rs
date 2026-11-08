//! The only module allowed a platform `cfg`. Everything else compiles and is
//! tested identically on every OS, branching on `decide::Os` at run time; a test
//! (`only_sys_is_platform_specific`) enforces that.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub use other::*;

use crate::decide::Os;

/// The OS this binary runs on, as a value the rest of the program branches on.
pub fn os() -> Os {
    if cfg!(windows) { Os::Windows } else { Os::Linux }
}
