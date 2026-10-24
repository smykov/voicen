//! Shared helpers of the T-008 log tests (`tests/diag_*.rs`, `tests/settings_log.rs`):
//! the hand-written line grammar and closed value sets the log output is checked
//! against, a fixed [`LocalOffset`], and readers for the files under a logs dir.
//!
//! The grammar is the contract of `diag::format_line` (T-008 analysis "closed
//! output"):
//!
//! ```text
//! line     = ts SP level SP message           ; no CR or LF inside
//! ts       = YYYY "-" MM "-" DD "T" hh ":" mm ":" ss "." mmm ("+" / "-") hh ":" mm
//! level    = "INFO" / "WARN" / "ERROR"
//! message  = "voicen " version " (" commit ") started" *(SP pair)
//!          / head *(SP pair)
//! head     = word *(SP word)                  ; word = 1*(a-z / "_")
//! pair     = key "=" value                    ; key = 1*(a-z / "_")
//! ```
//!
//! and every (head, key) has a closed value set ([`check_closed`]): an integer, a
//! literal of the diag tables, or a list of settings field ids / codes. An
//! unknown head, an unknown key or a value outside its set is an error, so a new
//! key reaches the log only together with a change here (P-009).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use voicen_core::clock::{FakeClock, LocalOffset};
use voicen_core::diag::{Log, LogConfig, LogsUnwritableReason};

/// The active file's name: the Windows CI smoke reads `logs\voicen.log`.
pub const ACTIVE: &str = "voicen.log";

/// A constant offset from UTC.
pub struct Offset(pub i32);

impl LocalOffset for Offset {
    fn seconds_east(&self, _t: SystemTime) -> i32 {
        self.0
    }
}

pub fn at(secs: u64, millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis)
}

/// 2026-10-04T12:00:00Z.
pub const NOON_UTC: u64 = 1_791_115_200;

/// What a [`Log`]'s `on_unwritable` received.
#[derive(Default)]
pub struct Unwritable {
    calls: AtomicUsize,
    reasons: Mutex<Vec<LogsUnwritableReason>>,
}

