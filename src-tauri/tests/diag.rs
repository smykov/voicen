//! T-008: the shell start of the local log (FR-20, FR-18; spec 006 US2) over the
//! shell's one construction path and app wiring, through Tauri's mock runtime.
//! Windows CI only (decision #5).
//!
//! - `diag::start` opens the one log and writes `Started` as the first line.
//! - `load_settings` (the path `run()` and the tests share, J4) then writes the
//!   settings load line and the autostart reconcile line (spec 004 R-11), so the log
//!   of a start reads: Started, load, reconcile.
//! - `build_app` manages the log; `settings_save` writes one save line per save,
//!   field ids and codes only (decision #30: no base URL or query).
//! - An unwritable logs dir stops nothing: `start` returns, one `on_unwritable`
//!   call, the settings load and save work.
//!
//! That `run()` passes `paths::log_dir()` (= `%LOCALAPPDATA%\Voicen\logs`) is the
//! resolver test `settings_ipc::data_dir_is_localappdata_voicen_and_logs_live_under_it`
//! (read only) plus the install smoke, which reads the start line from that
//! folder. Each test here uses its own `TempDir`; the runner's real
//! `%LOCALAPPDATA%\Voicen` is never written. Keys and hosts are fake.
//!
//! T-054 (the one-time `notice.logs_unwritable`, analysis red tests 5 and 6):
//! - a logs folder with a deny-write ACE: the start goes on, the latch
//!   (`voicen_core::diag::LogsNotice`, managed by the test as `run()`'s `wire` does)
//!   is attached by `start_dictation`, a dictation is delivered, and exactly one
//!   `Message{notice.logs_unwritable, reason = logs_reason.permission_denied}`
//!   reaches the session's indicator;
//! - a roll blocked by a reader that does not share delete degrades the log with one
//!   callback; the test pins the reason Windows gives, `Other(Some(32))` (a
//!   sharing violation, no `ErrorKind` in std 1.99), measured by CI run
//!   38234346755 (docs/decisions/diagnostics-log.md § Windows notes).
//!
//! The ACE is set and removed with `icacls`, the only external process here, bounded
//! and observable by the F-006 rule (`run_icacls`).

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::{Log, LogsUnwritableReason, OnUnwritable};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::settings_ipc::load_settings;

#[cfg(windows)]
mod win32_support;

/// Obviously fake; must never reach the log.
const KEY: &str = "sk-test-SHELL-LOG-0000";
const QUERY_SECRET: &str = "SECRETQ";

/// What `on_unwritable` received.
#[derive(Default)]
struct Calls(Mutex<Vec<LogsUnwritableReason>>);

