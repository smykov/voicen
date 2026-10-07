//! The one line formatter (T-008): `<local time with offset> <LEVEL> <message>`.
//!
//! Total over [`LogEvent`]; every value is an integer, a literal from this
//! module's tables (engine, detector, failure, microphone cause, warning kind,
//! load outcome), a closed `as_str`/`code` match of the crate (settings field
//! ids and codes, `ReconcileAction`, `DeliveryResult`, `FormError::kind`), or
//! `BuildInfo`. Std only (the date math of `clock.rs`).
//!
//! ```text
//! 2026-10-04T23:59:00.123+02:00 INFO voicen 0.1.0 (abc1234) started pid=4242
//! 2026-10-04T23:59:04.020+02:00 INFO dictation rec=0 engine=api outcome=delivered result=pasted detector=energy press_to_frame_ms=41 duration_ms=3000 stop_to_text_ms=812 text_to_paste_ms=95
//! 2026-10-04T23:59:04.500+02:00 WARN dictation rec=1 outcome=capture_failed mic=access_denied
//! 2026-10-04T23:59:04.700+02:00 WARN dictation outcome=blocked reason=no_engine
//! 2026-10-04T23:59:05.000+02:00 WARN warning kind=vad_fallback
//! 2026-10-04T23:59:06.000+02:00 INFO settings save outcome=ok warnings=engine.api.base_url:endpoint.insecure
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::borrow::Cow;
use std::fmt::{Display, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use super::event::{
    DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, LoadKind, LogEvent,
    SaveLine, WarningKind,
};
use crate::autostart::ReconcileAction;
use crate::build_info::BuildInfo;
use crate::clock::civil_from_days;
use crate::recording::MicCause;
use crate::settings::gate::Blocked;
use crate::settings::service::FormError;

/// The widest offset written: UTC±14:00, the range of real time zones.
const MAX_OFFSET_SECS: i32 = 14 * 3_600;
/// 9999-12-31T23:59:59.999 in milliseconds since the epoch: the last local instant
/// with a four-digit year. Earlier than the epoch or later than this is clamped.
const MAX_LOCAL_MS: i64 = 253_402_300_799_999;
const MS_PER_DAY: u64 = 86_400_000;

/// One line for `event` at `at`, local time = UTC + `seconds_east`
/// (`YYYY-MM-DDTHH:MM:SS.mmm+HH:MM`). Pure; no `'\n'` or `'\r'` inside and no
/// terminating newline (the [`Log`](super::Log) adds it).
///
/// The offset is clamped to ±14:00 and cut to whole minutes, and the time is
/// computed with that offset, so the timestamp is always self-consistent; the
/// local time is clamped to 1970-01-01T00:00:00.000 ..= 9999-12-31T23:59:59.999.
pub fn format_line(at: SystemTime, seconds_east: i32, event: &LogEvent) -> String {
    let mut line = String::with_capacity(160);
    write_timestamp(&mut line, &LocalTime::of(at, seconds_east));
    line.push(' ');
    line.push_str(level(event));
    line.push(' ');
    write_message(&mut line, event);
    line
}

/// A local wall time, the fields of a timestamp.
pub(crate) struct LocalTime {
    pub(crate) year: u64,
    pub(crate) month: u64,
    pub(crate) day: u64,
    pub(crate) hour: u64,
    pub(crate) minute: u64,
    pub(crate) second: u64,
    pub(crate) millis: u64,
    /// The offset actually applied, in seconds east (whole minutes, ±14:00).
    pub(crate) offset: i32,
    /// Days since 1970-01-01 of the local date (the date roll compares these).
    pub(crate) day_number: u64,
}

impl LocalTime {
    pub(crate) fn of(at: SystemTime, seconds_east: i32) -> LocalTime {
        let offset = seconds_east.clamp(-MAX_OFFSET_SECS, MAX_OFFSET_SECS) / 60 * 60;
        let utc_ms: i64 = match at.duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
        };
        let local_ms = utc_ms
            .saturating_add(i64::from(offset) * 1_000)
            .clamp(0, MAX_LOCAL_MS);
        let local_ms = u64::try_from(local_ms).unwrap_or(0);
        let day_number = local_ms / MS_PER_DAY;
        let rem = local_ms % MS_PER_DAY;
        let (year, month, day) = civil_from_days(day_number);
        LocalTime {
            year,
            month,
            day,
            hour: rem / 3_600_000,
            minute: rem / 60_000 % 60,
            second: rem / 1_000 % 60,
            millis: rem % 1_000,
            offset,
            day_number,
        }
    }
}

fn write_timestamp(out: &mut String, t: &LocalTime) {
    let sign = if t.offset < 0 { '-' } else { '+' };
    let abs = t.offset.unsigned_abs();
    let _ = write!(
        out,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{sign}{:02}:{:02}",
        t.year,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second,
        t.millis,
        abs / 3_600,
        abs % 3_600 / 60
    );
}

