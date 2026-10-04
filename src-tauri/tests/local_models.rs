//! T-044: the local-model commands and events (specs/002 contracts/ipc.md
//! `local_models_list`, `local_model_download { id }`,
//! `local_model_cancel_download { id }`, `local-model://progress|state`) over the
//! shell's one construction path (`load_settings`) and app wiring (`build_app`),
//! through Tauri's mock runtime; the models-dir resolver (`paths::models_dir`) and
//! the Windows disk probe (`WinDiskSpace`). Windows CI only (decision #5).
//!
//! Each test builds the one `LocalModels` the way `run()` does (`LocalModels::open`)
//! over a `TempDir` models dir, passes its store to `load_settings` and the
//! coordinator to `build_app`, and serves the five catalog models from core's raw-TCP
//! mock server (`voicen_core::test_support::local_models`, the copy core's tests use;
//! P-010). The disk probe is core's `FakeDisk`, except in the `WinDiskSpace` test. The
//! CI runner's real `%LOCALAPPDATA%\Voicen` is never written.
//!
//! No refused-port or stall case here (F-004, F-005: a refused loopback connect
//! takes ~2.17 s on windows-latest); core's downloader tests cover those. Rust
//! listeners run inside the emit call, on the download thread, so the order seen in a
//! channel is the emit order. Fake data only: 127.0.0.1.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, Context, Listener, Manager, Url, WebviewWindow, WebviewWindowBuilder};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::local_models::catalog::{CatalogEntry, MODELS};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::service::LocalModels;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::local_models::{
    dir_entries, five_entries, model_bytes, FakeDisk, Serve, Server, NEEDED,
};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
#[cfg(windows)]
use voicen_lib::local_models::WinDiskSpace;
use voicen_lib::paths;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::{self, LABEL};

/// contracts/ipc.md event names, spelled out here (not taken from the code).
const PROGRESS: &str = "local-model://progress";
const STATE: &str = "local-model://state";
/// Long enough for any local end event; a hang fails the test instead of the run.
const END_WAIT: Duration = Duration::from_secs(10);
/// How long "nothing more happens" is watched.
const QUIET: Duration = Duration::from_millis(500);

const IDS: [&str; 5] = [
    "tiny",
    "base",
    "small",
    "medium-q5_0",
    "large-v3-turbo-q5_0",
];

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

/// Every server here accepts; connect is never the deadline that fires.
fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(5),
        download_no_data: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

/// Slow enough to see a download running and cancel it: 1 KiB every 50 ms (~3.2 s).
fn trickle() -> Serve {
    Serve::Trickle {
        chunk: 1024,
        every: Duration::from_millis(50),
    }
}

// ---- harness ------------------------------------------------------------------

/// The app as `run()` wires it, over a `TempDir` data dir, with a webview labelled
/// like the settings window.
struct World {
    _tmp: TempDir,
    /// `<data>/models`; created only by `prepare` or the first download.
    models_dir: PathBuf,
    server: Server,
    service: Arc<SettingsService>,
    app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

/// The five models served by a server with `plan`, plenty of disk, nothing on disk.
fn world(plan: Vec<Serve>) -> World {
    world_with(plan, FakeDisk::with_available(10 * NEEDED), |_| {})
}

/// `prepare` runs on the models dir path before `LocalModels::open`.
fn world_with(plan: Vec<Serve>, disk: Arc<FakeDisk>, prepare: impl FnOnce(&Path)) -> World {
    let tmp = TempDir::new();
    let data = tmp.path().to_path_buf();
    let models_dir = data.join("models");
    prepare(&models_dir);
    let server = Server::start(plan);
    let catalog: &'static [CatalogEntry] = five_entries(&server);
    let probe: Arc<dyn DiskSpace> = disk;
    let (models, cleanup) = LocalModels::open(models_dir.clone(), probe, timeouts(), catalog);
    if let Err(e) = cleanup {
        panic!("cleanup_at_start failed at open: {e}");
    }
    let models = Arc::new(models);
    let (service, _) = load_settings(data, no_keys(), no_autostart(), models.store(), None);
    let app = voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        service.clone(),
        models,
    )
    .expect("mock app builds");
    let webview = WebviewWindowBuilder::new(&app, LABEL, Default::default())
        .build()
        .expect("mock webview builds");
    World {
        _tmp: tmp,
        models_dir,
        server,
        service,
        app,
        webview,
    }
}

