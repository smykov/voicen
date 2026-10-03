//! T-030: the settings IPC (contracts/ipc.md) over the shell's one construction path
//! (`load_settings`) and the one command registration (`commands`), through Tauri's
//! mock runtime (`tauri::test`). Windows CI only (decision #5).
//!
//! Each test gets its own `TempDir` as the data dir; the CI runner's real
//! `%LOCALAPPDATA%\Voicen` is never written (it must stay clean for the install
//! smoke). Keys are obviously fake.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, Listener, WebviewWindow, WebviewWindowBuilder};
use voicen_core::i18n::UiLanguage;
use voicen_core::secrets::{
    CredentialCall, CredentialError, CredentialOp, CredentialStore, FakeCredentialStore, KeyEdits,
    KeySlot,
};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings, WHISPER_ISO_639_1};
use voicen_core::test_support::TempDir;
use voicen_lib::settings_ipc::{load_settings, spawn_change_bridge};

/// Obviously fake key; must never leave the credential store.
const CANARY: &str = "sk-test-CANARY-7f3a-not-a-real-key";
const CHANGED: &str = "settings://changed";
/// How long "no event" is waited for.
const QUIET: Duration = Duration::from_secs(1);
/// How long an expected event is waited for.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The mock app with the release command set and the service managed, and a
/// webview labelled like the settings window.
struct Harness {
    app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

fn harness(service: &Arc<SettingsService>) -> Harness {
    let app = voicen_lib::commands(mock_builder())
        .manage(service.clone())
        .build(mock_context(noop_assets()))
        .expect("mock app builds");
    let webview = WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .expect("mock webview builds");
    Harness { app, webview }
}

impl Harness {
    /// Invokes `cmd` like the UI does; `Ok` = resolved value, `Err` = rejection.
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().expect("url"),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().expect("response is JSON"))
    }

    /// `settings_save` with `settings` and one wire `KeyEdit` per slot; the
    /// rejection (e.g. invalid args) is a test failure.
    #[track_caller]
    fn save(&self, settings: &Settings, keys: [Value; 3]) -> Value {
        let [api, local, pp] = keys;
        let args = json!({ "request": {
            "settings": settings,
            "keys": { "transcription_api": api, "local_server": local, "post_processing": pp },
        } });
        self.invoke("settings_save", args)
            .unwrap_or_else(|e| panic!("settings_save rejected: {e}"))
    }

    /// Every payload of `settings://changed`, as JSON, in a channel.
    fn changed_events(&self) -> mpsc::Receiver<Value> {
        let (tx, rx) = mpsc::channel();
        self.app.listen_any(CHANGED, move |event| {
            let payload = serde_json::from_str(event.payload()).expect("event payload is JSON");
            let _ = tx.send(payload);
        });
        rx
    }
}

fn untouched() -> Value {
    json!("Untouched")
}

fn replace(key: &str) -> Value {
    json!({ "Replace": key })
}

fn view_json(service: &SettingsService) -> Value {
    serde_json::to_value(service.view()).expect("view serializes")
}

/// Valid for engine `none` (the first-run engine); differs from the defaults.
fn edited(history_size: u32) -> Settings {
    let mut settings = defaults(None);
    settings.history.size = history_size;
    settings
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list data dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("list dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn read_settings_file(dir: &Path) -> Settings {
    let bytes = std::fs::read(dir.join(SETTINGS_FILE)).expect("settings.json exists");
    serde_json::from_slice(&bytes).expect("settings.json parses")
}

fn fake_store(store: FakeCredentialStore) -> Arc<FakeCredentialStore> {
    Arc::new(store)
}

fn as_port(store: &Arc<FakeCredentialStore>) -> Arc<dyn CredentialStore> {
    store.clone()
}

#[test]
fn get_returns_presence_only() {
    // Bite: settings_get not returning the service's view, a key value (instead of
    // a boolean) in the payload, or presence read from somewhere else.
    let dir = TempDir::new();
    let store = fake_store(
        FakeCredentialStore::new()
            .with_key(KeySlot::TranscriptionApi, CANARY)
            .with_key(KeySlot::PostProcessing, CANARY),
    );
    let (service, outcome) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));
    let h = harness(&service);

    let view = h
        .invoke("settings_get", json!({}))
        .unwrap_or_else(|e| panic!("settings_get rejected: {e}"));

    assert_eq!(view, view_json(&service));
    assert_eq!(
        view["keys"],
        json!({ "transcription_api": true, "local_server": false, "post_processing": true })
    );
    assert_eq!(view["first_run"], json!(true));
    assert_eq!(view["reset_notice"], json!(false));
    assert_eq!(
        view["settings"],
        serde_json::to_value(defaults(None)).expect("json")
    );
    let text = view.to_string();
    assert!(
        !text.contains(CANARY) && !text.contains("CANARY"),
        "key in the settings_get payload: {text}"
    );
}