fn level(event: &LogEvent) -> &'static str {
    const INFO: &str = "INFO";
    const WARN: &str = "WARN";
    match event {
        LogEvent::Started { .. } | LogEvent::LogsRecovered => INFO,
        LogEvent::Dictation(line) => match line.outcome {
            DictationOutcome::Failed { .. } | DictationOutcome::CaptureFailed { .. } => WARN,
            DictationOutcome::Delivered(_)
            | DictationOutcome::NoSpeech
            | DictationOutcome::TooShort => INFO,
        },
        LogEvent::DictationBlocked { .. } | LogEvent::Warning { .. } => WARN,
        LogEvent::SettingsLoad(kind) => match kind {
            LoadKind::Loaded | LoadKind::FirstRun => INFO,
            LoadKind::Reset | LoadKind::Unavailable => WARN,
        },
        LogEvent::SettingsSave(line) => match save_outcome(line) {
            SaveOutcomeLiteral::Failed => WARN,
            SaveOutcomeLiteral::Ok | SaveOutcomeLiteral::Refused => INFO,
        },
        LogEvent::AutostartReconcile(action) => match action {
            ReconcileAction::Failed => WARN,
            ReconcileAction::None | ReconcileAction::Written | ReconcileAction::Removed => INFO,
        },
    }
}

fn write_message(out: &mut String, event: &LogEvent) {
    match event {
        LogEvent::Started { build, pid } => {
            let BuildInfo { version, commit } = *build;
            let _ = write!(
                out,
                "voicen {} ({}) started",
                build_text(version, |b| b.is_ascii_alphanumeric()
                    || b".+-".contains(&b)),
                build_text(commit, |b| b.is_ascii_alphanumeric()),
            );
            pair(out, "pid", pid);
        }
        LogEvent::Dictation(line) => write_dictation(out, line),
        LogEvent::DictationBlocked { reason } => {
            out.push_str("dictation");
            pair(out, "outcome", "blocked");
            pair(out, "reason", block_reason(*reason));
        }
        LogEvent::Warning { kind, os_code } => {
            out.push_str("warning");
            pair(out, "kind", warning_kind(*kind));
            if let Some(code) = os_code {
                pair(out, "os_code", code);
            }
        }
        LogEvent::SettingsLoad(kind) => {
            out.push_str("settings load");
            pair(out, "outcome", load_kind(*kind));
        }
        LogEvent::SettingsSave(line) => write_save(out, line),
        LogEvent::AutostartReconcile(action) => {
            out.push_str("autostart reconcile");
            pair(out, "action", action.as_str());
        }
        LogEvent::LogsRecovered => out.push_str("logs recovered"),
    }
}

fn write_dictation(out: &mut String, line: &DictationLine) {
    let DictationLine {
        recording,
        engine: engine_tag,
        outcome,
        detector: detector_tag,
        press_to_frame_ms,
        duration_ms,
        stop_to_text_ms,
        text_to_paste_ms,
    } = line;
    out.push_str("dictation");
    pair(out, "rec", recording);
    if let Some(tag) = engine_tag {
        pair(out, "engine", engine(*tag));
    }
    match outcome {
        DictationOutcome::Delivered(result) => {
            pair(out, "outcome", "delivered");
            pair(out, "result", result.code());
        }
        DictationOutcome::Failed {
            failure: tag,
            http_status,
        } => {
            pair(out, "outcome", "failed");
            pair(out, "failure", failure(*tag));
            if let Some(status) = http_status {
                pair(out, "http_status", status);
            }
        }
        DictationOutcome::NoSpeech => pair(out, "outcome", "no_speech"),
        DictationOutcome::TooShort => pair(out, "outcome", "too_short"),
        DictationOutcome::CaptureFailed { cause } => {
            pair(out, "outcome", "capture_failed");
            pair(out, "mic", mic(*cause));
        }
    }
    if let Some(tag) = detector_tag {
        pair(out, "detector", detector(*tag));
    }
    for (key, value) in [
        ("press_to_frame_ms", press_to_frame_ms),
        ("duration_ms", duration_ms),
        ("stop_to_text_ms", stop_to_text_ms),
        ("text_to_paste_ms", text_to_paste_ms),
    ] {
        if let Some(ms) = value {
            pair(out, key, ms);
        }
    }
}

/// `settings save outcome=<ok|refused|failed>` (spec 004 R-11): a refusal the user
/// can fix (field errors, the Unavailable service) is `refused`; a file or key
/// store failure (`write_failed`, `partially_restored`) is `failed`.
#[derive(Clone, Copy)]
enum SaveOutcomeLiteral {
    Ok,
    Refused,
    Failed,
}

fn save_outcome(line: &SaveLine) -> SaveOutcomeLiteral {
    match line {
        SaveLine::Saved { .. } => SaveOutcomeLiteral::Ok,
        SaveLine::Refused { form_error, .. } => match form_error {
            None | Some(FormError::SettingsUnavailable) => SaveOutcomeLiteral::Refused,
            Some(FormError::WriteFailed | FormError::PartiallyRestored { .. }) => {
                SaveOutcomeLiteral::Failed
            }
        },
    }
}

