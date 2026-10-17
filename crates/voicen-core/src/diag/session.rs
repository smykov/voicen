//! The session marker and the previous-session verdict (T-100; spec 006 FR-016,
//! research R7, data-model "SessionMarker", "InstallerNote").
//!
//! Invariants (T-100 analysis (1), (2)):
//! - [`Session::begin`] is the only reader of a previous `logs/session.marker` and
//!   the only writer of a new one and of an `abnormal_end` crash file, so each
//!   abnormal end gets exactly one record: a crash file whose `session_started`
//!   equals the old marker's `started` gives [`PreviousSession::Crashed`] and no new
//!   file; else an `installer-ended` note whose modification time is not older than
//!   the old marker's gives [`PreviousSession::EndedByInstaller`]; anything else, a
//!   corrupt or unreadable marker included, gives [`PreviousSession::EndedAbnormally`]
//!   with one `abnormal_end` file. Then the old marker and the note are gone and the
//!   new marker exists (written as `session.marker.tmp`, then renamed).
//! - only [`Session::end_clean`] deletes the marker (not `Drop`).
//! - no marker byte is copied anywhere: its `pid` and `started` are parsed into a
//!   `u32` and an instant, and diag writes them back; a value that does not parse
//!   is left out.
//!
//! Not on the crash path: `begin` and `end_clean` write their lines with
//! `Log::write`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::crash::{
    crash_files, remove_stale_tmp, write_crash_file, CrashDetail, CrashFields, CrashFileName,
    LocalStamp, OsVersion,
};
use super::event::{LogEvent, PreviousSessionLine};
use super::format::{commit_text, parse_timestamp, version_text};
use super::log::Log;
use crate::build_info::BuildInfo;
use crate::clock::{Clock, LocalOffset};

const MARKER: &str = "session.marker";
const MARKER_TMP: &str = "session.marker.tmp";
const NOTE: &str = "installer-ended";
/// The most bytes read from the marker or from one crash file.
const MAX_READ: u64 = 64 * 1024;

/// What [`Session::begin`] needs.
pub struct SessionStart<'a> {
    pub logs_dir: &'a Path,
    pub build: BuildInfo,
    pub os: OsVersion,
    pub pid: u32,
    pub clock: &'a dyn Clock,
    pub offset: &'a dyn LocalOffset,
}

/// How the previous session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousSession {
    /// No marker: it ended cleanly (or there was none). No log line.
    Clean,
    /// A crash file of that session exists; no new record.
    Crashed { crash_file: CrashFileName },
    /// The installer ended it; no record.
    EndedByInstaller,
    /// It ended without a clean exit or a crash file; the `abnormal_end` file
    /// written for it, `None` when it could not be written.
    EndedAbnormally { crash_file: Option<CrashFileName> },
}

impl PreviousSession {
    fn line(self) -> Option<PreviousSessionLine> {
        match self {
            PreviousSession::Clean => None,
            PreviousSession::Crashed { crash_file } => {
                Some(PreviousSessionLine::Crashed { crash_file })
            }
            PreviousSession::EndedByInstaller => Some(PreviousSessionLine::EndedByInstaller),
            PreviousSession::EndedAbnormally { crash_file } => {
                Some(PreviousSessionLine::EndedAbnormally { crash_file })
            }
        }
    }
}

/// This process's session: its marker exists until [`end_clean`](Self::end_clean).
#[derive(Debug)]
pub struct Session {
    marker: PathBuf,
    started: SystemTime,
}

/// What a previous marker gave: its values that parse, and its modification time.
struct OldMarker {
    pid: Option<u32>,
    started: Option<LocalStamp>,
    modified: Option<SystemTime>,
}