/// Invokes `cmd` from `webview` at `url`; `Ok` = resolved value, `Err` = rejection
/// (a command error is its serialized value; an ACL refusal is a string).
fn invoke_at(
    webview: &WebviewWindow<MockRuntime>,
    url: Url,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url,
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("response is JSON"))
}

impl World {
    /// Invokes `cmd` like the UI does.
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        invoke_at(&self.webview, url.parse().expect("url"), cmd, args)
    }

    #[track_caller]
    fn list(&self) -> Vec<Value> {
        match self.invoke("local_models_list", json!({})) {
            Ok(Value::Array(views)) => views,
            Ok(other) => panic!("local_models_list returned a non-array: {other}"),
            Err(e) => panic!("local_models_list rejected: {e}"),
        }
    }

    /// The listed `state` of `id`.
    #[track_caller]
    fn state_of(&self, id: &str) -> Value {
        self.list()
            .into_iter()
            .find(|v| v["id"] == json!(id))
            .unwrap_or_else(|| panic!("{id} not listed"))["state"]
            .clone()
    }

    fn download(&self, id: &str) -> Result<Value, Value> {
        self.invoke("local_model_download", json!({ "id": id }))
    }

    #[track_caller]
    fn cancel(&self, id: &str) -> Value {
        self.invoke("local_model_cancel_download", json!({ "id": id }))
            .unwrap_or_else(|e| panic!("local_model_cancel_download rejected: {e}"))
    }

    /// `settings_save` of `settings` with every key `Untouched`.
    #[track_caller]
    fn save(&self, settings: &Settings) -> Value {
        let args = json!({ "request": {
            "settings": settings,
            "keys": {
                "transcription_api": "Untouched",
                "local_server": "Untouched",
                "post_processing": "Untouched",
            },
        } });
        self.invoke("settings_save", args)
            .unwrap_or_else(|e| panic!("settings_save rejected: {e}"))
    }

    fn part_files(&self) -> Vec<String> {
        dir_entries(&self.models_dir)
            .into_iter()
            .filter(|n| n.ends_with(".part"))
            .collect()
    }
}

/// `(event name, payload)` of both local-model events, as a listener on `target`
/// sees them.
fn record<L: Listener<MockRuntime>>(target: &L) -> Receiver<(String, Value)> {
    let (tx, rx) = mpsc::channel();
    for name in [PROGRESS, STATE] {
        let tx = tx.clone();
        target.listen(name, move |event| {
            let payload: Value =
                serde_json::from_str(event.payload()).expect("event payload is JSON");
            let _ = tx.send((name.to_string(), payload));
        });
    }
    rx
}

/// Blocks until a `local-model://state`; returns the progress payloads before it
/// and the state payload.
#[track_caller]
fn wait_state(events: &Receiver<(String, Value)>) -> (Vec<Value>, Value) {
    let deadline = Instant::now() + END_WAIT;
    let mut progress = Vec::new();
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name, payload)) if name == STATE => return (progress, payload),
            Ok((_, payload)) => progress.push(payload),
            Err(RecvTimeoutError::Timeout) => {
                panic!("no {STATE} within {END_WAIT:?}; progress seen: {progress:?}")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("listener gone"),
        }
    }
}

/// Blocks until the first `local-model://progress`.
#[track_caller]
fn wait_progress(events: &Receiver<(String, Value)>) -> Value {
    match events.recv_timeout(END_WAIT) {
        Ok((name, payload)) => {
            assert_eq!(name, PROGRESS, "first event is not progress: {payload}");
            payload
        }
        Err(e) => panic!("no {PROGRESS} within {END_WAIT:?} ({e:?})"),
    }
}

