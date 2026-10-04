//! T-014: `WinAutostart` against the real HKCU Run key, and the autostart wiring of
//! the shell's one construction path (`load_settings` → `reconcile_autostart`) and
//! app wiring (`build_app` → `settings_save`). Spec 004 FR-019, research R-5;
//! contracts/core-traits.md#autostart. Windows CI only (decision #5).
//!
//! Every real-registry test uses its own throwaway value name
//! (`WinAutostart::with_value_name`), so the user's real `Voicen` value is never
//! touched, and a guard deletes that value with raw `RegDeleteKeyValueW` (not
//! through the impl under test) whether the test passes or not. Expected registry
//! contents are read back with raw `RegQueryValueExW`.
#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};
use voicen_core::autostart::{Autostart, AutostartCall, AutostartError, FakeAutostart};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{defaults, LoadOutcome, Settings};
use voicen_core::test_support::TempDir;
use voicen_lib::autostart::{
    launched_by_autostart, WinAutostart, AUTOSTART_ARG, RUN_SUBKEY, RUN_VALUE_NAME,
};
use voicen_lib::settings_ipc::load_settings;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegOpenKeyExW, RegQueryValueExW, RegSetKeyValueW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_SZ, REG_VALUE_TYPE,
};

/// The per-user Run key, written out here (not taken from the impl).
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The user's real value; only ever read by these tests.
const REAL_VALUE: &str = "Voicen";

/// `voicen-test-<pid>-<n>-<nanos>`: unique per call, never the real `Voicen`.
fn unique_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "voicen-test-{}-{}-{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `s` as `REG_SZ` data: UTF-16LE with the terminating NUL.
fn reg_sz(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// What the Run value must hold: the quoted path of this (test) exe, then
/// `--autostart`.
fn expected_command() -> String {
    let exe = std::env::current_exe().expect("current_exe");
    format!("\"{}\" --autostart", exe.display())
}

/// One value under the Run key, read raw.
#[derive(Debug, PartialEq, Eq)]
struct RawValue {
    kind: u32,
    bytes: Vec<u8>,
}

fn raw_read(name: &str) -> Option<RawValue> {
    let subkey = wide(RUN);
    let value = wide(name);
    let mut key = HKEY(std::ptr::null_mut());
    // SAFETY: every string is NUL-terminated and outlives the calls; `key` is
    // closed before returning; `buf` is `size` bytes long.
    unsafe {
        let opened = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut key,
        );
        assert_eq!(opened, ERROR_SUCCESS, "open HKCU Run");
        let mut kind = REG_VALUE_TYPE(0);
        let mut size = 0u32;
        let queried = RegQueryValueExW(
            key,
            PCWSTR(value.as_ptr()),
            None,
            Some(&mut kind as *mut REG_VALUE_TYPE),
            None,
            Some(&mut size as *mut u32),
        );
        if queried == ERROR_FILE_NOT_FOUND {
            let _ = RegCloseKey(key);
            return None;
        }
        assert_eq!(queried, ERROR_SUCCESS, "size of {name}");
        let mut buf = vec![0u8; size as usize];
        let queried = RegQueryValueExW(
            key,
            PCWSTR(value.as_ptr()),
            None,
            Some(&mut kind as *mut REG_VALUE_TYPE),
            Some(buf.as_mut_ptr()),
            Some(&mut size as *mut u32),
        );
        let _ = RegCloseKey(key);
        assert_eq!(queried, ERROR_SUCCESS, "data of {name}");
        buf.truncate(size as usize);
        Some(RawValue {
            kind: kind.0,
            bytes: buf,
        })
    }
}

fn raw_write(name: &str, data: &str) {
    let subkey = wide(RUN);
    let value = wide(name);
    let bytes = reg_sz(data);
    // SAFETY: NUL-terminated strings and `bytes` outlive the call; `cbdata` is
    // the length of `bytes`.
    let written = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            REG_SZ.0,
            Some(bytes.as_ptr().cast::<std::ffi::c_void>()),
            bytes.len() as u32,
        )
    };
    assert_eq!(written, ERROR_SUCCESS, "seed {name}");
}

fn raw_delete(name: &str) {
    let subkey = wide(RUN);
    let value = wide(name);
    // SAFETY: NUL-terminated strings outlive the call.
    let _ = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
        )
    };
}

/// A throwaway value name, deleted raw on drop.
struct TestValue(String);

impl TestValue {
    fn new() -> TestValue {
        TestValue(unique_name())
    }

    fn autostart(&self) -> WinAutostart {
        WinAutostart::with_value_name(self.0.clone())
    }

    fn port(&self) -> Arc<dyn Autostart> {
        Arc::new(self.autostart())
    }

    fn read(&self) -> Option<RawValue> {
        raw_read(&self.0)
    }
}

impl Drop for TestValue {
    fn drop(&mut self) {
        raw_delete(&self.0);
    }
}