#[test]
fn save_twice_replaces_settings_json() {
    // Acceptance 2. Bite: settings_save not reaching SettingsService::save, the file
    // not replaced over an existing settings.json on NTFS, a leftover temp file, or
    // load_settings reading another directory than the one it wrote.
    let dir = TempDir::new();
    let store = fake_store(FakeCredentialStore::new());
    let (service, _) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    assert!(
        dir.path().join(SETTINGS_FILE).is_file(),
        "first run wrote no settings.json"
    );
    let h = harness(&service);

    for size in [42, 43] {
        let outcome = h.save(&edited(size), [untouched(), untouched(), untouched()]);
        assert_eq!(
            outcome["Saved"]["view"]["settings"]["history"]["size"],
            json!(size),
            "save {size}: {outcome}"
        );
        assert_eq!(
            read_settings_file(dir.path()),
            edited(size),
            "after save {size}"
        );
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);
    }
    drop(h);
    drop(service);

    let (_, outcome) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    assert_eq!(outcome, LoadOutcome::Loaded(edited(43)));
}

#[test]
fn saved_emits_changed_once_refused_emits_nothing() {
    // Acceptance 3, J3. Bite: no bridge; an emit inside settings_save as well as the
    // bridge (two events); an event on Refused; a payload other than view(); a save
    // that does not come through settings_save not reaching the windows.
    let dir = TempDir::new();
    let store = fake_store(FakeCredentialStore::new());
    let (service, _) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    let h = harness(&service);
    let events = h.changed_events();
    spawn_change_bridge(h.app.handle().clone(), service.clone());

    // Saved through IPC: exactly one event, equal to the view.
    let outcome = h.save(&edited(42), [untouched(), untouched(), untouched()]);
    let saved_view = outcome["Saved"]["view"].clone();
    assert!(saved_view.is_object(), "expected Saved: {outcome}");
    let event = events
        .recv_timeout(EVENT_TIMEOUT)
        .expect("no settings://changed after Saved");
    assert_eq!(event, view_json(&service));
    assert_eq!(event, saved_view);
    assert!(
        events.recv_timeout(QUIET).is_err(),
        "more than one settings://changed for one Saved"
    );

    // Refused at validation: no event.
    let outcome = h.save(&edited(0), [untouched(), untouched(), untouched()]);
    assert_eq!(
        outcome,
        json!({ "Refused": {
            "errors": [ { "field": "history.size", "code": "history.size_range" } ],
            "form_error": null,
        } })
    );
    assert!(
        events.recv_timeout(QUIET).is_err(),
        "settings://changed after a Refused"
    );

    // Saved by another caller (not settings_save): still exactly one event.
    let direct = service.save(SaveRequest {
        settings: edited(7),
        keys: KeyEdits::default(),
    });
    assert!(matches!(direct, SaveOutcome::Saved { .. }), "{direct:?}");
    let event = events
        .recv_timeout(EVENT_TIMEOUT)
        .expect("no settings://changed after a direct Saved");
    assert_eq!(event["settings"]["history"]["size"], json!(7));
    assert!(events.recv_timeout(QUIET).is_err(), "duplicate event");
}