impl Unwritable {
    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn reasons(&self) -> Vec<LogsUnwritableReason> {
        self.reasons
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A [`Log`] over `dir` with a fake clock and a fixed offset; returns the log, its
/// clock and the record of `on_unwritable` calls.
pub fn open_log(
    dir: &Path,
    start: SystemTime,
    seconds_east: i32,
    cfg: LogConfig,
) -> (Arc<Log>, Arc<FakeClock>, Arc<Unwritable>) {
    let clock = Arc::new(FakeClock::at(start));
    let seen = Arc::new(Unwritable::default());
    let sink = Arc::clone(&seen);
    let log = Log::open(
        dir.to_path_buf(),
        clock.clone(),
        Arc::new(Offset(seconds_east)),
        cfg,
        Box::new(move |reason| {
            sink.calls.fetch_add(1, Ordering::SeqCst);
            sink.reasons
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(reason);
        }),
    );
    (Arc::new(log), clock, seen)
}

// ---- files ---------------------------------------------------------------------

/// Every regular file under `dir`, recursively, sorted.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => out.push(path),
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

/// The names of the rolled files (`voicen-*.log`) in `dir`, sorted.
pub fn rolled_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("voicen-") && n.ends_with(".log"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Size of `voicen.log` (0 when absent).
pub fn active_len(dir: &Path) -> u64 {
    std::fs::metadata(dir.join(ACTIVE)).map_or(0, |m| m.len())
}

/// Total size of `voicen.log` plus every `voicen-*.log` in `dir`.
pub fn set_len(dir: &Path) -> u64 {
    active_len(dir)
        + rolled_names(dir)
            .iter()
            .map(|n| std::fs::metadata(dir.join(n)).map_or(0, |m| m.len()))
            .sum::<u64>()
}

/// The lines of `path`; panics on a missing file, on non-UTF-8, or on a last line
/// without its `\n` (a cut line).
#[track_caller]
pub fn lines_of(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let text =
        String::from_utf8(bytes).unwrap_or_else(|e| panic!("{} is not UTF-8: {e}", path.display()));
    if text.is_empty() {
        return Vec::new();
    }
    assert!(
        text.ends_with('\n'),
        "{}: the last line is cut (no terminating \\n): {:?}",
        path.display(),
        text.lines().last()
    );
    text.split_terminator('\n').map(str::to_string).collect()
}

/// The lines of `dir/voicen.log`.
#[track_caller]
pub fn active_lines(dir: &Path) -> Vec<String> {
    lines_of(&dir.join(ACTIVE))
}

/// The lines of every `voicen.log` / `voicen-*.log` in `dir`.
#[track_caller]
pub fn all_lines(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for name in rolled_names(dir) {
        out.extend(lines_of(&dir.join(name)));
    }
    if dir.join(ACTIVE).exists() {
        out.extend(active_lines(dir));
    }
    out
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

pub fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

// ---- grammar -------------------------------------------------------------------

/// One parsed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub raw: String,
    /// `YYYY-MM-DDTHH:MM:SS.mmm+HH:MM`.
    pub ts: String,
    pub level: String,
    /// `started` for the start line, else the head words (`settings save`).
    pub head: String,
    /// `<version> (<commit>)` of the start line.
    pub build: Option<String>,
    pub pairs: BTreeMap<String, String>,
}

impl Line {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.get(key).map(String::as_str)
    }

    pub fn keys(&self) -> Vec<&str> {
        self.pairs.keys().map(String::as_str).collect()
    }
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// (year, month, day, hour, minute, second, millisecond, offset seconds east).
pub type TsParts = (u32, u32, u32, u32, u32, u32, u32, i32);

/// Checks the timestamp grammar and ranges.
pub fn parse_ts(ts: &str) -> Result<TsParts, String> {
    let b = ts.as_bytes();
    if b.len() != 29 {
        return Err(format!("timestamp {ts:?} is not 29 bytes"));
    }
    let seps = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (26, b':'),
    ];
    for (i, c) in seps {
        if b.get(i) != Some(&c) {
            return Err(format!("timestamp {ts:?}: byte {i} is not {:?}", c as char));
        }
    }
    let sign = match b.get(23) {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(format!("timestamp {ts:?}: no offset sign")),
    };
    let f = |from: usize, n: usize| {
        ts.get(from..from + n)
            .and_then(|s| digits(s, n))
            .ok_or_else(|| format!("timestamp {ts:?}: bytes {from}..{} not digits", from + n))
    };
    let (y, mo, d, h, mi, s, ms, oh, om) = (
        f(0, 4)?,
        f(5, 2)?,
        f(8, 2)?,
        f(11, 2)?,
        f(14, 2)?,
        f(17, 2)?,
        f(20, 3)?,
        f(24, 2)?,
        f(27, 2)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return Err(format!("timestamp {ts:?}: field out of range"));
    }
    if oh > 14 || om > 59 {
        return Err(format!("timestamp {ts:?}: offset out of range"));
    }
    let offset = sign * i32::try_from(oh * 3600 + om * 60).map_err(|e| e.to_string())?;
    Ok((y, mo, d, h, mi, s, ms, offset))
}

/// Parses one line against the grammar above (not the closed value sets).
pub fn parse(raw: &str) -> Result<Line, String> {
    if raw.contains('\n') || raw.contains('\r') {
        return Err(format!("CR or LF inside {raw:?}"));
    }
    let ts = raw.get(..29).ok_or_else(|| format!("too short: {raw:?}"))?;
    parse_ts(ts)?;
    let rest = raw
        .get(29..)
        .and_then(|r| r.strip_prefix(' '))
        .ok_or_else(|| format!("no space after the timestamp: {raw:?}"))?;
    let (level, message) = rest
        .split_once(' ')
        .ok_or_else(|| format!("no message: {raw:?}"))?;
    if !matches!(level, "INFO" | "WARN" | "ERROR") {
        return Err(format!("level {level:?} in {raw:?}"));
    }
    let (head, build, tail): (String, Option<String>, &str) =
        if let Some(after) = message.strip_prefix("voicen ") {
            let (build, tail) = after
                .split_once(") started")
                .ok_or_else(|| format!("start line without ') started': {raw:?}"))?;
            let (version, commit) = build
                .split_once(" (")
                .ok_or_else(|| format!("start line without ' (': {raw:?}"))?;
            let version_ok = !version.is_empty()
                && version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b));
            let commit_ok = !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_alphanumeric());
            if !version_ok || !commit_ok {
                return Err(format!("start line build {build:?} in {raw:?}"));
            }
            let tail = if tail.is_empty() {
                tail
            } else {
                tail.strip_prefix(' ')
                    .ok_or_else(|| format!("start line: junk after 'started': {raw:?}"))?
            };
            ("started".to_string(), Some(format!("{build})")), tail)
        } else {
            let mut words = Vec::new();
            let mut tail = message;
            loop {
                let (token, more) = match tail.split_once(' ') {
                    Some((t, m)) => (t, m),
                    None => (tail, ""),
                };
                if token.contains('=') {
                    break;
                }
                if !is_word(token) {
                    return Err(format!("head word {token:?} in {raw:?}"));
                }
                words.push(token);
                tail = more;
                if tail.is_empty() {
                    break;
                }
            }
            if words.is_empty() {
                return Err(format!("no head in {raw:?}"));
            }
            (words.join(" "), None, tail)
        };
    let mut pairs = BTreeMap::new();
    if !tail.is_empty() {
        for token in tail.split(' ') {
            let (k, v) = token
                .split_once('=')
                .ok_or_else(|| format!("token {token:?} is not key=value in {raw:?}"))?;
            if !is_word(k) {
                return Err(format!("key {k:?} in {raw:?}"));
            }
            // T-118: an `EndpointId` value (`{0.0.1.00000000}.{guid}`) has braces and may have
            // upper-case hex; only the three endpoint keys may, and `check_closed` holds them
            // to the endpoint rule.
            let endpoint_key = matches!(k, "mic" | "system" | "to");
            let value_ok = !v.is_empty()
                && v.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b"_.:,-".contains(&b)
                        || (endpoint_key && (b"{}".contains(&b) || b.is_ascii_uppercase()))
                });
            if !value_ok {
                return Err(format!("value {v:?} of {k} in {raw:?}"));
            }
            if pairs.insert(k.to_string(), v.to_string()).is_some() {
                return Err(format!("key {k} twice in {raw:?}"));
            }
        }
    }
    Ok(Line {
        raw: raw.to_string(),
        ts: ts.to_string(),
        level: level.to_string(),
        head,
        build,
        pairs,
    })
}