fn write_save(out: &mut String, line: &SaveLine) {
    out.push_str("settings save");
    let outcome = match save_outcome(line) {
        SaveOutcomeLiteral::Ok => "ok",
        SaveOutcomeLiteral::Refused => "refused",
        SaveOutcomeLiteral::Failed => "failed",
    };
    pair(out, "outcome", outcome);
    match line {
        SaveLine::Saved { warnings } => {
            list(
                out,
                "warnings",
                warnings
                    .iter()
                    .map(|w| format!("{}:{}", w.field.as_str(), w.code.as_str())),
            );
        }
        SaveLine::Refused { errors, form_error } => {
            list(
                out,
                "errors",
                errors
                    .iter()
                    .map(|e| format!("{}:{}", e.field.as_str(), e.code.as_str())),
            );
            if let Some(form_error) = form_error {
                pair(out, "form_error", form_error.kind());
                if let FormError::PartiallyRestored { not_restored } = form_error {
                    list(
                        out,
                        "not_restored",
                        not_restored.iter().map(|f| f.as_str().to_string()),
                    );
                }
            }
        }
    }
}

/// ` key=value`.
fn pair(out: &mut String, key: &str, value: impl Display) {
    let _ = write!(out, " {key}={value}");
}

/// ` key=a,b,c`; nothing for an empty list (a key never has an empty value).
fn list(out: &mut String, key: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    if !items.is_empty() {
        pair(out, key, items.join(","));
    }
}

/// A `BuildInfo` field as written: unchanged when every byte is allowed (the real
/// version and commit are); otherwise only its allowed bytes, and `unknown` when
/// none is left, so a hand-built `BuildInfo` cannot break the line.
fn build_text(s: &'static str, allowed: impl Fn(u8) -> bool) -> Cow<'static, str> {
    if !s.is_empty() && s.bytes().all(&allowed) {
        return Cow::Borrowed(s);
    }
    let kept: String = s.bytes().filter(|b| allowed(*b)).map(char::from).collect();
    if kept.is_empty() {
        Cow::Borrowed("unknown")
    } else {
        Cow::Owned(kept)
    }
}

// ---- the literal tables ----------------------------------------------------------

fn engine(tag: EngineTag) -> &'static str {
    match tag {
        EngineTag::Api => "api",
        EngineTag::Builtin => "builtin",
        EngineTag::LocalServer => "local_server",
        EngineTag::Other => "other",
    }
}

fn detector(tag: DetectorTag) -> &'static str {
    match tag {
        DetectorTag::Silero => "silero",
        DetectorTag::Energy => "energy",
        DetectorTag::Other => "other",
    }
}

fn failure(tag: FailureTag) -> &'static str {
    match tag {
        FailureTag::InvalidApiKey => "invalid_api_key",
        FailureTag::NetworkUnavailable => "network_unavailable",
        FailureTag::CannotReach => "cannot_reach",
        FailureTag::Timeout => "timeout",
        FailureTag::ServerError => "server_error",
        FailureTag::UnexpectedResponse => "unexpected_response",
        FailureTag::KeyStoreUnavailable => "key_store_unavailable",
        FailureTag::EngineNotConfigured => "engine_not_configured",
        FailureTag::ClipboardUnavailable => "clipboard_unavailable",
        FailureTag::MicrophoneUnavailable => "microphone_unavailable",
        FailureTag::Other => "other",
    }
}

/// `MicCause` (already closed, no OS text) to its literal; `Other` is `other`,
/// like the other tables' unknown value.
fn mic(cause: MicCause) -> &'static str {
    match cause {
        MicCause::NoDevice => "no_device",
        MicCause::AccessDenied => "access_denied",
        MicCause::Busy => "busy",
        MicCause::Other => "other",
    }
}

/// `settings::gate::Blocked` to its literal (one per variant).
fn block_reason(reason: Blocked) -> &'static str {
    match reason {
        Blocked::NoEngine => "no_engine",
    }
}

fn warning_kind(kind: WarningKind) -> &'static str {
    match kind {
        WarningKind::VadFallback => "vad_fallback",
        WarningKind::EscUnavailable => "esc_unavailable",
        WarningKind::ToastFailed => "toast_failed",
        WarningKind::ModelsCleanupFailed => "models_cleanup_failed",
        WarningKind::SettingsWindowFailed => "settings_window_failed",
        WarningKind::ChangeBridgeFailed => "change_bridge_failed",
        WarningKind::SettingsOpenerFailed => "settings_opener_failed",
        WarningKind::TrayFailed => "tray_failed",
        WarningKind::TrayFollowerFailed => "tray_follower_failed",
        WarningKind::HotkeyThreadFailed => "hotkey_thread_failed",
        WarningKind::HotkeyRegisterFailed => "hotkey_register_failed",
        WarningKind::DictationStartFailed => "dictation_start_failed",
        WarningKind::OverlayFailed => "overlay_failed",
        WarningKind::LogsFolderFailed => "logs_folder_failed",
    }
}

fn load_kind(kind: LoadKind) -> &'static str {
    match kind {
        LoadKind::Loaded => "loaded",
        LoadKind::FirstRun => "first_run",
        LoadKind::Reset => "reset",
        LoadKind::Unavailable => "unavailable",
    }
}
