# Contract: `voicen_core::diag` and `voicen_core::paths` (core API)

Platform-independent; tested on the Linux host with a temp directory and a fake clock. Used by the shell (`src-tauri`) and, through `PipelineObserver`, by features 001–005. Signatures are the contract; bodies are not.

**Updated by T-008 (2026-10-04).** The Clock, Log and Pipeline observer sections below are the signatures T-008 shipped; they replace the earlier draft (a chrono `WallClock`, `Log::open(&AppPaths, ..)`, `retain_now()`, `format_line(DateTime<FixedOffset>, ..)`, `LogsUnwritableReason::Other(i32)`, `dictation_finished(&DictationRecord)`, `HostName::from_url`). Session and Crash files are not built yet; their signatures are updated to the same time ports. Why: `docs/decisions/diagnostics-log.md`.

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
                           pub text_to_paste_ms: Option<u64> }
pub enum DictationOutcome { Delivered(DeliveryResult), Failed { failure: FailureTag, http_status: Option<u16> },
                            NoSpeech, TooShort }
impl EngineTag   { pub fn from_kind(kind: &str) -> EngineTag; }      // api | builtin | local_server | other
impl DetectorTag { pub fn from_name(name: &str) -> DetectorTag; }    // silero | energy | other
impl FailureTag  { pub fn from_code(code: &str) -> FailureTag; }     // one per FailureReason::code() | other

pub enum LogsUnwritableReason { PermissionDenied, DiskFull, NotADirectory, Other(Option<i32>) }  // raw OS code if any

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
- Windows, a reader that blocks the roll. The roll closes the log's own handle and then renames `voicen.log`. A rename needs every other open handle on the file to share delete. A reader that shares read and write but not delete (.NET `FileShare.ReadWrite`, as used by a tail such as `Get-Content -Wait` or a log viewer) makes the rename fail with a sharing violation (OS error 32). The log then goes Degraded, `on_unwritable` gets `Other(Some(32))`, and lines are dropped, in 60 s retry windows, until the reader closes. This follows from reading `log.rs` (T-008 review round 1 #4); it has not been observed on Windows. T-054 decides how its notice treats this case: as a "not writable" reason, or retried on the next write without degrading.

## Pipeline observer (seam with 001 — P-011)

```rust
// declared by 001 in events.rs; implemented here
pub struct LogObserver { /* Arc<Log>, open records */ }
impl LogObserver { pub fn new(log: Arc<Log>) -> LogObserver; }
impl PipelineObserver for LogObserver { fn event(&self, e: &DictationEvent); }
```

`LogObserver` aggregates the events of each recording into one `LogEvent::Dictation` line. It joins them by `RecordingId` (`JobFinished.recording`, added by T-008) and joins `Delivered` to its job by `seq`, never by event order. A record closes at `Delivered` for a text job, at `JobFinished` for no-speech or failed, and at `RecordingEnded{TooShort}` for a discarded hold. A pipeline `Warning` is its own line at once. Every match over `DictationEvent`, `OutcomeCode`, `RecordingEnd` and `WarningCode` is exhaustive, so a new variant does not compile until it has a log mapping. At most 64 open records and 64 text jobs awaiting `Delivered` are held. Past that the oldest is dropped without a line: its timings are lost, and its late `Delivered` writes nothing. Tests: `diag_observer.rs` (`interleaved_jobs_are_joined_by_recording_id`, `delivered_is_joined_to_its_job_by_seq`, `open_records_are_bounded_*`, `text_jobs_awaiting_delivery_are_bounded_*`), `diag_pipeline.rs` `log_holds_no_key_query_or_transcript_on_any_path`. 001 owns the trait and the events. This feature owns `LogObserver` and the line format. The release session (T-006 / T-051) must pass the same `Arc<LogObserver>` to `PipelineDeps.observer` and to its capture worker. There is no second observer and no no-op observer in release.

## Session

Not built yet (a later task). The time parameters are the ports of "Clock" above. `started` is the instant; its local text comes from diag's timestamp formatting with the same `LocalOffset`.

```rust
pub enum PreviousSession { Clean, Crashed{crash_file: FileName}, EndedByInstaller,
                           EndedAbnormally{crash_file: Option<FileName>} }

pub struct Session { /* marker path, started */ }
impl Session {
    /// Classifies the previous session (research R7), writes an abnormal-end crash file if needed,
    /// deletes the old marker and installer note, writes the new marker. Call only in the primary instance.
    pub fn begin(logs_dir: &Path, build: BuildInfo, os: OsVersion, pid: u32, clock: &dyn Clock,
                 offset: &dyn LocalOffset, log: &Log) -> (Session, PreviousSession);
    pub fn started(&self) -> SystemTime;
    pub fn end_clean(self, log: &Log);           // deletes the marker; logs Exited
}
```

## Crash files

Not built yet (a later task). These use the same time ports. The panic hook gets its own `Arc` clones of the `Clock` and `LocalOffset` the log uses.

```rust
pub enum CrashKind { Panic, NativeFault, AbnormalEnd }

pub struct CrashContext { logs_dir: PathBuf, build: BuildInfo, os: OsVersion, session_started: SystemTime,
                          pid: u32, clock: Arc<dyn Clock>, offset: Arc<dyn LocalOffset>,
                          modules: Arc<dyn ModuleResolver> }

pub trait ModuleResolver: Send + Sync { fn resolve(&self, ip: usize) -> Option<(String /*module file name*/, usize /*offset*/)>; }

pub fn install_panic_hook(ctx: CrashContext);        // std::panic::set_hook; ignores the payload
pub fn render_crash_file(fields: &CrashFields) -> String;   // pure; allowlisted keys only
pub fn retain_crash_files(logs_dir: &Path, now: SystemTime);   // ≤ 30 days, ≤ 20 files
pub const CRASH_MAX_AGE_DAYS: u32 = 30;
pub const CRASH_MAX_FILES: usize = 20;
```

Guarantees: the hook never formats the panic payload; never panics (re-entry guard); writes the file before returning (the process then aborts under `panic = "abort"`).

## Credentials prefix

```rust
pub const CREDENTIAL_TARGET_PREFIX: &str = /* value from 004's key-slot naming */;
```

Used by 004's credential store and by the shell's `--purge-credentials` (contracts/installer-ci.md).