fn expected_value() -> RawValue {
    RawValue {
        kind: REG_SZ.0,
        bytes: reg_sz(&expected_command()),
    }
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn write_settings(dir: &TempDir, settings: &Settings) {
    let bytes = serde_json::to_vec_pretty(settings).expect("settings serialize");
    std::fs::write(dir.path().join(SETTINGS_FILE), bytes).expect("seed settings.json");
}

fn with_start(on: bool) -> Settings {
    let mut settings = defaults(None);
    settings.start_with_windows = on;
    settings
}

// ---- WinAutostart over the real Run key -------------------------------------

#[test]
fn set_true_writes_quoted_exe_with_autostart_arg_as_reg_sz() {
    // Acceptance "on → HKCU Run entry present". Bite: set(true) not writing, the
    // path unquoted (breaks on "Program Files"-style paths), the argument missing,
    // another type than REG_SZ, the NUL not counted, another key or value name.
    let real_before = raw_read(REAL_VALUE);
    let value = TestValue::new();
    let autostart = value.autostart();

    autostart.set(true).expect("set(true)");
    assert_eq!(value.read(), Some(expected_value()));
    assert_eq!(autostart.is_enabled(), Ok(true));
    // Idempotent.
    autostart.set(true).expect("set(true) again");
    assert_eq!(value.read(), Some(expected_value()));
    assert_eq!(
        raw_read(REAL_VALUE),
        real_before,
        "the real Voicen value changed"
    );
}

#[test]
fn set_false_removes_the_value_and_absent_is_ok() {
    // Acceptance "off → absent". Bite: set(false) not deleting, an absent value
    // reported as an error (ERROR_FILE_NOT_FOUND not mapped to Ok), or is_enabled
    // stuck on true.
    let real_before = raw_read(REAL_VALUE);
    let value = TestValue::new();
    let autostart = value.autostart();
    raw_write(&value.0, &expected_command());
    assert_eq!(autostart.is_enabled(), Ok(true));

    autostart.set(false).expect("set(false)");
    assert_eq!(value.read(), None);
    assert_eq!(autostart.is_enabled(), Ok(false));
    assert_eq!(autostart.set(false), Ok(()), "absent value must be Ok");
    assert_eq!(value.read(), None);
    assert_eq!(
        raw_read(REAL_VALUE),
        real_before,
        "the real Voicen value changed"
    );
}

#[test]
fn is_enabled_is_true_for_any_existing_value() {
    // core-traits: is_enabled = the value exists, whatever its data. Bite: is_enabled
    // comparing the data with the current command (a stale entry would read as off
    // and never be removed by reconcile).
    let value = TestValue::new();
    let autostart = value.autostart();
    assert_eq!(autostart.is_enabled(), Ok(false));
    raw_write(&value.0, r#""C:\Somewhere\else.exe""#);
    assert_eq!(autostart.is_enabled(), Ok(true));
}

#[test]
fn set_true_rewrites_a_stale_path() {
    // R-5: set(true) always writes the current command, which fixes a path left by
    // an older install location. Bite: set(true) skipping the write when the value
    // already exists.
    let value = TestValue::new();
    raw_write(&value.0, r#""C:\Old\Install\voicen.exe" --autostart"#);

    value.autostart().set(true).expect("set(true)");
    assert_eq!(value.read(), Some(expected_value()));
}

#[test]
fn release_names_are_pinned() {
    // 006's uninstaller hook deletes the same literal; the Run value passes the
    // argument `settings_window::on_ready` (T-037) reads. Bite: any constant changed.
    assert_eq!(RUN_VALUE_NAME, "Voicen");
    assert_eq!(AUTOSTART_ARG, "--autostart");
    assert_eq!(RUN_SUBKEY, RUN);
}

#[test]
fn launched_by_autostart_reads_the_argument() {
    // US5-3: the startup executor learns it was started by the Run value. Bite: the
    // argument ignored, matched by prefix, or matched case-insensitively.
    let exe = r"C:\Users\test\AppData\Local\Voicen\voicen.exe";
    assert!(launched_by_autostart([exe, "--autostart"]));
    assert!(launched_by_autostart(
        std::iter::once(exe).chain(std::iter::once(AUTOSTART_ARG))
    ));
    assert!(!launched_by_autostart([exe]));
    assert!(!launched_by_autostart(Vec::<&str>::new()));
    assert!(!launched_by_autostart([exe, "--autostart-later"]));
    assert!(!launched_by_autostart([exe, "--other"]));
}

// ---- load_settings: reconcile at start (J4) ------------------------------------

#[test]
fn load_settings_writes_the_value_for_a_saved_on_setting() {
    // Acceptance "startup reconciles ... on → written with the current exe path",
    // through load_settings. Bite: load_settings not calling reconcile_autostart.
    let value = TestValue::new();
    let dir = TempDir::new();
    write_settings(&dir, &with_start(true));

    let (_service, outcome) =
        load_settings(dir.path().to_path_buf(), no_keys(), value.port(), None);
    assert_eq!(outcome, LoadOutcome::Loaded(with_start(true)));
    assert_eq!(value.read(), Some(expected_value()));
}

#[test]
fn load_settings_removes_the_value_for_a_saved_off_setting() {
    // Acceptance "off → removed", through load_settings. Bite: reconcile not called,
    // or a present value kept for a saved off.
    let value = TestValue::new();
    raw_write(&value.0, r#""C:\Old\Install\voicen.exe" --autostart"#);
    let dir = TempDir::new();
    write_settings(&dir, &with_start(false));

    let (_service, outcome) =
        load_settings(dir.path().to_path_buf(), no_keys(), value.port(), None);
    assert_eq!(outcome, LoadOutcome::Loaded(with_start(false)));
    assert_eq!(value.read(), None);
}

#[test]
fn load_settings_reconciles_through_the_injected_port() {
    // J4 with a fake: the one construction path hands the injected Autostart to the
    // service and reconciles once. FirstRun reconciles from the defaults (off), so a
    // leftover entry is removed. Bite: reconcile not called, called twice, or called
    // on another Autostart than the injected one.
    let dir = TempDir::new();
    let fake = Arc::new(FakeAutostart::enabled());
    let (_service, outcome) =
        load_settings(dir.path().to_path_buf(), no_keys(), fake.clone(), None);
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));
    assert_eq!(
        fake.calls(),
        vec![AutostartCall::IsEnabled, AutostartCall::Set(false)]
    );
    assert!(!fake.is_on());
}

#[test]
fn load_settings_makes_no_autostart_call_while_unavailable() {
    // Acceptance "no call while Unavailable" (#19 by analogy), through load_settings.
    // A directory at settings.json makes the read fail. Bite: reconcile acting on
    // the in-memory defaults (would delete the entry of a user whose file merely
    // could not be read).
    let dir = TempDir::new();
    std::fs::create_dir(dir.path().join(SETTINGS_FILE)).expect("dir at settings.json");
    let fake = Arc::new(FakeAutostart::enabled());
    let (_service, outcome) =
        load_settings(dir.path().to_path_buf(), no_keys(), fake.clone(), None);
    assert!(
        matches!(outcome, LoadOutcome::Unavailable(_)),
        "{outcome:?}"
    );
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    assert!(fake.is_on());
}

// ---- settings_save through build_app (the release wiring) ---------------------

struct Harness {
    _app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

fn harness(service: &Arc<SettingsService>) -> Harness {
    let app = voicen_lib::build_app(mock_builder(), mock_context(noop_assets()), service.clone())
        .expect("mock app builds");
    let webview = WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .expect("mock webview builds");
    Harness { _app: app, webview }
}

impl Harness {
    /// `settings_save` with `settings` and every key `Untouched`, like the UI.
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
        get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: "settings_save".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "http://tauri.localhost".parse().expect("url"),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().expect("response is JSON"))
        .unwrap_or_else(|e| panic!("settings_save rejected: {e:?}"))
    }
}

#[test]
fn settings_save_on_writes_the_value_and_off_removes_it() {
    // Acceptance "on → HKCU Run entry present; off → absent" through the app's own
    // save path. Bite: the save step missing (Saved, Run value unchanged), or the
    // service built without the injected Autostart.
    let value = TestValue::new();
    let dir = TempDir::new();
    let (service, _) = load_settings(dir.path().to_path_buf(), no_keys(), value.port(), None);
    let h = harness(&service);

    let on = h.save(&with_start(true));
    assert_eq!(
        on["Saved"]["view"]["settings"]["start_with_windows"],
        json!(true),
        "{on}"
    );
    assert_eq!(value.read(), Some(expected_value()));

    let off = h.save(&with_start(false));
    assert_eq!(
        off["Saved"]["view"]["settings"]["start_with_windows"],
        json!(false),
        "{off}"
    );
    assert_eq!(value.read(), None);
}

#[test]
fn settings_save_with_a_refused_registry_write_is_autostart_failed() {
    // Acceptance failure branch on the wire: a refused Run write gives Refused with
    // general.start_with_windows / autostart.failed and settings.json unchanged.
    // Bite: the save step missing, or the error mapped to another field or code.
    let dir = TempDir::new();
    let fake = Arc::new(FakeAutostart::new());
    fake.fail_set(true, AutostartError { os_code: 5 });
    let (service, _) = load_settings(dir.path().to_path_buf(), no_keys(), fake.clone(), None);
    let before = std::fs::read(dir.path().join(SETTINGS_FILE)).expect("first run wrote");
    let h = harness(&service);

    let outcome = h.save(&with_start(true));
    assert_eq!(
        outcome,
        json!({ "Refused": {
            "errors": [ { "field": "general.start_with_windows", "code": "autostart.failed" } ],
            "form_error": null,
        } })
    );
    assert!(!fake.is_on());
    assert_eq!(
        std::fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json"),
        before
    );
}
