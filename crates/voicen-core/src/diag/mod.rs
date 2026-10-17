//! The local diagnostics log (FR-20, NFR-04, P-009; T-008; spec 006 US2).
//!
//! Invariant (T-008 analysis): every byte under `logs/` comes from the one
//! [`Log`] (one per primary-instance process, whole lines under one mutex) via
//! [`Log::write`] of a typed [`LogEvent`], and [`format_line`] is total over a
//! closed output: each value is an integer, a literal from this module's own
//! tables, or [`BuildInfo`](crate::BuildInfo). No `LogEvent` field is a `String`,
//! `&str`, path, `io::Error` or `FailureReason`; an outside `&'static str` (engine
//! kind, speech detector, failure code) passes through a table here
//! ([`EngineTag::from_kind`], [`DetectorTag::from_name`], [`FailureTag::from_code`]),
//! where an unknown value becomes `other`.
//!
//! T-100 adds the two other writers under `logs/`, each over closed typed fields
//! too: [`Session`] (the session marker and the previous-session verdict, `session`)
//! and the crash files (`crash`: [`render_crash_file`], [`install_panic_hook`],
//! [`retain_crash_files`]). The panic hook never calls [`Log::write`].
//!
//! A transcript, a key or any other text cannot be handed to the log: this must
//! not compile.
//!
//! ```compile_fail,E0308
//! fn log_text(log: &voicen_core::diag::Log, transcript: &str) {
//!     log.write(transcript);
//! }
//! ```
//!
//! ```compile_fail,E0308
//! fn log_text(log: &voicen_core::diag::Log, transcript: String) {
//!     log.write(transcript);
//! }
//! ```
//!
//! ```compile_fail,E0277
//! fn log_text(log: &voicen_core::diag::Log, transcript: &str) {
//!     log.write(transcript.into());
//! }
//! ```
//!
//! An engine kind, a detector name or a failure code from an open source cannot be
//! put on a dictation line without the table (one field per block, so each block
//! fails for its own field only):
//!
//! ```compile_fail,E0308
//! use voicen_core::diag::{DictationLine, DictationOutcome};
//! fn line(kind: &'static str) -> DictationLine {
//!     DictationLine {
//!         recording: 1,
//!         engine: Some(kind),
//!         outcome: DictationOutcome::TooShort,
//!         detector: None,
//!         press_to_frame_ms: None,
//!         duration_ms: Some(120),
//!         stop_to_text_ms: None,
//!         text_to_paste_ms: None,
//!         post_processing: None,
//!         local: None,
//!     }
//! }
//! ```
//!
//! ```compile_fail,E0308
//! use voicen_core::diag::{DictationLine, DictationOutcome};
//! fn line(name: &'static str) -> DictationLine {
//!     DictationLine {
//!         recording: 1,
//!         engine: None,
//!         outcome: DictationOutcome::NoSpeech,
//!         detector: Some(name),
//!         press_to_frame_ms: None,
//!         duration_ms: Some(120),
//!         stop_to_text_ms: None,
//!         text_to_paste_ms: None,
//!         post_processing: None,
//!         local: None,
//!     }
//! }
//! ```
//!
//! ```compile_fail,E0308
//! use voicen_core::diag::DictationOutcome;
//! fn outcome(code: &'static str) -> DictationOutcome {
//!     DictationOutcome::Failed { failure: code, http_status: None }
//! }
//! ```
//!
//! The same shapes compile through the tables, so each block above fails only
//! because of the string, not because of the harness:
//!
//! ```
//! use voicen_core::diag::{
//!     DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, Log, LogEvent,
//! };
//! fn log_line(log: &Log, kind: &'static str, name: &'static str, code: &'static str) {
//!     log.write(LogEvent::Dictation(DictationLine {
//!         recording: 1,
//!         engine: Some(EngineTag::from_kind(kind)),
//!         outcome: DictationOutcome::Failed {
//!             failure: FailureTag::from_code(code),
//!             http_status: None,
//!         },
//!         detector: Some(DetectorTag::from_name(name)),
//!         press_to_frame_ms: None,
//!         duration_ms: Some(120),
//!         stop_to_text_ms: None,
//!         text_to_paste_ms: None,
//!         post_processing: None,
//!         local: None,
//!     }));
//! }
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod crash;
pub mod event;
pub mod format;
pub mod log;
pub mod native_fault;
pub mod notice;
pub mod observer;
pub mod session;

pub use crash::{
    install_panic_hook, render_crash_file, retain_crash_files, CodeLocation, CrashContext,
    CrashDetail, CrashFields, CrashFileName, CrashKind, Frame, FrameSource, LocalStamp,
    ModuleResolver, OsVersion, ThreadName, CRASH_MAX_AGE_DAYS, CRASH_MAX_FILES,
};
pub use event::{
    DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, LoadKind, LogEvent,
    PreviousSessionLine, SaveLine, TestConnectionLine, WarningKind,
};
pub use format::format_line;
pub use log::{Log, LogConfig, LogsUnwritableReason, OnUnwritable};
pub use native_fault::{FaultFiles, NativeFaultSetup, NativeFaultWriter};
pub use notice::{LogsNotice, LogsNoticeSink};
pub use observer::LogObserver;
pub use session::{PreviousSession, Session, SessionStart};
