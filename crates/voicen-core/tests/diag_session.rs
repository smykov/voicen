//! T-100 (spec 006 T022, research R7; FR-20, NFR-07): `diag::Session` on a temp
//! dir with a fake clock and a fixed +02:00 offset.
//!
//! `Session::begin` classifies the previous session from `logs/session.marker`,
//! the crash files and `logs/installer-ended`, writes at most one new record (one
//! `abnormal_end` crash file and one `previous session` line), deletes the old
//! marker and the note, and writes the new marker. `Session::end_clean` deletes the
//! marker and writes one `exited` line.
//!
//! The API these tests pin (the developer builds it; contracts/core-diag.md
//! "Session" is updated with it):
//!
//! ```text
//! voicen_core::diag::{Session, SessionStart, PreviousSession, CrashFileName, OsVersion}
//! pub struct SessionStart<'a> { logs_dir: &'a Path, build: BuildInfo, os: OsVersion, pid: u32,
//!                               clock: &'a dyn Clock, offset: &'a dyn LocalOffset }   // pub fields
//! Session::begin(start: SessionStart<'_>, log: &Log) -> (Session, PreviousSession)
//! Session::started(&self) -> SystemTime;  Session::end_clean(self, log: &Log)
//! PreviousSession { Clean, Crashed { crash_file: CrashFileName }, EndedByInstaller,
//!                   EndedAbnormally { crash_file: Option<CrashFileName> } }  // Debug, PartialEq
//! CrashFileName: Display = the file name (`crash-YYYYMMDD-HHMMSS-<pid>.txt`)
//! OsVersion { major: u32, minor: u32, build: u32 }   // rendered major.minor.build
//! marker   = "pid=<n>\nstarted=<ts>\nversion=<v>\ncommit=<c>\n"
//! log line = "previous session outcome=<crashed|ended_by_installer|ended_abnormally> [crash_file=<name>]"
//!            (WARN, except ended_by_installer INFO; Clean writes no line), "exited" (INFO)
//! ```
//!
//! The installer note is judged by its mtime against the old marker's mtime (T-100
//! Investigation): not older -> `EndedByInstaller`. Mtimes are planted with
//! `set_modified`, so nothing here sleeps or reads the wall clock.
//!
//! Portable fixtures (these tests also run on the Windows CI job): every path is
//! built with `Path::join`.

mod diag_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use diag_support::{
    all_lines, at, closed, contains, files_under, lines_of, open_log, Line, Offset, NOON_UTC,
};
use voicen_core::build_info::BuildInfo;
use voicen_core::clock::FakeClock;
use voicen_core::diag::{Log, LogConfig, OsVersion, PreviousSession, Session, SessionStart};
use voicen_core::test_support::TempDir;

/// UTC+02:00: the local text differs from UTC, so an offset dropped or applied
/// twice is visible.
const EAST: i32 = 7_200;
/// The new process.
const PID: u32 = 4_242;
/// The previous session's process.
const OLD_PID: u32 = 4_100;
/// The previous session started 26 h before now: 2026-10-03T10:00:00Z.
const OLD_SECS: u64 = NOON_UTC - 26 * 3_600;
/// Its `started` as the marker holds it (local text at +02:00).
const OLD_STARTED: &str = "2026-10-03T12:00:00.000+02:00";
/// Now as local text (2026-10-04T12:00:00Z at +02:00).
const NOW_LOCAL: &str = "2026-10-04T14:00:00.000+02:00";
const MARKER: &str = "session.marker";
const NOTE: &str = "installer-ended";
/// Planted into corrupt markers and foreign crash files: must never be copied
/// into a crash file or a log line.
const CANARY: &str = "sk-test-PLANTED-MARKER";

fn build() -> BuildInfo {
    BuildInfo {
        version: "0.9.1",
        commit: "abc1234",
    }
}

fn os() -> OsVersion {
    OsVersion {
        major: 10,
        minor: 0,
        build: 22_631,
    }
}

fn now() -> SystemTime {
    at(NOON_UTC, 0)
}

/// Writes `bytes` to `path` with modification time `t`.
fn plant(path: &Path, bytes: &[u8], t: SystemTime) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .unwrap_or_else(|e| panic!("set_modified {}: {e}", path.display()));
}

