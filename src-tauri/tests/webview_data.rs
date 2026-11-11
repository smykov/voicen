//! T-062: the WebView2 data of every window lives under the data folder, in
//! `<data dir>\webview`, never in tauri's default `%LOCALAPPDATA%\<identifier>`
//! (FR-28 v4: the uninstaller's folder removal takes it too; decisions #76).
//!
//! The release context gets one app-level override (`paths::route_app_local_data`,
//! called once in `run()`): tauri's `app_local_data_dir`, which `prepare_webview`
//! uses for every window that sets no data directory of its own, then resolves to
//! `<root>\webview`, so `settings_window::open` and the overlay thread's build both
//! land there with no per-builder code. These tests build the app over the mock
//! runtime (tauri 2.12.1 `manager/webview.rs` fills and creates the directory for
//! every `Runtime`) with a `TempDir` root: the runner's real `%LOCALAPPDATA%\Voicen`
//! is never written. The same app without the helper must not create `<root>\webview`
//! (the bite: the folder appears because of the helper, not by itself). The installed
//! exe is proven by the install smoke step in ci.yml (P-013).
//!
//! Windows CI only (decision #5).
#![cfg(windows)]

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, Context, Manager};
use voicen_core::autostart::FakeAutostart;
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::Indicator;
use voicen_core::recording::OverlayState;
use voicen_core::secrets::FakeCredentialStore;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::dictation::ShellIndicator;
use voicen_lib::overlay;
use voicen_lib::paths;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window;

/// A hang bound, not a budget: how long the overlay's receipt may take.
const HANG: Duration = Duration::from_secs(120);

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

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

/// A log that cannot be written (its folder lies under this test exe, a file).
fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

fn load(dir: &Path) -> Arc<SettingsService> {
    load_settings(
        dir.to_path_buf(),
        Arc::new(FakeCredentialStore::new()),
        Arc::new(FakeAutostart::new()),
        Arc::new(ModelStore::new(dir.join("models"), MODELS)),
        None,
        &discard_log(),
    )
    .0
}

/// The app as `run()` builds it (`build_app`) over `context`, settings in `settings_dir`
/// (not the webview root).
fn app_with(settings_dir: &Path, context: Context<MockRuntime>) -> App<MockRuntime> {
    voicen_lib::build_app(
        mock_builder(),
        context,
        load(settings_dir),
        idle_models(),
        discard_log(),
    )
    .expect("mock app builds")
}

/// The mock context after the helper `run()` calls on the release context.
fn routed_context(root: &Path) -> Context<MockRuntime> {
    let mut context = mock_context(noop_assets());
    paths::route_app_local_data(&mut context, root);
    context
}

/// Opens the overlay window through the overlay thread and waits for its receipt.
#[track_caller]
fn open_overlay(app: &App<MockRuntime>) {
    ShellIndicator::new(app.handle()).set_overlay(&OverlayState::Recording);
    let receipt = overlay::settled(app.handle(), HANG);
    assert!(
        receipt.is_some_and(|r| !r.failed && r.window),
        "the overlay did not settle on a built window within {HANG:?}: {receipt:?}"
    );
}

#[track_caller]
fn open_settings(app: &App<MockRuntime>) {
    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("settings window opens");
}

#[test]
fn webview_dir_is_the_webview_folder_of_the_data_dir() {
    // The one resolver (P-010): `webview_dir()` is `data_dir()\webview`, the folder the
    // uninstaller's `Voicen\*.*` removal takes. Bite: any other folder name or root.
    assert_eq!(paths::webview_dir(), paths::data_dir().join("webview"));
}

#[test]
fn settings_window_webview_data_lands_under_the_routed_root() {
    // After `settings_window::open` on an app whose context went through the helper,
    // `<root>\webview` exists. Bite: no override (the dir is tauri's default folder),
    // an override of another folder, or a helper that stores the path but never sets
    // `app_directories_override`.
    let _serial = serial();
    let root = TempDir::new();
    let settings = TempDir::new();
    let app = app_with(settings.path(), routed_context(root.path()));
    assert!(
        !root.path().join("webview").exists(),
        "premise: building the app must not create the webview folder"
    );
    open_settings(&app);
    assert!(
        root.path().join("webview").is_dir(),
        "the settings window left no `webview` folder under the routed root {:?}",
        root.path()
    );
}

#[test]
fn overlay_webview_data_lands_under_the_routed_root() {
    // The overlay thread's `OverlayThread::build` takes the same default: with the
    // settings window never opened, the overlay alone makes `<root>\webview`. Bite:
    // the overlay builder setting its own data directory.
    let _serial = serial();
    let root = TempDir::new();
    let settings = TempDir::new();
    let app = app_with(settings.path(), routed_context(root.path()));
    assert!(
        !root.path().join("webview").exists(),
        "premise: building the app must not create the webview folder"
    );
    open_overlay(&app);
    assert!(
        root.path().join("webview").is_dir(),
        "the overlay window left no `webview` folder under the routed root {:?}",
        root.path()
    );
}

#[test]
fn without_the_helper_no_webview_folder_appears_under_the_root() {
    // The bite of the two tests above: the same app and the same two windows on the
    // plain context leave nothing under `root`, so the folder there is the helper's
    // doing. Bite: a test root that tauri fills regardless of the helper.
    let _serial = serial();
    let root = TempDir::new();
    let settings = TempDir::new();
    let app = app_with(settings.path(), mock_context(noop_assets()));
    open_settings(&app);
    open_overlay(&app);
    assert!(
        app.get_webview_window(settings_window::LABEL).is_some(),
        "premise: the settings window was built"
    );
    assert!(
        !root.path().join("webview").exists(),
        "an app whose context skipped the helper still created {:?}",
        root.path().join("webview")
    );
}