impl Session {
    /// Classifies the previous session (module docs), writes an `abnormal_end`
    /// crash file if needed, deletes the old marker, the installer note and stale
    /// `crash-*.tmp` files, writes the new marker and one `previous session` line
    /// (none for [`PreviousSession::Clean`]). Never fails or panics: a step that
    /// cannot be done is skipped. Call only in the primary instance, once.
    pub fn begin(start: SessionStart<'_>, log: &Log) -> (Session, PreviousSession) {
        let SessionStart {
            logs_dir,
            build,
            os,
            pid,
            clock,
            offset,
        } = start;
        let now = clock.now();
        let stamp = LocalStamp::of(now, offset);
        let marker = logs_dir.join(MARKER);
        let note = logs_dir.join(NOTE);
        let _ = fs::create_dir_all(logs_dir);
        remove_stale_tmp(logs_dir);

        let verdict = match read_marker(&marker) {
            None => PreviousSession::Clean,
            Some(old) => classify(logs_dir, &note, &old).unwrap_or_else(|| {
                let fields = CrashFields {
                    time: stamp,
                    build: build.clone(),
                    os,
                    session_started: old.started,
                    detail: CrashDetail::AbnormalEnd {
                        previous_session_pid: old.pid,
                    },
                };
                PreviousSession::EndedAbnormally {
                    crash_file: write_crash_file(logs_dir, pid, &fields),
                }
            }),
        };

        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(&note);
        write_marker(logs_dir, &marker, pid, stamp, &build);
        if let Some(line) = verdict.line() {
            log.write(LogEvent::PreviousSession(line));
        }
        (
            Session {
                marker,
                started: now,
            },
            verdict,
        )
    }

    /// When this session started (the clock's now at `begin`).
    pub fn started(&self) -> SystemTime {
        self.started
    }

    /// Deletes the marker and writes one `exited` line: the only clean end.
    pub fn end_clean(self, log: &Log) {
        let _ = fs::remove_file(&self.marker);
        log.write(LogEvent::Exited);
    }
}

/// `Crashed` or `EndedByInstaller` when the evidence says so, else `None`.
fn classify(logs_dir: &Path, note: &Path, old: &OldMarker) -> Option<PreviousSession> {
    if let Some(started) = old.started {
        if let Some(crash_file) = crash_file_of(logs_dir, &started.text()) {
            return Some(PreviousSession::Crashed { crash_file });
        }
    }
    let note_modified = fs::symlink_metadata(note).ok()?.modified().ok()?;
    (note_modified >= old.modified?).then_some(PreviousSession::EndedByInstaller)
}

/// The first (by name) crash file diag wrote whose `session_started` line is
/// `started`. The files are only read, never copied.
fn crash_file_of(logs_dir: &Path, started: &str) -> Option<CrashFileName> {
    let wanted = format!("session_started: {started}");
    let mut files = crash_files(logs_dir);
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files.into_iter().find_map(|(path, name, _)| {
        let name = CrashFileName::parse(&name)?;
        let text = read_bounded(&path)?;
        String::from_utf8_lossy(&text)
            .lines()
            .any(|line| line == wanted)
            .then_some(name)
    })
}

/// `None` when there is no marker; else the values of it that parse.
fn read_marker(path: &Path) -> Option<OldMarker> {
    let meta = fs::symlink_metadata(path).ok()?;
    let mut old = OldMarker {
        pid: None,
        started: None,
        modified: meta.modified().ok(),
    };
    let Some(bytes) = read_bounded(path) else {
        return Some(old);
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Some(old);
    };
    for line in text.split('\n') {
        match line.split_once('=') {
            Some(("pid", value)) if old.pid.is_none() => old.pid = parse_pid(value),
            Some(("started", value)) if old.started.is_none() => {
                old.started = parse_timestamp(value)
                    .map(|(at, seconds_east)| LocalStamp { at, seconds_east });
            }
            _ => {}
        }
    }
    Some(old)
}

/// Decimal digits only, as a `u32`.
fn parse_pid(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// At most [`MAX_READ`] bytes of a regular file.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut out = Vec::new();
    file.take(MAX_READ).read_to_end(&mut out).ok()?;
    Some(out)
}

/// `pid=<n>\nstarted=<ts>\nversion=<v>\ncommit=<c>\n`, through a temp file and a
/// rename; nothing is left when either step fails.
fn write_marker(logs_dir: &Path, marker: &Path, pid: u32, started: LocalStamp, build: &BuildInfo) {
    let text = format!(
        "pid={pid}\nstarted={}\nversion={}\ncommit={}\n",
        started.text(),
        version_text(build.version),
        commit_text(build.commit),
    );
    let tmp = logs_dir.join(MARKER_TMP);
    if fs::write(&tmp, text).is_err() || fs::rename(&tmp, marker).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}