/// The marker the previous session wrote, last modified when it started.
fn plant_old_marker(dir: &Path) {
    let text = format!("pid={OLD_PID}\nstarted={OLD_STARTED}\nversion=0.9.0\ncommit=fedcba9\n");
    plant(&dir.join(MARKER), text.as_bytes(), at(OLD_SECS, 0));
}

/// A crash file of the session that started at `session_started`.
fn plant_crash_file(dir: &Path, name: &str, session_started: &str) -> Vec<u8> {
    let text = format!(
        "kind: panic\ntime: 2026-10-03T13:00:00.000+02:00\nversion: 0.9.0\ncommit: fedcba9\n\
         os: 10.0.22631\nsession_started: {session_started}\nthread: main\n\
         location: src{sep}main.rs:1:1\nbacktrace:\n  voicen.exe+0x10\n",
        sep = std::path::MAIN_SEPARATOR
    );
    plant(&dir.join(name), text.as_bytes(), at(OLD_SECS + 3_600, 0));
    text.into_bytes()
}

/// The marker this process must have written.
fn new_marker() -> String {
    format!("pid={PID}\nstarted={NOW_LOCAL}\nversion=0.9.1\ncommit=abc1234\n")
}

/// `crash-*.txt` names in `dir`, sorted.
fn crash_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

struct Run {
    _tmp: TempDir,
    dir: PathBuf,
    log: std::sync::Arc<Log>,
    clock: FakeClock,
}

fn setup() -> Run {
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    let (log, _clock, _seen) = open_log(&dir, now(), EAST, LogConfig::default());
    Run {
        _tmp: tmp,
        dir,
        log,
        clock: FakeClock::at(now()),
    }
}

impl Run {
    fn begin(&self) -> (Session, PreviousSession) {
        Session::begin(
            SessionStart {
                logs_dir: &self.dir,
                build: build(),
                os: os(),
                pid: PID,
                clock: &self.clock,
                offset: &Offset(EAST),
            },
            &self.log,
        )
    }

    /// Every log line, each checked against the closed grammar.
    fn lines(&self) -> Vec<Line> {
        all_lines(&self.dir).iter().map(|l| closed(l)).collect()
    }

    fn previous_lines(&self) -> Vec<Line> {
        self.lines()
            .into_iter()
            .filter(|l| l.head == "previous session")
            .collect()
    }
}