/// Everything that arrives within `d`.
fn drain(events: &Receiver<(String, Value)>, d: Duration) -> Vec<(String, Value)> {
    let deadline = Instant::now() + d;
    let mut got = Vec::new();
    while let Ok(e) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        got.push(e);
    }
    got
}

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).expect("create models dir");
    std::fs::write(dir.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
}

/// `defaults(None)` with engine builtin_local and `model_id`.
fn builtin_local(model_id: &str) -> Settings {
    let mut settings = defaults(None);
    settings.engine = EngineKind::BuiltinLocal;
    settings.builtin_local.model_id = Some(model_id.to_string());
    settings
}

fn checksum_mismatch() -> Value {
    json!({ "code": "checksum_mismatch", "messageKey": "download.checksum_mismatch" })
}

// ---- local_models_list ----------------------------------------------------------

#[test]
fn local_models_list_reports_the_catalog_with_each_models_disk_state() {
    // Acceptance 1, first clause. Always five, catalog order, state from the disk
    // (a wrong-size file is not downloaded). Bite: the command not registered or
    // the coordinator not managed (rejection), a list from another store or the
    // production catalog (sizes), fields re-spelled in the shell.
    let w = world_with(vec![], FakeDisk::with_available(10 * NEEDED), |dir| {
        put(dir, "ggml-base.bin", &model_bytes());
        put(dir, "ggml-small.bin", &[1u8; 100]);
    });

    let list = w.list();

    let ids: Vec<Value> = list.iter().map(|v| v["id"].clone()).collect();
    assert_eq!(ids, IDS.map(|id| json!(id)).to_vec(), "catalog order");
    for view in &list {
        let expected = if view["id"] == json!("base") {
            json!({ "kind": "downloaded" })
        } else {
            json!({ "kind": "not_downloaded" })
        };
        assert_eq!(view["state"], expected, "{view}");
        assert_eq!(
            view["recommended"],
            json!(view["id"] == json!("small")),
            "{view}"
        );
    }
    assert_eq!(
        list[1],
        json!({
            "id": "base",
            "nameKey": "local_model.name.base",
            "sizeBytes": 65_600,
            "recommended": false,
            "state": { "kind": "downloaded" },
            "loaded": false,
        })
    );
    assert_eq!(w.server.accepts(), 0);
}

// ---- download: happy path --------------------------------------------------------

#[test]
fn download_through_ipc_emits_progress_then_downloaded_and_settings_accepts_it() {
    // Acceptance 1. One Arc<ModelStore> for the commands and the settings
    // validation: builtin_local with `base` is refused before and Saved after the
    // download. Bite: NoDownloadedModels left in load_settings (Refused after),
    // a second store, no progress event, an event from the command itself (taken
    // for the end), more than one end event.
    let w = world(vec![Serve::Full]);
    let events = record(&w.webview);

    let before = w.save(&builtin_local("base"));
    assert_eq!(
        before,
        json!({ "Refused": {
            "errors": [ { "field": "engine.builtin_local.model_id", "code": "model.not_downloaded" } ],
            "form_error": null,
        } }),
        "before the download"
    );

    let started = w
        .download("base")
        .unwrap_or_else(|e| panic!("local_model_download rejected: {e}"));
    assert_eq!(started, Value::Null);
    let (progress, end) = wait_state(&events);

    assert_eq!(
        end,
        json!({ "id": "base", "state": { "kind": "downloaded" } })
    );
    assert!(!progress.is_empty(), "no {PROGRESS} before the end");
    let mut last = 0;
    for p in &progress {
        assert_eq!(
            (&p["id"], &p["total"]),
            (&json!("base"), &json!(65_600)),
            "{p}"
        );
        let received = p["received"].as_u64().unwrap_or_else(|| panic!("{p}"));
        assert!(last <= received && received <= 65_600, "{p} after {last}");
        last = received;
    }
    let more = drain(&events, QUIET);
    assert!(more.is_empty(), "events after the end: {more:?}");

    assert_eq!(w.state_of("base"), json!({ "kind": "downloaded" }));
    assert_eq!(
        dir_entries(&w.models_dir),
        vec!["ggml-base.bin".to_string()]
    );

    let after = w.save(&builtin_local("base"));
    assert_eq!(
        after["Saved"]["view"]["settings"]["builtin_local"]["model_id"],
        json!("base"),
        "after the download: {after}"
    );
    assert_eq!(
        w.service.snapshot().builtin_local.model_id.as_deref(),
        Some("base")
    );
}