/// Parses `raw` or panics with the grammar error.
#[track_caller]
pub fn parsed(raw: &str) -> Line {
    match parse(raw) {
        Ok(line) => line,
        Err(e) => panic!("line breaks the grammar: {e}"),
    }
}

// ---- closed value sets ---------------------------------------------------------

pub const ENGINES: &[&str] = &["api", "builtin", "local_server", "other"];
pub const DETECTORS: &[&str] = &["silero", "energy", "other"];
pub const RESULTS: &[&str] = &["pasted", "copied_only", "copy_manual"];
pub const DICTATION_OUTCOMES: &[&str] = &[
    "delivered",
    "failed",
    "no_speech",
    "too_short",
    "capture_failed",
    "blocked",
    // T-009: an Esc-cancelled recording (FR-22).
    "cancelled",
];
/// Why a press was blocked (T-006 Refresh 5): one literal per
/// `settings::gate::Blocked`.
pub const BLOCK_REASONS: &[&str] = &["no_engine"];
/// The diag microphone table: one literal per `MicCause` (T-051).
pub const MIC_CAUSES: &[&str] = &["no_device", "access_denied", "busy", "other"];
/// The diag failure table: one literal per `FailureReason` plus `other`.
pub const FAILURES: &[&str] = &[
    "invalid_api_key",
    "network_unavailable",
    "cannot_reach",
    "timeout",
    "server_error",
    "unexpected_response",
    "key_store_unavailable",
    "engine_not_configured",
    "clipboard_unavailable",
    "microphone_unavailable",
    "no_local_model",
    "local_engine_failed",
    "other",
];
/// The built-in engine's model warmth on a dictation line (T-098, spec FR-015).
pub const LOCAL_WARMTH: &[&str] = &["cold", "warm"];
pub const WARNING_KINDS: &[&str] = &[
    "vad_fallback",
    "esc_unavailable",
    "toast_failed",
    "models_cleanup_failed",
    "settings_window_failed",
    "change_bridge_failed",
    "settings_opener_failed",
    "tray_failed",
    "tray_follower_failed",
    "hotkey_thread_failed",
    "hotkey_register_failed",
    "dictation_start_failed",
    "overlay_failed",
    "logs_folder_failed",
    "test_connection_failed",
];
/// `pp=<...>` on a dictation line (T-076, spec 003 FR-014): one literal per
/// `events::PostProcessResult` kind.
pub const PP_RESULTS: &[&str] = &["off", "applied", "skipped"];
/// `pp_reason=<...>` (T-076): one literal per `events::SkipKind`, the six
/// `SkipReason::code()` spellings; never a host or a status.
pub const PP_REASONS: &[&str] = &[
    "timeout",
    "unreachable",
    "invalid_key",
    "http",
    "invalid_response",
    "not_configured",
];
pub const LOAD_OUTCOMES: &[&str] = &["loaded", "first_run", "reset", "unavailable"];
/// `settings test_connection result=<...>` (spec 004 R-11, T-046 choice (iv)),
/// besides `http_<status>`.
pub const TEST_CONNECTION_RESULTS: &[&str] = &[
    "ok",
    "cannot_reach",
    "invalid_key",
    "timeout",
    "unexpected",
    "invalid",
    "key_store_unavailable",
];
pub const SAVE_OUTCOMES: &[&str] = &["ok", "refused", "failed"];
pub const FORM_ERRORS: &[&str] = &["write_failed", "settings_unavailable", "partially_restored"];
pub const RECONCILE_ACTIONS: &[&str] = &["none", "written", "removed", "failed"];
/// `previous session outcome=<...>` (T-100, research R7): one literal per
/// `PreviousSession` verdict that is logged (a clean previous session writes no line).
pub const PREVIOUS_SESSION_OUTCOMES: &[&str] =
    &["crashed", "ended_by_installer", "ended_abnormally"];