/// What holds after every `begin`: the new marker exists with this process's
/// values (written whole: no temp file left), the old note is gone, and at most one
/// `previous session` line was written.
#[track_caller]
fn assert_after_begin(run: &Run) {
    let marker =
        std::fs::read(run.dir.join(MARKER)).unwrap_or_else(|e| panic!("no new marker: {e}"));
    assert_eq!(
        String::from_utf8_lossy(&marker),
        new_marker(),
        "the new marker holds pid, started, version and commit of this session"
    );
    assert!(
        !run.dir.join(NOTE).exists(),
        "the installer note is consumed"
    );
    let leftovers: Vec<PathBuf> = files_under(&run.dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    assert!(
        run.previous_lines().len() <= 1,
        "at most one previous-session line: {:?}",
        run.previous_lines()
    );
}

/// Asserts the verdict is `EndedAbnormally` with one new `abnormal_end` file and
/// returns that file's lines.
#[track_caller]
fn assert_one_abnormal_end(run: &Run, verdict: &PreviousSession, before: &[String]) -> Vec<String> {
    let PreviousSession::EndedAbnormally {
        crash_file: Some(name),
    } = verdict
    else {
        panic!("expected EndedAbnormally with a crash file, got {verdict:?}");
    };
    let name = name.to_string();
    let after = crash_files(&run.dir);
    let new: Vec<&String> = after.iter().filter(|n| !before.contains(n)).collect();
    assert_eq!(
        new,
        vec![&name],
        "exactly one new crash file, the one the verdict names"
    );
    assert!(
        name.starts_with("crash-20261004-140000-"),
        "named by the local time of this start: {name}"
    );
    let previous = run.previous_lines();
    assert_eq!(previous.len(), 1, "one previous-session line");
    let line = previous.first().unwrap_or_else(|| unreachable!());
    assert_eq!(line.level, "WARN");
    assert_eq!(line.get("outcome"), Some("ended_abnormally"));
    assert_eq!(
        line.get("crash_file"),
        Some(name.as_str()),
        "the line names only the file"
    );
    lines_of(&run.dir.join(&name))
}

/// The abnormal_end file of a readable old marker, line by line.
fn abnormal_end_with_marker() -> Vec<String> {
    [
        "kind: abnormal_end".to_string(),
        format!("time: {NOW_LOCAL}"),
        "version: 0.9.1".to_string(),
        "commit: abc1234".to_string(),
        "os: 10.0.22631".to_string(),
        format!("session_started: {OLD_STARTED}"),
        format!("previous_session_pid: {OLD_PID}"),
    ]
    .to_vec()
}

/// The data-model keys of an abnormal_end file, in order.
const ABNORMAL_END_KEYS: &[&str] = &[
    "kind",
    "time",
    "version",
    "commit",
    "os",
    "session_started",
    "previous_session_pid",
];

// ---- R7 case 1: no marker ------------------------------------------------------------

#[test]
fn no_marker_is_clean_writes_the_marker_and_no_record() {
    let run = setup();
    let (session, verdict) = run.begin();
    assert_eq!(verdict, PreviousSession::Clean);
    assert_eq!(session.started(), now(), "started is the clock's now");
    assert!(
        crash_files(&run.dir).is_empty(),
        "no crash file after a clean session"
    );
    assert!(
        run.previous_lines().is_empty(),
        "a clean previous session writes no line"
    );
    assert_after_begin(&run);
}

#[test]
fn a_stray_installer_note_without_a_marker_is_clean_and_consumed() {
    // R7: the note is written even when the app was not running; harmless, deleted.
    let run = setup();
    plant(
        &run.dir.join(NOTE),
        b"2026-10-04T13:00:00+02:00\n",
        at(NOON_UTC - 60, 0),
    );
    let (_session, verdict) = run.begin();
    assert_eq!(verdict, PreviousSession::Clean);
    assert!(crash_files(&run.dir).is_empty());
    assert_after_begin(&run);
}

// ---- R7 case 2: a crash file of the previous session -----------------------------------

#[test]
fn a_crash_file_of_the_previous_session_gives_crashed_and_no_new_record() {
    let run = setup();
    plant_old_marker(&run.dir);
    let name = "crash-20261003-130000-4100.txt";
    let bytes = plant_crash_file(&run.dir, name, OLD_STARTED);
    let (_session, verdict) = run.begin();
    let PreviousSession::Crashed { crash_file } = &verdict else {
        panic!("expected Crashed, got {verdict:?}");
    };
    assert_eq!(
        crash_file.to_string(),
        name,
        "the verdict names the existing file"
    );
    assert_eq!(
        crash_files(&run.dir),
        vec![name.to_string()],
        "no second record"
    );
    assert_eq!(
        std::fs::read(run.dir.join(name)).ok(),
        Some(bytes),
        "the existing crash file is left as it was"
    );
    let previous = run.previous_lines();
    assert_eq!(previous.len(), 1);
    let line = previous.first().unwrap_or_else(|| unreachable!());
    assert_eq!(line.level, "WARN");
    assert_eq!(line.get("outcome"), Some("crashed"));
    assert_eq!(line.get("crash_file"), Some(name));
    assert_after_begin(&run);
}

#[test]
fn a_crash_file_of_another_session_does_not_suppress_the_record() {
    // Bites "any crash file means crashed": the link is session_started.
    let run = setup();
    plant_old_marker(&run.dir);
    let other = "crash-20260920-090000-3000.txt";
    plant_crash_file(&run.dir, other, "2026-09-20T10:00:00.000+02:00");
    let before = crash_files(&run.dir);
    let (_session, verdict) = run.begin();
    let lines = assert_one_abnormal_end(&run, &verdict, &before);
    assert_eq!(lines, abnormal_end_with_marker());
    assert!(
        run.dir.join(other).exists(),
        "the other session's file is kept"
    );
    assert_after_begin(&run);
}

#[test]
fn a_matching_crash_file_wins_over_an_installer_note() {
    // R7 order: case 2 before case 3.
    let run = setup();
    plant_old_marker(&run.dir);
    let name = "crash-20261003-130000-4100.txt";
    plant_crash_file(&run.dir, name, OLD_STARTED);
    plant(&run.dir.join(NOTE), b"note\n", at(OLD_SECS + 7_200, 0));
    let (_session, verdict) = run.begin();
    assert!(
        matches!(&verdict, PreviousSession::Crashed { crash_file } if crash_file.to_string() == name),
        "expected Crashed({name}), got {verdict:?}"
    );
    assert_eq!(crash_files(&run.dir), vec![name.to_string()]);
    assert_after_begin(&run);
}

// ---- R7 case 3: ended by the installer -------------------------------------------------

#[test]
fn an_installer_note_newer_than_the_marker_gives_ended_by_installer_without_a_record() {
    let run = setup();
    plant_old_marker(&run.dir);
    plant(
        &run.dir.join(NOTE),
        b"2026-10-03T18:00:00+02:00\n",
        at(OLD_SECS + 6 * 3_600, 0),
    );
    let (_session, verdict) = run.begin();
    assert_eq!(verdict, PreviousSession::EndedByInstaller);
    assert!(crash_files(&run.dir).is_empty(), "no crash record");
    let previous = run.previous_lines();
    assert_eq!(previous.len(), 1);
    let line = previous.first().unwrap_or_else(|| unreachable!());
    assert_eq!(line.level, "INFO");
    assert_eq!(line.get("outcome"), Some("ended_by_installer"));
    assert_eq!(line.get("crash_file"), None);
    assert_after_begin(&run);
}

#[test]
fn an_installer_note_as_old_as_the_marker_gives_ended_by_installer() {
    // "not older than": equal mtimes count for the installer.
    let run = setup();
    plant_old_marker(&run.dir);
    plant(&run.dir.join(NOTE), b"note\n", at(OLD_SECS, 0));
    let (_session, verdict) = run.begin();
    assert_eq!(verdict, PreviousSession::EndedByInstaller);
    assert!(crash_files(&run.dir).is_empty());
    assert_after_begin(&run);
}

#[test]
fn an_installer_note_older_than_the_marker_gives_one_abnormal_end_record() {
    // A note left over from an install before the previous session started.
    let run = setup();
    plant_old_marker(&run.dir);
    plant(&run.dir.join(NOTE), b"note\n", at(OLD_SECS - 60, 0));
    let (_session, verdict) = run.begin();
    let lines = assert_one_abnormal_end(&run, &verdict, &[]);
    assert_eq!(lines, abnormal_end_with_marker());
    assert_after_begin(&run);
}

// ---- R7 case 4: ended abnormally ---------------------------------------------------------

#[test]
fn a_marker_alone_gives_one_abnormal_end_record_linked_to_the_old_session() {
    let run = setup();
    plant_old_marker(&run.dir);
    let (_session, verdict) = run.begin();
    let lines = assert_one_abnormal_end(&run, &verdict, &[]);
    assert_eq!(
        lines,
        abnormal_end_with_marker(),
        "kind, time, current build, os, the old session's started and pid, in data-model order"
    );
    assert_after_begin(&run);
}

#[test]
fn a_corrupt_marker_gives_ended_abnormally_without_panicking_or_copying_its_bytes() {
    let garbage =
        format!("\u{1}\u{0}not a marker started={CANARY}\npid=12x\r\nkind: native_fault\n");
    let cases: [(&str, Vec<u8>); 5] = [
        ("garbage", garbage.into_bytes()),
        ("empty", Vec::new()),
        (
            "non-UTF-8",
            [0xFF, 0xFE, 0xFD, b'\n', 0xC3, 0x28, 0x80].to_vec(),
        ),
        // Well-formed keys, values that are not what they claim: nothing may be copied.
        (
            "forged values",
            format!("pid={CANARY}\nstarted={CANARY}\nversion={CANARY}\ncommit={CANARY}\n")
                .into_bytes(),
        ),
        // A valid pid and a forged `started`: the pid may be kept, the text may not.
        (
            "forged started",
            format!("pid={OLD_PID}\nstarted={CANARY}\nversion=0.9.0\ncommit=fedcba9\n")
                .into_bytes(),
        ),
    ];
    for (what, bytes) in cases {
        let run = setup();
        plant(&run.dir.join(MARKER), &bytes, at(OLD_SECS, 0));
        let (_session, verdict) = run.begin();
        let lines = assert_one_abnormal_end(&run, &verdict, &[]);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("kind: abnormal_end"),
            "{what}: the record is an abnormal_end file"
        );
        // Only data-model keys, in data-model order (session_started and
        // previous_session_pid may be left out when the marker does not give them).
        let keys: Vec<&str> = lines
            .iter()
            .map(|l| l.split_once(": ").map_or(l.as_str(), |(k, _)| k))
            .collect();
        let mut allowed = ABNORMAL_END_KEYS.iter();
        for key in &keys {
            assert!(
                allowed.any(|a| a == key),
                "{what}: key {key:?} is not an abnormal_end key or is out of order: {lines:?}"
            );
        }
        assert_eq!(
            keys.iter().filter(|k| **k == "kind").count(),
            1,
            "{what}: one kind line"
        );
        for path in files_under(&run.dir) {
            if path.file_name().is_some_and(|n| n == MARKER) {
                continue;
            }
            let got = std::fs::read(&path).unwrap_or_default();
            assert!(
                !contains(&got, CANARY.as_bytes()),
                "{what}: marker bytes copied into {}",
                path.display()
            );
            assert!(
                !contains(&got, b"not a marker") && !contains(&got, &[0xFF, 0xFE]),
                "{what}: marker bytes copied into {}",
                path.display()
            );
        }
        assert_after_begin(&run);
    }
}

