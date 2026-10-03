//! Platform-independent core of Voicen.

pub mod build_info;
pub mod hotkey_registrar;
pub mod i18n;
pub mod models;
pub mod post_process;
pub mod secrets;
pub mod settings;

pub use build_info::{build_info, BuildInfo};
