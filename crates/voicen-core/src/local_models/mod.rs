//! Built-in whisper.cpp models on disk: the catalog, the store that derives
//! "downloaded" from the disk, and the one downloader (spec 002 US1, T-016,
//! decision #49).
//!
//! Invariant (T-016 analysis): a file appears under a catalog model's final name
//! only through the [`download::Downloader`], which renames `<file>.part` after the
//! received byte count equals the catalog size and the streamed SHA-256 equals the
//! pinned value. Every other end of a download removes `<file>.part` before the end
//! event. [`store::ModelStore`] is the one `DownloadedModels` the settings
//! validation, the IPC list and the engine read.

pub mod catalog;
pub mod download;
pub mod store;
