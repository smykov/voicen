# Contract: `voicen_core::diag` and `voicen_core::paths` (core API)

Platform-independent; tested on the Linux host with a temp directory and a fake clock. Used by the shell (`src-tauri`) and, through `PipelineObserver`, by features 001–005. Signatures are the contract; bodies are not.

**Updated by T-008 (2026-10-04).** The Clock, Log and Pipeline observer sections below are the signatures T-008 shipped; they replace the earlier draft (a chrono `WallClock`, `Log::open(&AppPaths, ..)`, `retain_now()`, `format_line(DateTime<FixedOffset>, ..)`, `LogsUnwritableReason::Other(i32)`, `dictation_finished(&DictationRecord)`, `HostName::from_url`). Session and Crash files were built by T-100 (2026-10-17) over the same time ports; see their sections. Why: `docs/decisions/diagnostics-log.md`.

## Paths (P-010)

```rust
pub struct AppPaths { /* root */ }
impl AppPaths {
    pub fn new(local_app_data: &Path) -> Self;     // root = <base>/Voicen
    pub fn root(&self) -> &Path;
    pub fn logs(&self) -> PathBuf;                 // root/logs
    pub fn webview(&self) -> PathBuf;              // root/webview  (spec FR-034)
    // models(), tmp(), settings_file(), history_file(): owned by 002/001/004/005
}
```

If 001 or 004 creates `AppPaths` first, this feature only adds `logs()`/`webview()`.

*Not built (T-008):* the one resolver of the data folder is the shell's `paths` module (`src-tauri/src/paths.rs`: `data_dir()`, `log_dir()`, `models_dir()`; T-030, `docs/architecture.md` "data directory"). `Log::open` takes the logs dir as a `PathBuf`, and the shell passes `paths::log_dir()`. A later `AppPaths` in core replaces that module; it does not sit beside it.

## Clock (P-010, P-011: one clock)

```rust
// voicen_core::clock — the one wall-clock port of the core (exists since T-032; LocalOffset added by T-008)
pub trait Clock: Send + Sync {
    fn now(&self) -> std::time::SystemTime;                     // the instant (UTC)
}
pub trait LocalOffset: Send + Sync {
    fn seconds_east(&self, t: std::time::SystemTime) -> i32;    // local − UTC at `t`, DST included
}
pub struct SystemClock;                                         // impl Clock (SystemTime::now)
// test-fakes: FakeClock::at(t) / set(t)
```

- There is no `WallClock` and no chrono in core. The wall time comes from `Clock`. The local offset at an instant comes from `LocalOffset`; the shell fills it from the Windows time-zone rules (`SystemTimeToTzSpecificLocalTime`, `windows` feature `Win32_System_Time`, UTC when the conversion fails). The date math is std-only (`clock::civil_from_days`).
- **What later tasks reuse:** the session marker, the crash files and T-054 read time through this `Clock` and this `LocalOffset`, with the same shell implementations the log uses (`SystemClock`, and `OsLocalOffset` in `src-tauri/src/diag.rs`). A task that needs them outside the log gets them from `diag::start`, the one place that constructs them; it does not construct a second offset source. They write local time through diag's own timestamp formatting (`YYYY-MM-DDTHH:MM:SS.mmm±HH:MM`, offset clamped to ±14:00 and whole minutes), never through chrono or a second clock or formatter. A new time need extends these ports; it adds no parallel one.

## Log