/// A crash-file name as diag writes it: `crash-YYYYMMDD-HHMMSS-<pid>.txt` (T-100,
/// data-model "CrashFile"); nothing else may follow `crash_file=`.
pub fn crash_file_name(v: &str) -> bool {
    let Some(body) = v
        .strip_prefix("crash-")
        .and_then(|r| r.strip_suffix(".txt"))
    else {
        return false;
    };
    let parts: Vec<&str> = body.split('-').collect();
    matches!(parts.as_slice(), [date, time, pid]
        if digits(date, 8).is_some() && digits(time, 6).is_some() && unsigned(pid))
}

fn unsigned(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) && v.parse::<u64>().is_ok()
}

/// An OS version `<major>.<minor>.<build>` (T-105): exactly three parts, each a
/// `u32` written in plain digits. The closed placeholder `0.0.0`
/// (`OsVersion::UNKNOWN`) is of this form too.
fn os_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok()
        })
}

fn signed(v: &str) -> bool {
    let body = v.strip_prefix('-').unwrap_or(v);
    unsigned(body) && v.parse::<i64>().is_ok()
}

/// A dotted settings id (`engine.api.base_url`) or code (`required`,
/// `url.malformed`): lowercase words joined by dots.
fn dotted(v: &str, min_parts: usize) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() >= min_parts && parts.iter().all(|p| is_word(p))
}

/// `<field id>:<code>` items, comma-separated.
fn field_code_list(v: &str) -> bool {
    v.split(',').all(|item| {
        item.split_once(':')
            .is_some_and(|(f, c)| dotted(f, 2) && dotted(c, 1))
    })
}

fn field_list(v: &str) -> bool {
    v.split(',').all(|f| dotted(f, 2))
}