// ---- clean end ------------------------------------------------------------------------------

#[test]
fn end_clean_deletes_the_marker_and_writes_one_exited_line() {
    let run = setup();
    let (session, verdict) = run.begin();
    assert_eq!(verdict, PreviousSession::Clean);
    assert!(run.dir.join(MARKER).exists());
    session.end_clean(&run.log);
    assert!(
        !run.dir.join(MARKER).exists(),
        "end_clean deletes the marker"
    );
    let exited: Vec<Line> = run
        .lines()
        .into_iter()
        .filter(|l| l.head == "exited")
        .collect();
    assert_eq!(exited.len(), 1, "one exited line");
    let line = exited.first().unwrap_or_else(|| unreachable!());
    assert_eq!(line.level, "INFO");
    assert!(
        line.pairs.is_empty(),
        "exited carries no field: {:?}",
        line.raw
    );
    assert!(crash_files(&run.dir).is_empty());
}

#[test]
fn after_a_clean_end_the_next_start_is_clean() {
    let run = setup();
    let (session, _) = run.begin();
    session.end_clean(&run.log);
    let (_next, verdict) = run.begin();
    assert_eq!(
        verdict,
        PreviousSession::Clean,
        "only end_clean ends a session cleanly"
    );
    assert!(crash_files(&run.dir).is_empty());
}

#[test]
fn a_session_dropped_without_end_clean_is_ended_abnormally_at_the_next_start() {
    // Invariant (2): only end_clean deletes the marker (not Drop, not begin's caller).
    let run = setup();
    let (session, _) = run.begin();
    drop(session);
    assert!(
        run.dir.join(MARKER).exists(),
        "dropping a session keeps the marker"
    );
    run.clock.set(now() + Duration::from_secs(3_600));
    let (_next, verdict) = Session::begin(
        SessionStart {
            logs_dir: &run.dir,
            build: build(),
            os: os(),
            pid: PID + 1,
            clock: &run.clock,
            offset: &Offset(EAST),
        },
        &run.log,
    );
    let PreviousSession::EndedAbnormally {
        crash_file: Some(name),
    } = &verdict
    else {
        panic!("expected EndedAbnormally with a file, got {verdict:?}");
    };
    let lines = lines_of(&run.dir.join(name.to_string()));
    assert!(
        lines.contains(&format!("session_started: {NOW_LOCAL}")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!("previous_session_pid: {PID}")),
        "{lines:?}"
    );
    assert_eq!(crash_files(&run.dir).len(), 1);
}