```rust
// voicen_core::diag
pub enum LogEvent {
    Started { build: BuildInfo, pid: u32 },                 // `voicen <version> (<commit>) started pid=<n>`
    Dictation(DictationLine),                               // written by LogObserver only
    Warning { kind: WarningKind, os_code: Option<i32> },
    SettingsLoad(LoadKind),                                 // LogEvent::settings_load(&LoadOutcome)
    SettingsSave(SaveLine),                                 // LogEvent::settings_save(&SaveOutcome)
    AutostartReconcile(ReconcileAction),
    LogsRecovered,                                          // written by the log itself
}   // later tasks add variants (data-model.md "LogEvent"); a variant is the only way to log something new

pub struct DictationLine { pub recording: u64, pub engine: Option<EngineTag>, pub outcome: DictationOutcome,
                           pub detector: Option<DetectorTag>, pub press_to_frame_ms: Option<u64>,
                           pub duration_ms: Option<u64>, pub stop_to_text_ms: Option<u64>,
                           pub text_to_paste_ms: Option<u64>,
                           pub post_processing: Option<PostProcessTrace>,    // T-076: pp= / pp_reason= / pp_ms=
                           pub local: Option<Warmth> }   // T-098: local=cold load_ms=<n> / local=warm; Some only for engine=builtin
pub enum DictationOutcome { Delivered(DeliveryResult), Failed { failure: FailureTag, http_status: Option<u16> },
                            NoSpeech, TooShort }
impl EngineTag   { pub fn from_kind(kind: &str) -> EngineTag; }      // api | builtin | local_server | other
impl DetectorTag { pub fn from_name(name: &str) -> DetectorTag; }    // silero | energy | other
impl FailureTag  { pub fn from_code(code: &str) -> FailureTag; }     // one per FailureReason::code() | other

pub enum LogsUnwritableReason { PermissionDenied, DiskFull, NotADirectory, Other(Option<i32>) }  // raw OS code if any
impl LogsUnwritableReason { pub fn message_id(self) -> MessageId; }   // logs_reason.{permission_denied,disk_full,not_a_directory,other}; every Other is other (T-054)

pub struct LogConfig { pub roll_bytes: u64 /*2 MiB*/, pub total_bytes: u64 /*10 MiB*/, pub max_age: Duration /*7 d*/,
                       pub reopen_every: Duration /*60 s*/ }        // impl Default with these values
pub type OnUnwritable = Box<dyn Fn(LogsUnwritableReason) + Send + Sync>;

pub struct Log { /* Mutex<Writer> */ }                               // Send + Sync; one per primary process
impl Log {
    pub fn open(dir: PathBuf, clock: Arc<dyn Clock>, offset: Arc<dyn LocalOffset>, cfg: LogConfig,
                on_unwritable: OnUnwritable) -> Log;   // never fails; creates `dir`; runs retention; writes nothing
    pub fn write(&self, event: LogEvent);              // never panics, never surfaces an I/O error
}

pub fn format_line(at: SystemTime, seconds_east: i32, event: &LogEvent) -> String;   // pure; one line, no '\n' / '\r'
```

The shell opens the log only through `src-tauri` `diag::start(logs_dir, on_unwritable) -> Arc<Log>`, which writes `Started` first. Retention has no public entry point: `open` runs it, and so does every roll.