/// Checks every key of `line` against the closed value set of its head, and the
/// keys every line of that head must have.
pub fn check_closed(line: &Line) -> Result<(), String> {
    let one_of = |k: &str, v: &str, set: &[&str]| {
        if set.contains(&v) {
            Ok(())
        } else {
            Err(format!("{k}={v} is outside {set:?} in {:?}", line.raw))
        }
    };
    let check = |ok: bool, k: &str, v: &str| {
        if ok {
            Ok(())
        } else {
            Err(format!("{k}={v} has the wrong form in {:?}", line.raw))
        }
    };
    let required: &[&str] = match line.head.as_str() {
        // T-105: the OS version is on every start line.
        "started" => &["pid", "os"],
        // A blocked press is not a recording: no `rec`, a `reason` instead.
        "dictation" if line.get("outcome") == Some("blocked") => &["outcome", "reason"],
        "dictation" => &["rec", "outcome"],
        "warning" => &["kind"],
        "settings load" | "settings save" => &["outcome"],
        "settings test_connection" if line.get("result") == Some("ok") => &["result", "latency_ms"],
        "settings test_connection" => &["result"],
        "autostart reconcile" => &["action"],
        "logs recovered" => &[],
        // T-100: the previous-session verdict and the clean exit.
        "previous session" => &["outcome"],
        "exited" => &[],
        // T-118: the call lines (contracts/core-call.md "Log lines").
        "call_requested" => &["source"],
        "call_refused" => &["part", "reason"],
        "call_started" => &["record", "subtitles", "mic", "system", "comms_differs"],
        "call_device" => &["track", "cause", "gap_ms", "to"],
        "call_track_silent" => &["track", "rule"],
        "call_stopped" => &["reason", "duration_ms", "mic_bytes", "system_bytes"],
        "call_storage_failed" => &["kind"],
        "call_recovered" => &["calls"],
        "final_stage" => &["track", "stage", "ms", "chunks"],
        "final_finished" => &["outcome", "mode", "ms"],
        "call_switch_refused" => &["reason"],
        "vad_fallback" => &["track", "cause"],
        "transcript_invalid" => &["rule", "segment"],
        other => return Err(format!("unknown head {other:?} in {:?}", line.raw)),
    };
    for k in required {
        if line.get(k).is_none() {
            return Err(format!("{:?} line without {k}: {:?}", line.head, line.raw));
        }
    }
    for (k, v) in &line.pairs {
        let (k, v) = (k.as_str(), v.as_str());
        match (line.head.as_str(), k) {
            ("started", "pid") => check(unsigned(v), k, v)?,
            ("started", "os") => check(os_version(v), k, v)?,
            ("dictation", "rec" | "press_to_frame_ms" | "duration_ms" | "stop_to_text_ms")
            | ("dictation", "text_to_paste_ms" | "pp_ms" | "load_ms") => {
                check(unsigned(v), k, v)?;
            }
            ("dictation", "local") => one_of(k, v, LOCAL_WARMTH)?,
            ("dictation", "http_status") => {
                check(unsigned(v) && v.parse::<u16>().is_ok(), k, v)?;
            }
            ("dictation", "engine") => one_of(k, v, ENGINES)?,
            ("dictation", "outcome") => one_of(k, v, DICTATION_OUTCOMES)?,
            ("dictation", "result") => one_of(k, v, RESULTS)?,
            ("dictation", "failure") => one_of(k, v, FAILURES)?,
            ("dictation", "detector") => one_of(k, v, DETECTORS)?,
            ("dictation", "mic") => one_of(k, v, MIC_CAUSES)?,
            ("dictation", "reason") => one_of(k, v, BLOCK_REASONS)?,
            ("dictation", "pp") => one_of(k, v, PP_RESULTS)?,
            ("dictation", "pp_reason") => one_of(k, v, PP_REASONS)?,
            ("warning", "kind") => one_of(k, v, WARNING_KINDS)?,
            ("warning", "os_code") => check(signed(v), k, v)?,
            ("settings load", "outcome") => one_of(k, v, LOAD_OUTCOMES)?,
            ("settings save", "outcome") => one_of(k, v, SAVE_OUTCOMES)?,
            ("settings save", "errors" | "warnings") => check(field_code_list(v), k, v)?,
            ("settings save", "form_error") => one_of(k, v, FORM_ERRORS)?,
            ("settings save", "not_restored") => check(field_list(v), k, v)?,
            ("autostart reconcile", "action") => one_of(k, v, RECONCILE_ACTIONS)?,
            ("previous session", "outcome") => one_of(k, v, PREVIOUS_SESSION_OUTCOMES)?,
            ("previous session", "crash_file") => check(crash_file_name(v), k, v)?,
            ("settings test_connection", "result") => match v.strip_prefix("http_") {
                Some(status) => check(unsigned(status) && status.parse::<u16>().is_ok(), k, v)?,
                None => one_of(k, v, TEST_CONNECTION_RESULTS)?,
            },
            // Only an `ok` line has a latency.
            ("settings test_connection", "latency_ms") => {
                check(unsigned(v) && line.get("result") == Some("ok"), k, v)?;
            }
            ("call_requested", "source") => one_of(k, v, CALL_SOURCES)?,
            ("call_refused", "part") => one_of(k, v, REFUSED_PARTS)?,
            ("call_refused", "reason") => one_of(k, v, REFUSED_REASONS)?,
            ("call_started", "record") => one_of(k, v, &["1"])?,
            ("call_started", "subtitles") => one_of(k, v, &["0"])?,
            ("call_started", "comms_differs") => one_of(k, v, &["0", "1"])?,
            ("call_started", "mic" | "system") => check(endpoint(v), k, v)?,
            ("call_device", "track")
            | ("call_track_silent", "track")
            | ("final_stage", "track")
            | ("vad_fallback", "track") => one_of(k, v, TRACKS)?,
            ("call_device", "cause") => one_of(k, v, DEVICE_CAUSES)?,
            ("call_device", "gap_ms") => check(unsigned(v), k, v)?,
            ("call_device", "to") => check(v == "none" || endpoint(v), k, v)?,
            ("call_track_silent", "rule") => one_of(k, v, SILENT_RULES)?,
            ("call_stopped", "reason") => one_of(k, v, STOP_REASONS)?,
            ("call_stopped", "duration_ms" | "mic_bytes" | "system_bytes") => {
                check(unsigned(v), k, v)?;
            }
            ("call_storage_failed", "kind") => one_of(k, v, STORAGE_KINDS)?,
            ("call_storage_failed", "os") => check(signed(v), k, v)?,
            ("call_recovered", "calls") => check(unsigned(v), k, v)?,
            ("final_stage", "stage") => one_of(k, v, FINAL_STAGES)?,
            ("final_stage", "ms" | "chunks") => check(unsigned(v), k, v)?,
            ("final_finished", "outcome") => one_of(k, v, &["ready", "failed"])?,
            ("final_finished", "reason") => one_of(k, v, FAIL_REASONS)?,
            ("final_finished", "mode") => one_of(k, v, &["timed", "timeless"])?,
            ("final_finished", "ms") => check(unsigned(v), k, v)?,
            ("call_switch_refused", "reason") => one_of(k, v, &["unknown_value"])?,
            ("vad_fallback", "cause") => one_of(k, v, &["unavailable", "error"])?,
            ("transcript_invalid", "rule") => one_of(k, v, INVALID_RULES)?,
            ("transcript_invalid", "segment") => check(unsigned(v), k, v)?,
            (head, key) => {
                return Err(format!(
                    "unknown key {key:?} on a {head:?} line: {:?}",
                    line.raw
                ))
            }
        }
    }
    // T-105: `pid=` comes first, right after `started` (the CI smoke matches
    // `started pid=<n>`), and `os=` follows it.
    if line.head == "started" && !line.raw.contains(") started pid=") {
        return Err(format!("a start line begins with pid=: {:?}", line.raw));
    }
    // T-118: `reason=` exactly on a failed final pass; `os=` is optional.
    if line.head == "final_finished"
        && (line.get("outcome") == Some("failed")) != line.get("reason").is_some()
    {
        return Err(format!("reason= exactly on failed final passes: {:?}", line.raw));
    }
    if line.head == "previous session" {
        let outcome = line.get("outcome").unwrap_or_default();
        let file = line.get("crash_file").is_some();
        let ok = match outcome {
            "crashed" => file,
            "ended_by_installer" => !file,
            _ => true,
        };
        if !ok {
            return Err(format!(
                "crash_file= on every crashed line and never on ended_by_installer: {:?}",
                line.raw
            ));
        }
    }
    if line.head == "dictation" {
        let outcome = line.get("outcome").unwrap_or_default();
        if (outcome == "delivered") != line.get("result").is_some() {
            return Err(format!(
                "result= exactly on delivered lines: {:?}",
                line.raw
            ));
        }
        if (outcome == "failed") != line.get("failure").is_some() {
            return Err(format!("failure= exactly on failed lines: {:?}", line.raw));
        }
        if line.get("http_status").is_some() && outcome != "failed" {
            return Err(format!("http_status= only on failed lines: {:?}", line.raw));
        }
        if outcome == "blocked" && line.keys() != ["outcome", "reason"] {
            return Err(format!(
                "a blocked line has only outcome= and reason=: {:?}",
                line.raw
            ));
        }
        if (outcome == "blocked") != line.get("reason").is_some() {
            return Err(format!("reason= exactly on blocked lines: {:?}", line.raw));
        }
        if (outcome == "capture_failed") != line.get("mic").is_some() {
            return Err(format!(
                "mic= exactly on capture_failed lines: {:?}",
                line.raw
            ));
        }
        // T-098: local= only on a built-in line; load_ms= exactly on local=cold.
        let local = line.get("local");
        if local.is_some() && line.get("engine") != Some("builtin") {
            return Err(format!(
                "local= only on engine=builtin lines: {:?}",
                line.raw
            ));
        }
        if (local == Some("cold")) != line.get("load_ms").is_some() {
            return Err(format!(
                "load_ms= exactly on local=cold lines: {:?}",
                line.raw
            ));
        }
        // T-076: the post-processing keys. pp_reason exactly on pp=skipped; pp_ms
        // exactly on pp=applied|skipped (never on off, never without pp); no pp* on
        // a line whose recording never reached the stage.
        let pp = line.get("pp");
        if (pp == Some("skipped")) != line.get("pp_reason").is_some() {
            return Err(format!(
                "pp_reason= exactly on pp=skipped lines: {:?}",
                line.raw
            ));
        }
        if matches!(pp, Some("applied" | "skipped")) != line.get("pp_ms").is_some() {
            return Err(format!(
                "pp_ms= exactly on pp=applied|skipped lines: {:?}",
                line.raw
            ));
        }
        if pp.is_some()
            && matches!(
                outcome,
                "blocked" | "too_short" | "cancelled" | "capture_failed"
            )
        {
            return Err(format!(
                "no pp= on a line whose recording never reached post-processing: {:?}",
                line.raw
            ));
        }
    }
    Ok(())
}