#[test]
fn key_store_write_refused_is_key_store_failed_and_nothing_written() {
    // Acceptance 4 (NFR-04, no fallback). Bite: the failure swallowed (Saved), the
    // error on another field, settings.json written anyway, the key put anywhere
    // else (a file under the data dir), a retry, or an event.
    let dir = TempDir::new();
    let store = fake_store(FakeCredentialStore::new());
    store.fail(
        CredentialOp::Write,
        KeySlot::TranscriptionApi,
        CredentialError { os_code: 5 },
    );
    let (service, _) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    let before_bytes = std::fs::read(dir.path().join(SETTINGS_FILE)).expect("first-run file");
    let before_entries = entries(dir.path());
    let h = harness(&service);
    let events = h.changed_events();
    spawn_change_bridge(h.app.handle().clone(), service.clone());

    let outcome = h.save(&edited(42), [replace(CANARY), untouched(), untouched()]);

    assert_eq!(
        outcome,
        json!({ "Refused": {
            "errors": [ { "field": "engine.api.key", "code": "key.store_failed" } ],
            "form_error": null,
        } })
    );
    assert_eq!(
        std::fs::read(dir.path().join(SETTINGS_FILE)).expect("file still there"),
        before_bytes,
        "settings.json changed"
    );
    assert_eq!(entries(dir.path()), before_entries, "data dir changed");
    for file in files_under(dir.path()) {
        let bytes = std::fs::read(&file).expect("read");
        assert!(
            !contains(&bytes, CANARY.as_bytes()),
            "key in {}",
            file.display()
        );
    }
    assert_eq!(store.stored(KeySlot::TranscriptionApi), None);
    let call = |op, slot| CredentialCall { op, slot };
    assert_eq!(
        store.calls(),
        vec![
            call(CredentialOp::Read, KeySlot::TranscriptionApi),
            call(CredentialOp::Read, KeySlot::LocalServer),
            call(CredentialOp::Read, KeySlot::PostProcessing),
            call(CredentialOp::Write, KeySlot::TranscriptionApi),
        ]
    );
    assert!(
        events.recv_timeout(QUIET).is_err(),
        "settings://changed after a refused save"
    );
    assert!(!outcome.to_string().contains(CANARY));
}

#[cfg(windows)]
#[test]
fn canary_key_never_in_data_dir_or_log() {
    // Acceptance 1, last clause; J1. A real WinCredentialStore under a throwaway
    // target prefix. Bite: a key written into settings.json, the log or any other
    // file under the data dir (UTF-8 or UTF-16LE), or into the save response or
    // the settings://changed payload.
    use voicen_lib::credentials::WinCredentialStore;

    let prefix = format!(
        "voicen-test-{}-ipc-{}/",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let cleanup = WinCredentialStore::with_target_prefix(prefix.clone());
    struct Cleanup(WinCredentialStore);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for slot in KeySlot::all() {
                let _ = self.0.delete(slot);
            }
        }
    }
    let _cleanup = Cleanup(cleanup);

    let dir = TempDir::new();
    let log_dir = dir.path().join("logs");
    voicen_lib::log_start(&log_dir);
    let store: Arc<dyn CredentialStore> =
        Arc::new(WinCredentialStore::with_target_prefix(prefix.clone()));
    let (service, _) = load_settings(dir.path().to_path_buf(), store, None);
    let h = harness(&service);
    let events = h.changed_events();
    spawn_change_bridge(h.app.handle().clone(), service.clone());

    let mut settings = edited(42);
    settings.engine = EngineKind::Api;
    let outcome = h.save(
        &settings,
        [replace(CANARY), replace(CANARY), replace(CANARY)],
    );
    assert!(outcome["Saved"].is_object(), "expected Saved: {outcome}");
    assert_eq!(
        outcome["Saved"]["view"]["keys"],
        json!({ "transcription_api": true, "local_server": true, "post_processing": true })
    );
    let event = events
        .recv_timeout(EVENT_TIMEOUT)
        .expect("no settings://changed after Saved");

    // The keys did reach Credential Manager.
    let reader = WinCredentialStore::with_target_prefix(prefix);
    for slot in KeySlot::all() {
        let key = reader
            .read(slot)
            .unwrap_or_else(|e| panic!("read {slot:?}: {e:?}"))
            .map(|s| s.expose().to_string());
        assert_eq!(key.as_deref(), Some(CANARY), "{slot:?}");
    }

    // ...and nowhere else.
    for text in [outcome.to_string(), event.to_string()] {
        assert!(!text.contains(CANARY) && !text.contains("CANARY"), "{text}");
    }
    let utf16: Vec<u8> = CANARY.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let files = files_under(dir.path());
    assert!(
        files.contains(&dir.path().join(SETTINGS_FILE))
            && files.contains(&log_dir.join("voicen.log")),
        "settings.json and the log must exist for this check to mean anything: {files:?}"
    );
    for file in files {
        let bytes = std::fs::read(&file).expect("read");
        assert!(
            !contains(&bytes, CANARY.as_bytes()) && !contains(&bytes, &utf16),
            "key in {}",
            file.display()
        );
    }
}

