# Contract: `voicen_core::diag` and `voicen_core::paths` (core API)

Platform-independent; tested on the Linux host with a temp directory and a fake clock. Used by the shell (`src-tauri`) and, through `PipelineObserver`, by features 001–005. Signatures are the contract; bodies are not.

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

## Clock

```rust
pub trait WallClock: Send + Sync {
    fn now(&self) -> chrono::DateTime<chrono::FixedOffset>;   // local time with offset
}
```

Merged with 001's `platform::Clock` if that already yields wall time (research R2).

## Log

```rust
pub enum LogEvent { Started{..}, PreviousSession{..}, Dictation{..}, Error{..}, Warning{..},
                    LogsRecovered, CrashFileWritten{..}, RetentionDeleted{..}, Exited{..} }   // data-model.md

pub enum LogsUnwritableReason { PermissionDenied, DiskFull, NotADirectory, Other(i32) }

pub struct LogConfig { pub roll_bytes: u64 /*2 MiB*/, pub total_bytes: u64 /*10 MiB*/, pub max_age: Duration /*7 d*/,
                       pub reopen_every: Duration /*60 s*/ }

pub struct Log { /* Mutex<Writer> */ }
impl Log {
    pub fn open(paths: &AppPaths, clock: Arc<dyn WallClock>, cfg: LogConfig,
                on_unwritable: Box<dyn Fn(LogsUnwritableReason) + Send + Sync>) -> Self; // never fails
    pub fn write(&self, event: LogEvent);        // never panics, never blocks on I/O errors
    pub fn retain_now(&self);                    // called at start; also after each roll
}

pub fn format_line(ts: DateTime<FixedOffset>, event: &LogEvent) -> String;   // pure; one line, no '\n' inside
```

Guarantees (each pinned by a test):
- `format_line(Started)` contains `voicen <version> (<commit>) started` (CI greps `(<commit>) started`).
- No `LogEvent` constructor accepts `&str`/`String` for transcript, prompt, body, header, URL or error text; `HostName::from_url` keeps only the host.
- `write` from N threads produces N complete lines (no interleaving).
- `on_unwritable` is called at most once per `Log` instance.
- Retention never deletes `voicen.log`, `crash-*.txt`, `session.marker`, `installer-ended`.

## Pipeline observer (seam with 001 — P-011)

```rust
// declared by 001 in events.rs; implemented here
impl PipelineObserver for LogObserver { fn dictation_finished(&self, record: &DictationRecord); /* … */ }
```

`LogObserver` maps each callback to exactly one `LogEvent`. 001 owns the trait; this feature owns `LogObserver` and `DictationRecord`'s log formatting.

## Session

```rust
pub enum PreviousSession { Clean, Crashed{crash_file: FileName}, EndedByInstaller,
                           EndedAbnormally{crash_file: Option<FileName>} }

pub struct Session { /* marker path, started */ }
impl Session {
    /// Classifies the previous session (research R7), writes an abnormal-end crash file if needed,
    /// deletes the old marker and installer note, writes the new marker. Call only in the primary instance.
    pub fn begin(paths: &AppPaths, build: BuildInfo, os: OsVersion, pid: u32, clock: &dyn WallClock,
                 log: &Log) -> (Session, PreviousSession);
    pub fn started(&self) -> DateTime<FixedOffset>;
    pub fn end_clean(self, log: &Log);           // deletes the marker; logs Exited
}
```

## Crash files

```rust
pub enum CrashKind { Panic, NativeFault, AbnormalEnd }

pub struct CrashContext { logs_dir: PathBuf, build: BuildInfo, os: OsVersion, session_started: DateTime<FixedOffset>,
                          pid: u32, clock: Arc<dyn WallClock>, modules: Arc<dyn ModuleResolver> }

pub trait ModuleResolver: Send + Sync { fn resolve(&self, ip: usize) -> Option<(String /*module file name*/, usize /*offset*/)>; }

pub fn install_panic_hook(ctx: CrashContext);        // std::panic::set_hook; ignores the payload
pub fn render_crash_file(fields: &CrashFields) -> String;   // pure; allowlisted keys only
pub fn retain_crash_files(logs_dir: &Path, now: DateTime<FixedOffset>);   // ≤ 30 days, ≤ 20 files
pub const CRASH_MAX_AGE_DAYS: u32 = 30;
pub const CRASH_MAX_FILES: usize = 20;
```

Guarantees: the hook never formats the panic payload; never panics (re-entry guard); writes the file before returning (the process then aborts under `panic = "abort"`).

## Credentials prefix

```rust
pub const CREDENTIAL_TARGET_PREFIX: &str = /* value from 004's key-slot naming */;
```

Used by 004's credential store and by the shell's `--purge-credentials` (contracts/installer-ci.md).
