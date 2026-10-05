//! The Windows dictation adapters (T-006 invariants 1-4): they carry out and stamp,
//! and decide nothing. Each implements one core port (`voicen_core::platform`) or,
//! for the hotkey, calls the session's inputs; the Win32 numbers come from
//! `voicen_core::win32_data`. Windows CI proves them (`src-tauri/tests/{hotkey,
//! capture,clipboard,paste}.rs`).

pub mod capture;
pub mod clipboard;
pub mod hotkey;
pub mod paste;