impl Calls {
    fn reasons(&self) -> Vec<LogsUnwritableReason> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn recorder() -> (Arc<Calls>, OnUnwritable) {
    let calls = Arc::new(Calls::default());
    let sink = Arc::clone(&calls);
    let cb: OnUnwritable = Box::new(move |reason| {
        sink.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(reason);
    });
    (calls, cb)
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

/// T-044: the store `load_settings` takes; nothing is downloaded here.
fn idle_store(data_dir: &Path) -> Arc<ModelStore> {
    Arc::new(ModelStore::new(data_dir.join("models"), MODELS))
}

/// T-044: the one `LocalModels` that `build_app` manages; its models dir lies under
/// a throwaway dir that is removed at once, so it is never created.
fn idle_models() -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        TempDir::new().path().join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// The lines of `logs/voicen.log`.
fn log_lines(logs: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(logs.join("voicen.log"))
        .unwrap_or_else(|e| panic!("read {}: {e}", logs.join("voicen.log").display()));
    text.lines().map(str::to_string).collect()
}

/// The message of a line: what follows `<timestamp> <LEVEL> `.
fn message(line: &str) -> String {
    line.splitn(3, ' ')
        .nth(2)
        .unwrap_or_else(|| panic!("not `<ts> <LEVEL> <message>`: {line:?}"))
        .to_string()
}

struct Harness {
    _app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

/// The app as `run()` builds it (`build_app`), with `log` managed.
fn harness(service: &Arc<SettingsService>, log: Arc<Log>) -> Harness {
    let app = voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        service.clone(),
        idle_models(),
        log,
    )
    .expect("mock app builds");
    let webview = WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .expect("mock webview builds");
    Harness { _app: app, webview }
}

impl Harness {
    /// `settings_save` like the UI: `settings`, the API key slot as given, the
    /// other slots `Untouched`.
    #[track_caller]
    fn save(&self, settings: &Settings, api_key: Value) -> Value {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        let args = json!({ "request": {
            "settings": settings,
            "keys": {
                "transcription_api": api_key,
                "local_server": "Untouched",
                "post_processing": "Untouched",
            },
        } });
        get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: "settings_save".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().expect("url"),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().expect("response is JSON"))
        .unwrap_or_else(|e| panic!("settings_save rejected: {e:?}"))
    }
}

fn edited(history_size: u32) -> Settings {
    let mut settings = defaults(None);
    settings.history.size = history_size;
    settings
}

#[test]
fn start_writes_started_first_then_the_load_and_reconcile_lines() {
    // FR-18 / spec 006 FR-003 (the first line of a session is the start line) and
    // R-11 (the load outcome and the reconcile action, which load_settings
    // discarded before T-008). A first run with a leftover Run value: FirstRun,
    // then `removed`. Bite: Started not written by start, written after the
    // settings lines, the load or reconcile line missing, the reconcile action
    // dropped again, a stderr line instead of a log line.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    let leftover: Arc<dyn Autostart> = Arc::new(FakeAutostart::enabled());
    let (_service, outcome) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        leftover,
        idle_store(dir.path()),
        None,
        &log,
    );
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));

    let messages: Vec<String> = log_lines(&logs).iter().map(|l| message(l)).collect();
    let info = voicen_core::build_info();
    // T-105: the start line carries the OS version from the shell's one source,
    // RtlGetVersion. On the Windows runner it is readable, so the expected value
    // is never the placeholder (a start writing OsVersion::UNKNOWN is caught).
    let os = voicen_lib::diag::os_version();
    assert!(os.major >= 10 && os.build > 0, "os_version() = {os}");
    assert_eq!(
        messages,
        vec![
            format!("voicen {info} started pid={} os={os}", std::process::id()),
            "settings load outcome=first_run".to_string(),
            "autostart reconcile action=removed".to_string(),
        ]
    );
    assert_eq!(calls.reasons(), vec![]);
}

#[test]
fn an_unwritable_logs_dir_does_not_stop_the_start() {
    // Acceptance failure branch: the logs folder cannot be written (here a file at
    // its path; the ACL-denied folder is T-054's) -> the app keeps working and the
    // log reports it once. Bite: start panicking or never returning, a callback per
    // write, the settings load or save failing because of the log, the file at the
    // logs path overwritten.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    std::fs::write(&logs, b"not a directory\n").expect("plant a file at the logs path");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    assert_eq!(calls.reasons(), vec![LogsUnwritableReason::NotADirectory]);

    let (service, outcome) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        Arc::new(FakeAutostart::new()),
        idle_store(dir.path()),
        None,
        &log,
    );
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));
    assert!(
        dir.path().join(SETTINGS_FILE).is_file(),
        "settings.json written"
    );
    let h = harness(&service, Arc::clone(&log));
    let saved = h.save(&edited(42), json!("Untouched"));
    assert_eq!(
        saved["Saved"]["view"]["settings"]["history"]["size"],
        json!(42),
        "{saved}"
    );

    assert_eq!(
        calls.reasons(),
        vec![LogsUnwritableReason::NotADirectory],
        "still one callback"
    );
    assert_eq!(
        std::fs::read(&logs).expect("still a file"),
        b"not a directory\n"
    );
}

