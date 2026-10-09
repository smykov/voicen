//! Platform-independent core of Voicen.

pub mod audio;
pub mod autostart;
pub mod build_info;
pub mod clock;
pub mod connection_test;
pub mod delivery;
pub mod diag;
pub mod dictation;
pub mod engine;
pub mod events;
pub mod failure;
pub mod hotkey_registrar;
pub mod i18n;
pub mod local_models;
pub mod microphone;
pub mod models;
pub mod overlay;
pub mod pipeline;
pub mod platform;
pub mod post_process;
pub mod recording;
pub mod secrets;
pub mod settings;
#[cfg(any(test, feature = "test-fakes"))]
pub mod test_support;
pub mod timeouts;
pub mod tray;
pub mod vad;
pub mod win32_data;

pub use build_info::{build_info, BuildInfo};