// ---- failure branch ----------------------------------------------------------------

#[test]
fn failed_downloads_leave_no_part_and_a_retry_finishes() {
    // Acceptance 2: a failed download leaves no `.part` and the model not
    // downloaded, listed as failed with its reason; Retry = download again. Bite:
    // the reason dropped from the event or the list, a `.part` left behind,
    // the failed state stuck (the retry refused), the HTTP status lost.
    let w = world(vec![Serve::Altered, Serve::Status(500), Serve::Full]);
    let events = record(&w.webview);

    let failed_checksum = json!({ "kind": "failed", "reason": checksum_mismatch() });
    let failed_500 = json!({
        "kind": "failed",
        "reason": { "code": "http_status", "messageKey": "download.http_status", "params": { "code": "500" } },
    });
    for expected in [failed_checksum, failed_500] {
        w.download("base")
            .unwrap_or_else(|e| panic!("local_model_download rejected: {e}"));
        let (_, end) = wait_state(&events);
        assert_eq!(end, json!({ "id": "base", "state": expected.clone() }));
        assert_eq!(w.part_files(), Vec::<String>::new(), "after {expected}");
        assert!(
            !w.models_dir.join("ggml-base.bin").exists(),
            "a final file after {expected}"
        );
        assert_eq!(w.state_of("base"), expected, "listed");
    }

    w.download("base")
        .unwrap_or_else(|e| panic!("retry rejected: {e}"));
    let (_, end) = wait_state(&events);
    assert_eq!(
        end,
        json!({ "id": "base", "state": { "kind": "downloaded" } })
    );
    assert_eq!(
        dir_entries(&w.models_dir),
        vec!["ggml-base.bin".to_string()]
    );
    assert_eq!(w.state_of("base"), json!({ "kind": "downloaded" }));
    assert_eq!(w.server.accepts(), 3);
}

#[test]
fn cancel_mid_download_leaves_no_part_and_the_model_not_downloaded() {
    // Acceptance 2: Cancel returns the model to not_downloaded with no reason (no
    // retry offer) and no `.part`; cancel answers true once. Bite: the cancel
    // command not reaching the downloader (the download finishes), Cancelled
    // shown as failed, a `.part` left, true for an idle or unknown id.
    let w = world(vec![trickle()]);
    let events = record(&w.webview);

    w.download("base")
        .unwrap_or_else(|e| panic!("local_model_download rejected: {e}"));
    let first = wait_progress(&events);
    assert_eq!(first["id"], json!("base"), "{first}");
    assert!(
        matches!(w.state_of("base")["kind"].as_str(), Some("downloading")),
        "listed while running: {}",
        w.state_of("base")
    );

    assert_eq!(w.cancel("base"), json!(true));
    let (_, end) = wait_state(&events);

    assert_eq!(
        end,
        json!({ "id": "base", "state": { "kind": "not_downloaded" } })
    );
    assert_eq!(dir_entries(&w.models_dir), Vec::<String>::new());
    assert_eq!(w.state_of("base"), json!({ "kind": "not_downloaded" }));
    assert_eq!(w.cancel("base"), json!(false), "a second cancel");
    assert_eq!(w.cancel("nope"), json!(false), "an unknown id");
}