#[test]
fn settings_save_writes_one_line_per_save_without_values() {
    // T-008 seam: settings_save writes the save line (spec 004 T035 / R-11):
    // outcome, warnings and errors as field ids and codes; decision #30: the base
    // URL's query string (and the URL, model and key) never reach the log. Bite: no
    // save line, the SaveOutcome or the request formatted into the log, a line only
    // for Saved.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    let (service, _) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        Arc::new(FakeAutostart::new()),
        idle_store(dir.path()),
        None,
        &log,
    );
    let h = harness(&service, Arc::clone(&log));

    let mut api = defaults(None);
    api.engine = EngineKind::Api;
    api.api.base_url = format!("http://192.0.2.10:8000/v1?api-version={QUERY_SECRET}");
    api.api.model = "MODEL-MARKER".to_string();
    let saved = h.save(&api, json!({ "Replace": KEY }));
    assert!(saved["Saved"].is_object(), "expected Saved: {saved}");
    let refused = h.save(&edited(0), json!("Untouched"));
    assert!(
        refused["Refused"].is_object(),
        "expected Refused: {refused}"
    );

    let lines = log_lines(&logs);
    let saves: Vec<String> = lines
        .iter()
        .map(|l| message(l))
        .filter(|m| m.starts_with("settings save "))
        .collect();
    assert_eq!(
        saves,
        vec![
            "settings save outcome=ok warnings=engine.api.base_url:endpoint.insecure".to_string(),
            "settings save outcome=refused errors=history.size:history.size_range".to_string(),
        ],
        "{lines:#?}"
    );
    let text = lines.join("\n");
    for value in [KEY, QUERY_SECRET, "192.0.2.10", "MODEL-MARKER"] {
        assert!(!text.contains(value), "{value} in the log:\n{text}");
    }
    assert_eq!(calls.reasons(), vec![]);
}

// ---- T-054: the one-time logs notice on Windows ------------------------------------

/// `icacls`, run by the F-006 rule: null stdin, `CREATE_NO_WINDOW`, one run at a time
/// (`ICACLS_TURN`), stdout and stderr read on threads, killed and waited for at
/// `ICACLS_BUDGET`; a stall is reported with the elapsed time and the output so far.
#[cfg(windows)]
mod acl {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use windows::Win32::System::Threading::CREATE_NO_WINDOW;

    /// One `icacls` at a time in this binary.
    static ICACLS_TURN: Mutex<()> = Mutex::new(());
    const ICACLS_BUDGET: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(50);
    const READER_GRACE: Duration = Duration::from_secs(5);
    /// The well-known SID of Everyone: the deny ACE applies to the runner's token
    /// whatever its user name (an administrator too: std opens without backup
    /// semantics).
    const EVERYONE: &str = "*S-1-1-0";

    type Reader = (Arc<Mutex<Vec<u8>>>, JoinHandle<()>);