Guarantees (each pinned by a test):
- `format_line(Started)` contains `voicen <version> (<commit>) started` (CI greps `(<commit>) started`): `diag_format.rs` `started_line_carries_version_commit_and_pid`.
- Closed output. No `LogEvent` field is a `String`, `&str`, path, `io::Error` or `FailureReason`, so a transcript, a key, a URL, a host or error text cannot be passed in: the `compile_fail` doctests in `diag/mod.rs`. An outside `&'static str` (engine kind, detector, failure code) passes through its table, where an unknown value is `other`: `diag_format.rs` `every_log_event_formats_to_one_closed_line`, `foreign_engine_detector_and_failure_strings_become_other`. No model name and no host is logged (T-008 Q1, decision #64), so `HostName` / `ModelName` do not exist yet.
- `write` from N threads produces N complete lines (no interleaving): `diag_log.rs` `eight_threads_write_4000_whole_lines`.
- Size: the active `voicen.log` ≤ `roll_bytes`, rolled before a line would cross it or at the local date change. The rolled `voicen-*.log` files total ≤ `total_bytes − roll_bytes`, so the whole set stays ≤ `total_bytes` after every write. Rolled files older than `max_age` are deleted at open and after each roll: `size_bound_holds_after_every_write_*`, `date_roll_follows_local_midnight_at_an_offset`, `rolled_files_*`.
- Retention touches only regular `voicen-*.log` files. It never deletes `voicen.log`, `crash-*.txt`, `session.marker`, `installer-ended` or any other file: `a_clock_one_year_off_deletes_at_most_what_the_rules_name`.
- `on_unwritable` is called at most once per `Log` instance, outside the lock. A degraded log retries the open at most every `reopen_every`, writes `LogsRecovered` first after a successful reopen, and does not replay dropped lines: `a_file_at_the_logs_path_*`, `a_full_disk_*`, `a_degraded_log_reopens_*`.
- Windows, a reader that blocks the roll. The roll closes the log's own handle and then renames `voicen.log`. A rename needs every other open handle on the file to share delete. A reader that shares read and write but not delete (.NET `FileShare.ReadWrite`, as used by a tail such as `Get-Content -Wait` or a log viewer) makes the rename fail. The log then goes Degraded, `on_unwritable` is called once, and lines are dropped, in 60 s retry windows, until the reader closes. The rename fails with a sharing violation (OS error 32), which std 1.99 maps to no `ErrorKind`, so the reason is `Other(Some(32))` and the notice shows `logs_reason.other` (measured by Windows CI run 38234346755; pinned by `src-tauri/tests/diag.rs::a_roll_blocked_by_a_reader_without_share_delete_degrades_with_one_notice`). Decided by T-054: a blocked roll is "not writable" like any other failure (retrying without degrading would write past `roll_bytes`).

## Logs notice (T-054)

```rust
pub trait LogsNoticeSink: Send + Sync { fn logs_unwritable(&self, reason: LogsUnwritableReason); }
pub struct LogsNotice { /* Mutex<Empty | Pending(reason) | Attached(sink) | Delivered> */ }
impl LogsNotice {
    pub fn new() -> LogsNotice;
    pub fn callback(self: &Arc<Self>) -> OnUnwritable;      // the log's on_unwritable
    pub fn attach(&self, sink: Arc<dyn LogsNoticeSink>);
}
impl LogsNoticeSink for voicen_core::dictation::DictationSession { /* notice_with(notice.logs_unwritable, [("reason", logs_reason.*)], now) */ }
impl DictationSession { pub fn notice_with(&self, id: MessageId, params: Vec<(&'static str, String)>, at: Instant); }
```

The latch between the log and the user: one per process (`src-tauri` `release_parts`), its callback given to `diag::start`. A reported failure is delivered exactly once, whatever the order: a call before the attach is latched and delivered by `attach` on the attaching thread before it returns; a call after the attach is delivered on one spawned thread (the caller may hold the session lock: the session publishes to the tray under it, and the tray's render writes a log line); a call while attaching is one of the two. The callback takes only the latch's own mutex and never calls the sink inline. Later calls, and a second log's, add nothing. If the delivery thread cannot be spawned the notice is lost. The latch holds the attached sink strongly until it delivers (the latch is its only owner in tests that attach a fresh sink). In release this makes a reference cycle while no notice has been delivered: session -> `PipelineDeps.observer` (`LogObserver`) -> `Log` -> `on_unwritable` -> `Arc<LogsNotice>` -> the attached session, so the session's `Drop` (which stops and joins its threads) never runs. That is harmless there: the session is Tauri managed state and lives for the whole process. A caller that relies on dropping the session (a harness, a session restart) must not attach it to a log it keeps, or must give that log a fresh latch. The session shows it with `DictationSession::notice_with`: the controller's `notice_with` under the session lock plus one publish, so the overlay shows `Message{notice.logs_unwritable, reason}` for 3 s, the tray is unchanged, the newest message wins the one slot and the next press drops it. Tests: `diag_logs_notice.rs` (orders, off-lock delivery, 8-thread race, reason map), `dictation_session.rs` (`notice_with_shows_the_message_for_3s_*`, `a_log_unwritable_before_the_session_*`, `a_log_failing_under_the_session_lock_*`), `src-tauri/tests/diag.rs` (ACL-denied folder; blocked roll; Windows CI).

## Pipeline observer (seam with 001 — P-011)

```rust
// declared by 001 in events.rs; implemented here
pub struct LogObserver { /* Arc<Log>, open records */ }
impl LogObserver { pub fn new(log: Arc<Log>) -> LogObserver; }
impl PipelineObserver for LogObserver { fn event(&self, e: &DictationEvent); }
```

`LogObserver` aggregates the events of each recording into one `LogEvent::Dictation` line. It joins them by `RecordingId` (`JobFinished.recording`, added by T-008) and joins `Delivered` to its job by `seq`, never by event order. A record closes at `Delivered` for a text job, at `JobFinished` for no-speech or failed, at `RecordingEnded{TooShort}` for a discarded hold, and at `CaptureFailed` for a capture that failed at press or at stop (T-051). A pipeline `Warning` is its own line at once, and so is a blocked press (`PressBlocked`, T-006), which touches no open record. Every match over `DictationEvent`, `OutcomeCode`, `RecordingEnd` and `WarningCode` is exhaustive, so a new variant does not compile until it has a log mapping. At most 64 open records and 64 text jobs awaiting `Delivered` are held. Past that the oldest is dropped without a line: its timings are lost, and its late `Delivered` writes nothing. Tests: `diag_observer.rs` (`interleaved_jobs_are_joined_by_recording_id`, `delivered_is_joined_to_its_job_by_seq`, `open_records_are_bounded_*`, `text_jobs_awaiting_delivery_are_bounded_*`), `diag_pipeline.rs` `log_holds_no_key_query_or_transcript_on_any_path`. 001 owns the trait and the events. This feature owns `LogObserver` and the line format. The dictation session (T-051) takes its observer only from `PipelineDeps.observer` and emits its own events (`RecordingStarted`, `RecordingEnded`, `CaptureFailed`) on that instance, so the release wiring (T-006) passes the one `Arc<LogObserver>` there. There is no second observer and no no-op observer in release.

As built (T-008, T-051): a `CaptureFailed` record is written as `DictationOutcome::CaptureFailed { cause: MicCause }`, formatted `dictation rec=<n> outcome=capture_failed mic=<no_device|access_denied|busy|other>` at WARN, plus `press_to_frame_ms` / `duration_ms` when the release's events came first (a failure at stop). Only the closed `MicCause` is written, never OS text. Tests: `diag_observer.rs` `a_capture_failure_at_press_is_one_warn_line_with_the_mic_literal`, `a_capture_failure_at_stop_closes_the_open_record_in_one_line`.

As built (T-006): a press the dictation gate blocks is one `DictationEvent::PressBlocked { reason: Blocked }`, written at once as `LogEvent::DictationBlocked { reason }`, formatted `dictation outcome=blocked reason=<no_engine>` at WARN (the reason through a closed table, one literal per `Blocked` variant). It has no `rec=`: a blocked press takes no `RecordingId`, so the next recording's number is unchanged. So every press that reaches the session leaves a line (a recording's dictation line, or the blocked line). Tests: `diag_blocked_press.rs`, `dictation_session.rs` `a_blocked_press_writes_one_log_line_and_takes_no_recording_number`.

## Session

**As built (T-100, 2026-10-17).** `crates/voicen-core/src/diag/session.rs`. The time parameters are the ports of "Clock" above; the seven inputs are one `SessionStart` struct (no `too_many_arguments` allow). `started` is the clock's now at `begin`; its local text is diag's timestamp format with the same `LocalOffset`. Wired by T-024: the shell's `diag::begin_session(logs_dir, &Arc<Log>) -> SessionEnd` (`src-tauri/src/diag.rs`), called by `release_parts` right after `diag::start` in the primary instance only, runs in this fixed order: `RtlGetVersion` → `Session::begin` → `install_panic_hook` (`session_started = session.started()`) → the native-fault filter → `retain_crash_files`. `diag::start` stays hook-free. `SessionEnd` is managed state; the single `RunEvent::Exit` arm of `on_run_event` (tray Exit, and `WM_ENDSESSION(TRUE)` through tao) calls `end_clean` at most once. No other exit path deletes the marker.

```rust
pub struct SessionStart<'a> { pub logs_dir: &'a Path, pub build: BuildInfo, pub os: OsVersion, pub pid: u32,
                              pub clock: &'a dyn Clock, pub offset: &'a dyn LocalOffset }

pub enum PreviousSession { Clean, Crashed { crash_file: CrashFileName }, EndedByInstaller,
                           EndedAbnormally { crash_file: Option<CrashFileName> } }

pub struct Session { /* marker path, started */ }
impl Session {
    /// Classifies the previous session (research R7), writes an abnormal-end crash file if needed,
    /// deletes the old marker, the installer note and stale crash-*.tmp files, writes the new marker,
    /// writes one `previous session` line (none for Clean). Never fails or panics. Primary instance only.
    pub fn begin(start: SessionStart<'_>, log: &Log) -> (Session, PreviousSession);
    pub fn started(&self) -> SystemTime;
    pub fn end_clean(self, log: &Log);           // deletes the marker; logs `exited`. Drop does not.
}
```

- Marker: `pid=<n>\nstarted=<ts>\nversion=<v>\ncommit=<c>\n`, written as `session.marker.tmp` and renamed. Reading an old marker parses `pid` (decimal digits, `u32`) and `started` (diag's timestamp grammar, accepted only when it formats back byte for byte); any other byte is ignored and never copied. Garbage, empty or non-UTF-8 → both absent, still a marker.
- Verdict, in order: no marker → `Clean`; a `crash-*.txt` diag named, holding the line `session_started: <old started>` → `Crashed` (no new file); `logs/installer-ended` whose modification time is not older than the old marker's modification time → `EndedByInstaller` (the note's text is not read; R7's "time ≥ started" is decided on file times, so there is no second timestamp parser); otherwise → `EndedAbnormally` with one `abnormal_end` file (`crash_file: None` when it cannot be written). The order holds for every marker: a garbage, empty or non-UTF-8 marker is still a marker, so with a note not older than it the verdict is `EndedByInstaller` (no file), and only without such a note `EndedAbnormally` (`a_corrupt_marker_with_a_newer_installer_note_is_ended_by_installer`).
- The `abnormal_end` file carries the current build's version and commit, the OS, the old marker's `started` as `session_started` and its `pid` as `previous_session_pid` (each left out when it did not parse). Whose build it names is OQ-38 (built with the proposal: the current one, typed).
- Log lines: `previous session outcome=crashed crash_file=<name>` (WARN), `previous session outcome=ended_by_installer` (INFO), `previous session outcome=ended_abnormally [crash_file=<name>]` (WARN), `exited` (INFO, no field: the shell cannot tell tray Exit from session end, both are `RunEvent::Exit`). `LogEvent::PreviousSession(PreviousSessionLine)` has no `Clean` variant; `CrashFileName` is held as integers.
- Tests: `tests/diag_session.rs`.

## Crash files

**As built (T-100, 2026-10-17).** `crates/voicen-core/src/diag/crash.rs`. The panic hook gets its own `Arc` clones of the `Clock` and `LocalOffset` the log uses, and two ports filled on Windows by T-024: `FrameSource` (`RtlCaptureStackBackTrace`; std's `Backtrace` gives no frame addresses on stable) and `ModuleResolver` (`GetModuleHandleExW` + `GetModuleFileNameW`), both in `src-tauri/src/win/crash.rs`. Installed by `diag::begin_session` (T-024; see "Session").

```rust
pub enum CrashKind { Panic, NativeFault, AbnormalEnd }          // as_str: panic / native_fault / abnormal_end
pub struct OsVersion { pub major: u32, pub minor: u32, pub build: u32 }   // Display major.minor.build
pub struct LocalStamp { pub at: SystemTime, pub seconds_east: i32 }
pub struct CrashFileName { /* local date-time fields + pid */ }  // Display crash-YYYYMMDD-HHMMSS-<pid>.txt
// test-fakes: CrashFileName::fake(&str) -> Option<CrashFileName>   // same acceptance as diag's own parse

pub trait FrameSource: Send + Sync { fn capture(&self, out: &mut [usize]) -> usize; }   // count clamped
pub trait ModuleResolver: Send + Sync { fn resolve(&self, ip: usize) -> Option<(String, usize)>; }

pub struct CrashContext { pub logs_dir: PathBuf, pub build: BuildInfo, pub os: OsVersion,
                          pub session_started: SystemTime, pub pid: u32, pub clock: Arc<dyn Clock>,
                          pub offset: Arc<dyn LocalOffset>, pub frames: Arc<dyn FrameSource>,
                          pub modules: Arc<dyn ModuleResolver> }

pub struct CrashFields { pub time: LocalStamp, pub build: BuildInfo, pub os: OsVersion,
                         pub session_started: Option<LocalStamp>, pub detail: CrashDetail }
pub enum CrashDetail { Panic { thread: ThreadName, location: Option<CodeLocation>, backtrace: Vec<Frame> },
                       NativeFault { exception_code: u32, fault_address: Option<Frame> },
                       AbnormalEnd { previous_session_pid: Option<u32> } }   // kind() -> CrashKind
ThreadName::new(Option<&str>)  CodeLocation::new(&str, u32, u32)  Frame::new(&str, usize)

pub fn install_panic_hook(ctx: CrashContext);        // std::panic::set_hook; never reads the payload
pub fn render_crash_file(fields: &CrashFields) -> String;   // pure; allowlisted keys only, data-model order
pub fn retain_crash_files(logs_dir: &Path, now: SystemTime);   // > 30 days by mtime, then all but the 20 newest
pub const CRASH_MAX_AGE_DAYS: u32 = 30;
pub const CRASH_MAX_FILES: usize = 20;

// diag::native_fault (T-024): the second writer, for the shell's unhandled-exception filter
pub struct NativeFaultSetup { pub build: BuildInfo, pub os: OsVersion, pub session_started: SystemTime,
                              pub pid: u32, pub seconds_east: i32 }
pub trait FaultFiles { fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()>;
                       fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()>;
                       fn remove(&mut self, tmp: &str); }
pub struct NativeFaultWriter;                                   // Send + Sync
impl NativeFaultWriter {
    pub fn new(setup: NativeFaultSetup) -> NativeFaultWriter;   // allocates (at install)
    pub fn record(&self, now: SystemTime, exception_code: u32, fault_address: usize,
                  module: Option<(&[u16], usize)>, files: &mut dyn FaultFiles) -> Option<CrashFileName>;
}
```

- Text: `key: value` lines; the panic kind writes a bare `backtrace:` line and then `  <module>+0x<lowercase hex>` lines; `exception_code: 0x%08X`; an absent optional is left out. Bounded texts: control characters removed, at most 255 bytes; a module is reduced to the text after its last `/` or `\` (a full path names the user); an empty thread name is `unnamed`; an unresolved frame is `unknown+0x<address>`.
- Guarantees: the hook never formats or reads the payload; never panics (every error ignored; std aborts a panic inside a hook, so this is by construction); takes no Voicen lock (T-024: the Windows ports `RtlCaptureStackBackTrace`, `GetModuleHandleExW` and `GetModuleFileNameW` take OS loader / function-table locks; accepted as a residual) and never calls `Log::write` (decision #134: no log line from the hook, so no `CrashFileWritten` event); does not chain the previous hook (std's default hook prints the payload); one installed hook writes at most one file, the first panic wins, and a later panic waits (sleeping, at most 2 s) for that file so `panic = "abort"` cannot cut it short; each `install_panic_hook` call has its own guard. The file is written as `crash-<stem>-<kind>.tmp` (created new) and hard-linked to its `.txt` name (`Session::begin` deletes stale tmp files); the hook never creates the folder.
- A crash file is never replaced. `write_crash_file` (used by `Session::begin` and the hook) and `NativeFaultWriter::record` (the filter's writer, T-024: no allocation, buffers and the `version`..`session_started` lines prepared at install from `render_crash_file`, the offset fixed at install, first call wins, bytes identical to `render_crash_file`; the filter returns `EXCEPTION_CONTINUE_SEARCH` and does not chain a previous filter) claims a name with `fs::hard_link`, which fails when the path exists (`linkat` on Linux, `CreateHardLinkW` on Windows), so two writers cannot both win a name and no `exists`-then-rename race is left. A taken name (an `abnormal_end` file and a panic of the same pid in the same second, either order) steps the name's second forward, at most 60 names, the pid kept; the `time:` line stays the true time. The name returned, and so `Crashed`/`EndedAbnormally { crash_file }` and its `previous session` line, is the name actually written; all 60 taken, or any other link error → `None`, no file left. The logs folder must support hard links (NTFS under `%LOCALAPPDATA%`; ext4/tmpfs in the Linux gate). Tests: `begin_never_replaces_a_crash_file_that_has_the_name_it_would_write`, `a_panic_in_the_second_of_begin_keeps_the_abnormal_end_file`, `begin_in_the_second_of_a_panic_keeps_the_panic_file`.
- Retention removes only regular `crash-*.txt` files (a file from the future has age zero; ties by name). The shell calls it at start (T-024); the log's retention never touches crash files.
- Tests: `tests/diag_crash.rs` (its own binary: the hook is process-global).

## Credentials prefix

```rust
// in voicen_core::secrets (T-061), value "Voicen/"
pub const CREDENTIAL_TARGET_PREFIX: &str = "Voicen/";
```

Lives in `voicen_core::secrets`, not in `diag`. Every `KeySlot::target_name` is built from it. Used by 004's credential store and by the shell's `--purge-credentials` (contracts/installer-ci.md); the purge policy is `voicen_core::secrets::purge_credentials`, the Win32 adapter `src-tauri/src/win/purge.rs` (`from_args`).