#[test]
fn a_part_left_by_a_previous_run_is_deleted_at_open() {
    // Acceptance 2, last clause: a `.part` from an earlier run is deleted when the
    // one LocalModels is opened (on NTFS here). Bite: open without
    // cleanup_at_start, or a `.part` listed as downloaded.
    let w = world_with(vec![], FakeDisk::with_available(10 * NEEDED), |dir| {
        put(dir, "ggml-base.bin.part", &model_bytes()[..32_800]);
        put(dir, "ggml-tiny.bin.part", &model_bytes());
    });

    assert_eq!(dir_entries(&w.models_dir), Vec::<String>::new());
    for view in w.list() {
        assert_eq!(view["state"], json!({ "kind": "not_downloaded" }), "{view}");
    }
}

// ---- refusals ------------------------------------------------------------------------

#[test]
fn a_second_download_is_download_busy() {
    // contracts/ipc.md `download_busy`; one download at a time (R-2). A refusal
    // changes no listed state. Bite: a second download started, another code, the
    // refused model shown as downloading.
    let w = world(vec![trickle()]);
    let events = record(&w.webview);
    w.download("base")
        .unwrap_or_else(|e| panic!("local_model_download rejected: {e}"));
    wait_progress(&events);

    let busy = json!({ "code": "download_busy", "messageKey": "download.busy" });
    assert_eq!(w.download("tiny"), Err(busy.clone()), "another model");
    assert_eq!(w.download("base"), Err(busy), "the same model");
    assert_eq!(w.state_of("tiny"), json!({ "kind": "not_downloaded" }));
    assert_eq!(w.state_of("base")["kind"], json!("downloading"));

    assert_eq!(w.cancel("base"), json!(true));
    wait_state(&events);
    assert_eq!(w.server.accepts(), 1, "a refused download sent a request");
}

#[test]
fn not_enough_disk_space_is_refused_with_needed_and_sends_no_request() {
    // contracts/ipc.md `not_enough_disk_space{needed}` (size + 1 %, R-9), refused
    // before any request; no event. Bite: `needed` missing, a request sent, an
    // event emitted, the models dir created.
    let w = world_with(vec![], FakeDisk::with_available(NEEDED - 1), |_| {});
    let events = record(&w.webview);

    let got = w.download("base");

    assert_eq!(
        got,
        Err(json!({
            "code": "not_enough_disk_space",
            "messageKey": "download.not_enough_disk_space",
            "params": { "needed": "66256" },
        }))
    );
    let emitted = drain(&events, QUIET);
    assert!(emitted.is_empty(), "events after a refusal: {emitted:?}");
    assert_eq!(w.server.accepts(), 0, "a request was sent");
    assert!(!w.models_dir.exists(), "the models dir was created");
    assert_eq!(w.state_of("base"), json!({ "kind": "not_downloaded" }));
}

#[test]
fn an_unknown_id_is_not_in_catalog() {
    // T-044 wire code `not_in_catalog` for an id string ModelId::parse rejects.
    // Bite: a parse failure rejected with a deserialize text or another code, a
    // path-like id reaching the disk, a request sent.
    let w = world(vec![]);
    let not_in_catalog =
        json!({ "code": "not_in_catalog", "messageKey": "download.not_in_catalog" });

    for id in ["medium", "../ggml-base", ""] {
        assert_eq!(w.download(id), Err(not_in_catalog.clone()), "{id:?}");
    }
    assert_eq!(w.server.accepts(), 0);
    assert!(!w.models_dir.exists());
}

// ---- events: settings window only ------------------------------------------------------

#[test]
fn events_reach_the_settings_window_and_no_app_or_other_window_listener() {
    // contracts/ipc.md: the events go to the settings window (emit_to the
    // `settings` webview window), unlike settings://changed (app.emit). Bite:
    // app.emit / emit_to(Any) (the App-target listener sees them), emit_to a label
    // that also matches another window's listener.
    let w = world(vec![Serve::Full]);
    let other = WebviewWindowBuilder::new(&w.app, "other", Default::default())
        .build()
        .expect("mock webview builds");
    let settings = record(&w.webview);
    let app = record(&w.app);
    let elsewhere = record(&other);

    w.download("base")
        .unwrap_or_else(|e| panic!("local_model_download rejected: {e}"));
    let (_, end) = wait_state(&settings);
    assert_eq!(
        end,
        json!({ "id": "base", "state": { "kind": "downloaded" } })
    );

    let at_app = drain(&app, QUIET);
    assert!(at_app.is_empty(), "an App-target listener got: {at_app:?}");
    let at_other = drain(&elsewhere, QUIET);
    assert!(at_other.is_empty(), "the `other` window got: {at_other:?}");
}

