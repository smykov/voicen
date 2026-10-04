//! Platform-independent core of Voicen.

pub mod audio;
pub mod autostart;
pub mod build_info;
pub mod clock;
pub mod engine;
pub mod failure;
pub mod hotkey_registrar;
pub mod i18n;
pub mod models;
pub mod post_process;
pub mod secrets;
pub mod settings;
#[cfg(any(test, feature = "test-fakes"))]
pub mod test_support;
pub mod timeouts;

pub use build_info::{build_info, BuildInfo};