pub const CALL_SOURCES: &[&str] = &["tray", "hotkey", "switch"];
pub const REFUSED_PARTS: &[&str] = &["mic", "system", "root"];
pub const REFUSED_REASONS: &[&str] = &[
    "no_device",
    "access_denied",
    "busy",
    "unwritable",
    "missing",
    "other",
];
pub const TRACKS: &[&str] = &["mic", "system"];
pub const DEVICE_CAUSES: &[&str] = &[
    "default_output_changed",
    "mic_lost",
    "mic_default_changed",
    "track_lost",
    "reopened",
    "no_device",
];
pub const SILENT_RULES: &[&str] = &["lost", "cant_hear"];
pub const STOP_REASONS: &[&str] = &[
    "user",
    "hotkey",
    "exit",
    "system_end",
    "sleep",
    "max_length",
    "low_disk",
    "storage_error",
];
pub const STORAGE_KINDS: &[&str] = &["write", "flush", "sync", "meta", "transcript", "low_disk"];
pub const FINAL_STAGES: &[&str] = &["decode", "vad", "upload", "merge", "write"];
pub const FAIL_REASONS: &[&str] = &[
    "key",
    "network",
    "timeout",
    "provider",
    "storage",
    "engine_not_set_up",
];
pub const INVALID_RULES: &[&str] = &[
    "unsorted",
    "out_of_range",
    "unknown_speaker",
    "unknown_track",
    "bad_source",
    "bad_lang",
    "empty_text",
];

/// An `EndpointId` as the log writes it: `invalid`, or 1..=128 characters of `{`, `}`, `.`,
/// hex digits and `-`.
pub fn endpoint(v: &str) -> bool {
    v == "invalid"
        || ((1..=128).contains(&v.chars().count())
            && v.chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, '{' | '}' | '.' | '-')))
}

/// Grammar plus closed sets, or a panic naming the line.
#[track_caller]
pub fn closed(raw: &str) -> Line {
    let line = parsed(raw);
    if let Err(e) = check_closed(&line) {
        panic!("line outside the closed output: {e}");
    }
    line
}

/// The dictation lines among `lines` (every line checked against the grammar and
/// the closed sets on the way).
#[track_caller]
pub fn dictation_lines(lines: &[String]) -> Vec<Line> {
    lines
        .iter()
        .map(|l| closed(l))
        .filter(|l| l.head == "dictation")
        .collect()
}

/// `key=value` pairs as an ordered map, for exact comparisons.
pub fn pairs(items: &[(&str, String)]) -> BTreeMap<String, String> {
    items
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}