#[test]
fn speech_languages_is_core_list() {
    // Decision #30. Bite: a list re-spelled in the shell, re-ordered, or filtered.
    let dir = TempDir::new();
    let store = fake_store(FakeCredentialStore::new());
    let (service, _) = load_settings(dir.path().to_path_buf(), as_port(&store), None);
    let h = harness(&service);

    let languages = h
        .invoke("settings_speech_languages", json!({}))
        .unwrap_or_else(|e| panic!("settings_speech_languages rejected: {e}"));

    assert_eq!(languages, json!(WHISPER_ISO_639_1));
    assert_eq!(languages.as_array().map(Vec::len), Some(97));
}

#[test]
fn first_run_with_russian_os_language_writes_ru() {
    // Acceptance 5 (spec 004 T049), the load half. Bite: load_settings dropping
    // the OS language (defaults(None) = en written on Russian Windows).
    let dir = TempDir::new();
    let store = fake_store(FakeCredentialStore::new());
    let (service, outcome) =
        load_settings(dir.path().to_path_buf(), as_port(&store), Some("ru-RU"));

    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(Some("ru-RU"))));
    assert_eq!(service.snapshot().ui_language, UiLanguage::Ru);
    let raw: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json written"),
    )
    .expect("parses");
    assert_eq!(raw["ui_language"], json!("ru"));
    assert_eq!(raw["engine"], json!("none"));
}

#[test]
fn first_language_tag_is_the_first_entry_of_the_list() {
    // Acceptance 5, the OS half, with injected GetUserPreferredUILanguages buffers.
    // Bite: the last entry taken, the terminator kept, an empty list read as "".
    use voicen_lib::locale::first_language_tag;

    fn multi_sz(tags: &[&str]) -> Vec<u16> {
        let mut buf: Vec<u16> = Vec::new();
        for tag in tags {
            buf.extend(tag.encode_utf16());
            buf.push(0);
        }
        buf.push(0);
        buf
    }

    assert_eq!(
        first_language_tag(&multi_sz(&["ru-RU", "en-US"])),
        Some("ru-RU".to_string())
    );
    assert_eq!(
        first_language_tag(&multi_sz(&["en-US", "ru-RU"])),
        Some("en-US".to_string())
    );
    assert_eq!(
        first_language_tag(&multi_sz(&["ru-RU"])),
        Some("ru-RU".to_string())
    );
    assert_eq!(first_language_tag(&multi_sz(&[])), None);
    assert_eq!(first_language_tag(&[]), None);
    // An unpaired surrogate: not a tag.
    assert_eq!(first_language_tag(&[0xD800, 0, 0]), None);

    // The composed rule: a Russian first entry makes the first run Russian.
    let tag = first_language_tag(&multi_sz(&["ru-RU", "en-US"]));
    assert_eq!(defaults(tag.as_deref()).ui_language, UiLanguage::Ru);
}

#[cfg(windows)]
#[test]
fn os_language_reads_a_tag_on_this_machine() {
    // The real call (no injected buffer). Bite: os_language() never calling
    // GetUserPreferredUILanguages (always None), or returning the whole buffer.
    let tag = voicen_lib::locale::os_language().expect("the runner has a UI language");
    assert!(!tag.is_empty());
    assert!(
        tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "not a single BCP-47 tag: {tag:?}"
    );
}

#[cfg(windows)]
#[test]
fn data_dir_is_localappdata_voicen_and_logs_live_under_it() {
    // J2, P-010: one resolver. Read only, never written by tests. Bite: a second
    // resolver for logs, or another folder name.
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA set on Windows");
    let data_dir = voicen_lib::paths::data_dir();
    assert_eq!(data_dir, PathBuf::from(local).join("Voicen"));
    assert_eq!(voicen_lib::paths::log_dir(), data_dir.join("logs"));
}