    fn spawn_reader<R: Read + Send + 'static>(mut source: R) -> Reader {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buffer);
        let handle = thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match source.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(&chunk[..n]),
                }
            }
        });
        (buffer, handle)
    }

    fn drain(readers: [Reader; 2]) -> (String, String) {
        let deadline = Instant::now() + READER_GRACE;
        while readers.iter().any(|(_, h)| !h.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let [out, err] = readers.map(|(buffer, handle)| {
            if handle.is_finished() {
                let _ = handle.join();
            }
            let bytes = buffer.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&bytes).into_owned()
        });
        (out, err)
    }

    /// Runs `icacls <args>`; `Err` names a spawn failure, a non-zero exit or a stall,
    /// with what the child wrote.
    pub fn run_icacls(args: &[&str]) -> Result<(), String> {
        let _turn = ICACLS_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        let started = Instant::now();
        let mut child = Command::new("icacls")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map_err(|e| format!("icacls could not be started: {e}"))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("icacls started without piped stdout and stderr".into());
        };
        let readers = [spawn_reader(stdout), spawn_reader(stderr)];
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < ICACLS_BUDGET => thread::sleep(POLL),
                Ok(None) | Err(_) => break None,
            }
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            let elapsed = started.elapsed();
            let (out, err) = drain(readers);
            return Err(format!(
                "icacls {args:?} stalled: killed after {elapsed:?} (budget {ICACLS_BUDGET:?}); stdout {out:?}; stderr {err:?}"
            ));
        };
        let (out, err) = drain(readers);
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "icacls {args:?} exited with {status}; stdout {out:?}; stderr {err:?}"
            ))
        }
    }

    /// A deny-write ACE for Everyone on a folder, inherited by what is created in it;
    /// removed on drop (before the `TempDir` that holds the folder is removed: declare
    /// the guard after the `TempDir`).
    pub struct DenyWrite(PathBuf);

    impl DenyWrite {
        #[track_caller]
        pub fn on(dir: &Path) -> DenyWrite {
            let path = dir.to_str().expect("a UTF-8 temp path");
            run_icacls(&[path, "/deny", &format!("{EVERYONE}:(OI)(CI)(W)")])
                .unwrap_or_else(|e| panic!("precondition: set the deny-write ACE: {e}"));
            DenyWrite(dir.to_path_buf())
        }
    }

    impl Drop for DenyWrite {
        fn drop(&mut self) {
            let path = self.0.to_string_lossy().into_owned();
            if let Err(e) = run_icacls(&[&path, "/remove:d", EVERYONE]) {
                // Not a panic in drop: the temp dir is left behind, and said so.
                eprintln!("could not remove the deny ACE of {path}: {e}");
            }
        }
    }
}

/// A `LogsNoticeSink` that records what it was given.
#[cfg(windows)]
#[derive(Default)]
struct Notices(Mutex<Vec<LogsUnwritableReason>>);

#[cfg(windows)]
impl voicen_core::diag::LogsNoticeSink for Notices {
    fn logs_unwritable(&self, reason: LogsUnwritableReason) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(reason);
    }
}

#[cfg(windows)]
impl Notices {
    fn reasons(&self) -> Vec<LogsUnwritableReason> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The logs notices (`notice.logs_unwritable`, any reason) an indicator was shown.
#[cfg(windows)]
fn logs_notices(overlays: &[voicen_core::recording::OverlayState]) -> Vec<win32_support::Shown> {
    win32_support::shown(overlays)
        .into_iter()
        .filter(|s| {
            matches!(
                s,
                win32_support::Shown::Message("notice.logs_unwritable", _)
            )
        })
        .collect()
}

#[cfg(windows)]
#[test]
fn a_write_denied_logs_folder_shows_one_notice_and_the_dictation_still_delivers() {
    // T-054 Acceptance failure branch "the log fails before the dictation session
    // exists (at start)", on a real ACL (analysis red test 5): a logs folder that
    // denies writing. As `run()`: the latch's callback goes to `diag::start`, the
    // app is built, the latch is managed (`wire` manages the one `release_parts`
    // made), `start_dictation` attaches the session. Then: the start returns with
    // reason PermissionDenied, a dictation through the session's inputs reaches the
    // clipboard, and the session's indicator shows exactly one
    // Message{notice.logs_unwritable, reason = logs_reason.permission_denied},
    // across the start, the dictation and further writes; no file is created in the
    // folder. Bite: `start_dictation` not attaching the managed latch (no notice),
    // the latch dropping the pre-attach reason, a notice per write, the log's
    // failure stopping the start or the dictation.
    use std::time::{Duration, Instant};

    use tauri::Manager;
    use voicen_core::diag::LogsNotice;
    use voicen_core::dictation::DictationSession;
    use voicen_core::platform::{
        FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster, FrameChunk,
    };
    use voicen_core::settings::EngineKind;
    use voicen_core::test_support::fixtures;
    use voicen_lib::dictation::{start_dictation, DictationPorts};
    use voicen_lib::win::hotkey::HotkeyRegistrarHandle;
    use win32_support::{
        assert_hotkey_free, engine_factory, eventually, keyed_store, serial, settings_with, Shown,
        TextEngine, BUDGET, HOLD, SETTLE,
    };

    const TEXT: &str = "logs notice test text";
    let _serial = serial();
    assert_hotkey_free();
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).expect("logs folder");
    let _deny = acl::DenyWrite::on(&logs);
    let probe = std::fs::File::create(logs.join("probe.tmp")).map_err(|e| e.kind());
    assert_eq!(
        probe.map(drop),
        Err(std::io::ErrorKind::PermissionDenied),
        "precondition: the deny-write ACE refuses a new file in the logs folder"
    );