// ---- tauri's real ACL ------------------------------------------------------------------

/// The release context (tauri.conf.json and the capabilities as tauri-build resolved
/// them into OUT_DIR), for the mock runtime.
fn release_context() -> Context<MockRuntime> {
    tauri::generate_context!(test = true)
}

#[test]
fn local_model_commands_pass_the_real_acl_from_the_settings_window() {
    // Decision #57 / T-049: app commands are not ACL-checked from a local origin
    // while the app defines no permissions; this pins that the three local-model
    // commands are reachable from the window `settings_window::open` made, under
    // the release context. Bite: an app ACL manifest (AppManifest::commands or
    // src-tauri/permissions/) that does not allow them for `settings` ("not
    // allowed"), a command not registered.
    let tmp = TempDir::new();
    let data = tmp.path().to_path_buf();
    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(u64::MAX);
    let (models, _) = LocalModels::open(data.join("models"), probe, timeouts(), MODELS);
    let models = Arc::new(models);
    let (service, _) = load_settings(data, no_keys(), no_autostart(), models.store(), None);
    let app = voicen_lib::build_app(mock_builder(), release_context(), service, models)
        .expect("mock app builds");
    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("open");
    let window = app
        .get_webview_window(LABEL)
        .unwrap_or_else(|| panic!("no `{LABEL}` window"));
    let url = window.url().expect("window url");

    let list = invoke_at(&window, url.clone(), "local_models_list", json!({}))
        .unwrap_or_else(|e| panic!("local_models_list refused for `{LABEL}`: {e}"));
    assert_eq!(list.as_array().map(Vec::len), Some(5), "{list}");

    let cancel = invoke_at(
        &window,
        url.clone(),
        "local_model_cancel_download",
        json!({ "id": "base" }),
    )
    .unwrap_or_else(|e| panic!("local_model_cancel_download refused for `{LABEL}`: {e}"));
    assert_eq!(cancel, json!(false));

    // A command error, not an ACL refusal: the call reached the command.
    let refused = invoke_at(
        &window,
        url,
        "local_model_download",
        json!({ "id": "medium" }),
    );
    assert_eq!(
        refused,
        Err(json!({ "code": "not_in_catalog", "messageKey": "download.not_in_catalog" }))
    );
}

// ---- models dir and disk probe ---------------------------------------------------------

#[test]
fn models_dir_is_the_data_dir_models() {
    // The one models-dir resolver (P-010): `data_dir()\models`. Bite: another
    // name, a dir not under the data dir (the uninstaller and FR-28 cover data_dir).
    assert_eq!(paths::models_dir(), paths::data_dir().join("models"));
}

#[cfg(windows)]
#[test]
fn win_disk_space_answers_for_an_existing_and_a_not_yet_created_dir() {
    // Investigation hypothesis 5: the models dir does not exist before the first
    // download, and a probe error lets the download proceed (R-9), so the probe
    // must answer for a missing dir (from its nearest existing ancestor) and must
    // not create it. Bite: GetDiskFreeSpaceExW on the missing dir itself (error),
    // a probe that creates the dir, 0 bytes.
    let tmp = TempDir::new();
    let missing = tmp.path().join("models").join("not-yet");

    let existing = WinDiskSpace.available_bytes(tmp.path());
    let not_yet = WinDiskSpace.available_bytes(&missing);

    assert!(matches!(&existing, Ok(n) if *n > 0), "{existing:?}");
    assert!(matches!(&not_yet, Ok(n) if *n > 0), "{not_yet:?}");
    assert!(
        !tmp.path().join("models").exists(),
        "the probe created the models dir"
    );
}