    let notice = Arc::new(LogsNotice::new());
    let latch = notice.callback();
    let (calls, record) = recorder();
    let log = voicen_lib::diag::start(
        logs.clone(),
        Box::new(move |reason| {
            record(reason);
            latch(reason);
        }),
    );
    assert_eq!(
        calls.reasons(),
        vec![LogsUnwritableReason::PermissionDenied]
    );

    let creds = keyed_store();
    let hotkeys = HotkeyRegistrarHandle::new();
    let service = settings_with(
        dir.path(),
        &creds,
        &log,
        EngineKind::Api,
        Arc::new(FakeAutostart::new()),
        &hotkeys,
    );
    let app = voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        Arc::clone(&service),
        idle_models(),
        Arc::clone(&log),
    )
    .expect("mock app builds");
    assert!(
        app.manage(Arc::clone(&notice)),
        "build_app manages no LogsNotice of its own (run()'s wire manages the one of release_parts)"
    );
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_chunks(vec![FrameChunk::from_buffer(
        &fixtures::speech_3s(),
        Instant::now() + Duration::from_millis(10),
    )]);
    let clipboard = Arc::new(FakeClipboard::new());
    let indicator = Arc::new(FakeIndicator::new());
    let engine = TextEngine::new(TEXT);
    let _dictation = start_dictation(
        app.handle(),
        DictationPorts {
            audio,
            clipboard: clipboard.clone(),
            paster: Arc::new(FakePaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&creds),
            hotkeys: Arc::clone(&hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
        },
    )
    .expect("start_dictation");
    assert_eq!(
        logs_notices(&indicator.overlays()),
        vec![Shown::Message(
            "notice.logs_unwritable",
            vec![("reason", "logs_reason.permission_denied".to_string())]
        )],
        "start_dictation did not attach the managed latch (or the reason was lost): {:?}",
        indicator.overlays()
    );

    let session = Arc::clone(&app.state::<Arc<DictationSession>>());
    session.hotkey_pressed(Instant::now());
    std::thread::sleep(HOLD);
    session.hotkey_released(Instant::now());
    assert!(
        eventually(BUDGET, || clipboard.texts() == vec![TEXT.to_string()]),
        "the dictation was not delivered within {BUDGET:?}: clipboard {:?}",
        clipboard.texts()
    );
    for _ in 0..3 {
        log.write(voicen_core::diag::LogEvent::Warning {
            kind: voicen_core::diag::WarningKind::TrayFailed,
            os_code: None,
        });
    }
    std::thread::sleep(SETTLE);
    assert_eq!(
        logs_notices(&indicator.overlays()).len(),
        1,
        "{:?}",
        indicator.overlays()
    );
    assert_eq!(
        calls.reasons(),
        vec![LogsUnwritableReason::PermissionDenied]
    );
    let created: Vec<_> = std::fs::read_dir(&logs)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        created.is_empty(),
        "files created in the denied folder: {created:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_roll_blocked_by_a_reader_without_share_delete_degrades_with_one_notice() {
    // T-054 Acceptance failure branch "a second time in the same session (a roll
    // blocked by a reader that does not share delete, or a failure after
    // LogsRecovered)" (analysis red test 6, Notes item "Windows error 32"): a Log
    // with roll_bytes 512 and a reader of voicen.log opened with share
    // READ|WRITE (no DELETE). The roll's rename fails: the log degrades (the
    // active file never grows past roll_bytes, no rolled file), the latch's sink
    // gets one reason, the one Windows gives (pinned: Other(Some(32)), the sharing
    // violation by std 1.99's mapping, measured by CI run 38234346755; a change
    // of it changes the pin and docs/decisions/diagnostics-log.md together). With the
    // reader closed and 61 s later (fake clock) the log recovers and rolls; a
    // second blocked roll gives no second notice. Bite: appending past roll_bytes
    // when the rename fails (10 MiB bound), a callback per failed roll, the reason
    // mapped differently than measured, the latch not delivering after the attach.
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use voicen_core::clock::{FakeClock, LocalOffset};
    use voicen_core::diag::{LogConfig, LogEvent, LogsNotice, WarningKind};
    use win32_support::{eventually, BUDGET, SETTLE};

    /// FILE_SHARE_READ | FILE_SHARE_WRITE: others may read and write, not delete
    /// or rename.
    const SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    const ROLL: u64 = 512;
    struct Utc;
    impl LocalOffset for Utc {
        fn seconds_east(&self, _: SystemTime) -> i32 {
            0
        }
    }

    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let start = UNIX_EPOCH + Duration::from_secs(1_791_115_200);
    let clock = Arc::new(FakeClock::at(start));
    let notice = Arc::new(LogsNotice::new());
    let seen = Arc::new(Notices::default());
    notice.attach(Arc::clone(&seen) as Arc<dyn voicen_core::diag::LogsNoticeSink>);
    let log = Log::open(
        logs.clone(),
        clock.clone(),
        Arc::new(Utc),
        LogConfig {
            roll_bytes: ROLL,
            total_bytes: 8 * ROLL,
            ..LogConfig::default()
        },
        notice.callback(),
    );
    let line = || LogEvent::Warning {
        kind: WarningKind::TrayFailed,
        os_code: None,
    };
    let active = logs.join("voicen.log");
    let rolled = || -> Vec<String> {
        std::fs::read_dir(&logs)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("voicen-"))
                    .collect()
            })
            .unwrap_or_default()
    };
    log.write(line());
    assert!(active.is_file(), "premise: the log opened healthy");
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(&active)
        .expect("open voicen.log without FILE_SHARE_DELETE");

    for _ in 0..64 {
        log.write(line());
    }
    assert!(
        eventually(BUDGET, || !seen.reasons().is_empty()),
        "no notice within {BUDGET:?} for the blocked roll"
    );
    let len = std::fs::metadata(&active).expect("voicen.log").len();
    assert!(len <= ROLL, "the active file grew past roll_bytes: {len}");
    assert_eq!(
        rolled(),
        Vec::<String>::new(),
        "the blocked roll left a rolled file"
    );
    assert_eq!(
        seen.reasons(),
        vec![LogsUnwritableReason::Other(Some(32))],
        "the reason Windows gives for a rename blocked by a reader without FILE_SHARE_DELETE"
    );

    drop(reader);
    clock.set(start + Duration::from_secs(61));
    for _ in 0..16 {
        log.write(line());
    }
    assert!(
        !rolled().is_empty(),
        "premise: with the reader closed the log recovered and rolled"
    );
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(&active)
        .expect("open voicen.log again without FILE_SHARE_DELETE");
    let rolled_before = rolled().len();
    for _ in 0..64 {
        log.write(line());
    }
    std::thread::sleep(SETTLE);
    assert_eq!(
        rolled().len(),
        rolled_before,
        "premise: the second roll was blocked"
    );
    assert_eq!(
        seen.reasons().len(),
        1,
        "a second notice in the same session: {:?}",
        seen.reasons()
    );
    drop(reader);
}
