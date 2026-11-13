//! T-209 (spec 001 FR-032, decision #274, invariant I3): the shell wires the pending-audio
//! file store. `start_dictation` takes it through `DictationPorts::temp_audio`, removes the
//! stale files of an earlier session before the session starts, and manages it; the
//! `RunEvent::Exit` arm of `on_run_event` removes the files of this session. Only files
//! of the store's own name pattern (`pending-<id>.wav`, `.wav.part`) go, never anything else.
//!
//! Through the real wiring: `build_app` on the mock runtime (as `run()`), `start_dictation`
//! over fakes, and `on_run_event(app, RunEvent::Exit, ..)` called as the loop calls it
//! (the pattern of `crash.rs`'s tray Exit test). Windows CI only (decision #5); not compiled
//! on the Linux gate. The core store's own guarantees (atomic put, own-pattern, the calls-roots
//! guard) are tested in `voicen_core::pending`.
//!
//! The API these tests pin (the developer may rename, the tests follow):
//! - `DictationPorts { .., temp_audio: Arc<dyn TempAudioStore> }`;
//! - `voicen_lib::dictation::pending_store<R: Runtime>(app: &AppHandle<R>, dir: PathBuf)
//!   -> Arc<dyn TempAudioStore>`: the one constructor of the shipped store, called by
//!   `run()`'s `release_ports` (with `paths::temp_audio_dir()`) and by these tests. It reads
//!   the managed `Arc<SettingsService>` (`try_state`) for `call_roots()` at every call and
//!   fails closed (no delete, no write) when none is managed.
//!
//! Data: a `TempDir` per test (the rig's data dir); the store's folder is
//! `<data>/tmp/audio`. No network, no real audio.
#![cfg(windows)]

mod timing_support;
mod win32_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::{Manager, RunEvent};
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster, TempAudioStore,
};
use voicen_core::settings::{EngineKind, LoadOutcome};
use voicen_core::test_support::TempDir;
use voicen_lib::dictation::{pending_store, start_dictation, DictationHandle, DictationPorts};
use win32_support::{
    assert_hotkey_free, engine_factory, has_pair, headed, serial, AppRig, TextEngine,
};

/// The store over `<data>/tmp/audio`, from the shell's own constructor (the one `run()`'s
/// `release_ports` calls with `paths::temp_audio_dir()`). The rig's app manages the same
/// `Arc<SettingsService>` as `rig.service` (`build_app` -> `assemble` -> `app.manage(
/// Arc::clone(&service))`), so `pending_store`'s `try_state` finds the roots of that service.
fn store_of(rig: &AppRig) -> (Arc<dyn TempAudioStore>, PathBuf) {
    let folder = rig.dir.path().join("tmp").join("audio");
    let store = pending_store(rig.app.handle(), folder.clone());
    (store, folder)
}

fn start(rig: &AppRig, store: Arc<dyn TempAudioStore>) -> DictationHandle {
    let engine = TextEngine::new("pending audio test transcript");
    start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(FakeAudioSource::new()),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: Arc::new(FakeIndicator::new()),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
            temp_audio: store,
        },
    )
    .expect("start_dictation")
}

/// The loop's Exit event, through the shell's handler.
fn exit(rig: &AppRig) {
    let outcome = LoadOutcome::Loaded((*rig.service.snapshot()).clone());
    voicen_lib::on_run_event(rig.app.handle(), RunEvent::Exit, &outcome, false);
}

/// The names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn plant(folder: &Path, files: &[&str]) {
    fs::create_dir_all(folder).expect("create the store folder");
    for name in files {
        fs::write(folder.join(name), b"planted").expect("plant a file");
    }
}

#[test]
fn start_dictation_removes_the_stale_pending_files_of_an_earlier_session() {
    // T-209 Acceptance (shell): a pending-7.wav (and a crash-leftover .part) from an earlier
    // session is gone once start_dictation returns; notes.txt, a file the store does not own,
    // stays. Bite: NoPendingAudio still wired (the stale file stays), the temp_audio port
    // ignored, delete_all not called at start, a cleanup that empties the folder.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    let (store, folder) = store_of(&rig);
    plant(
        &folder,
        &["pending-7.wav", "pending-8.wav.part", "notes.txt"],
    );

    let _dictation = start(&rig, store);

    assert_eq!(
        names(&folder),
        vec!["notes.txt".to_string()],
        "only the file the store does not own is left after the start"
    );
}

#[test]
fn the_exit_event_removes_the_pending_files_of_this_session() {
    // T-209 Acceptance (shell): after the app's RunEvent::Exit through on_run_event the folder
    // holds no file of the pattern, however it got there (planted after the start, as a put of the session
    // would write it); notes.txt survives. Bite: no delete_all in the Exit arm, the store not
    // managed (the arm cannot find it), the arm deleting only the session's own id.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    let (store, folder) = store_of(&rig);
    let _dictation = start(&rig, store);

    plant(
        &folder,
        &[
            "pending-1.wav",
            "pending-9.wav",
            "pending-9.wav.part",
            "notes.txt",
        ],
    );
    assert!(
        names(&folder).contains(&"pending-1.wav".to_string()),
        "premise: the file is there: {:?}",
        names(&folder)
    );

    exit(&rig);

    assert_eq!(
        names(&folder),
        vec!["notes.txt".to_string()],
        "no file of the pattern after Exit; the non-pattern file stays"
    );
}

#[test]
fn a_calls_root_equal_to_the_store_folder_stops_both_cleanups() {
    // T-209 invariant I3 through the wiring: the user's calls root is chosen equal to the
    // store folder; neither the start cleanup nor the Exit arm deletes the pattern file in it
    // (the guard reads the roots from the service at every call). Bite: a store built over
    // a second, hard-wired resolution of the roots (the default `calls` folder), the guard
    // skipped in the shell.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    let (store, folder) = store_of(&rig);
    let mut draft = (*rig.service.snapshot()).clone();
    draft.calls.storage_root = Some(folder.clone());
    plant(&folder, &["pending-5.wav"]);
    let saved = rig
        .service
        .save(voicen_core::settings::service::SaveRequest {
            settings: draft,
            keys: voicen_core::secrets::KeyEdits::default(),
        });
    assert!(
        matches!(
            saved,
            voicen_core::settings::service::SaveOutcome::Saved { .. }
        ),
        "premise: the root is saved"
    );

    let _dictation = start(&rig, store);
    assert!(
        folder.join("pending-5.wav").is_file(),
        "the start cleanup deleted a file under a calls root"
    );
    exit(&rig);
    assert!(
        folder.join("pending-5.wav").is_file(),
        "the Exit cleanup deleted a file under a calls root"
    );
}

#[test]
fn the_shipped_store_fails_closed_without_a_managed_settings_service() {
    // T-209 invariant I3 (review #1): an app with no managed `Arc<SettingsService>` gives the
    // store no roots, so it cannot tell a calls folder from its own and neither cleanup
    // nor `delete_pending` removes anything. The app is a bare mock app (no `build_app`).
    // Bite: `pending_store` treating a missing service as "no roots" (allow all), a
    // `unwrap_or_default()` on the roots.
    let dir = TempDir::new();
    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("bare mock app builds");
    assert!(
        app.try_state::<Arc<voicen_core::settings::service::SettingsService>>()
            .is_none(),
        "premise: no settings service is managed"
    );
    let folder = dir.path().join("tmp").join("audio");
    plant(&folder, &["pending-3.wav", "notes.txt"]);

    let store = pending_store(app.handle(), folder.clone());
    let _ = store.delete_all();

    assert_eq!(
        names(&folder),
        vec!["notes.txt".to_string(), "pending-3.wav".to_string()],
        "a store without roots deleted a file"
    );
}

#[test]
fn a_refused_start_cleanup_writes_one_warning_line_without_the_path() {
    // T-209 review #1 finding #2: a failed start cleanup (here the calls-roots guard refuses,
    // the root equal to the store folder) is not silent. `start_dictation` writes exactly one
    // `warning kind=pending_audio_cleanup_failed` line, the kind and the OS code only: no path
    // of the folder, no file name. The session still starts. Bite: `let _ = delete_all()`
    // (no line), the line written per file, or the error text formatted into it.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    let (store, folder) = store_of(&rig);
    let mut draft = (*rig.service.snapshot()).clone();
    draft.calls.storage_root = Some(folder.clone());
    plant(&folder, &["pending-6.wav", "pending-7.wav"]);
    let saved = rig
        .service
        .save(voicen_core::settings::service::SaveRequest {
            settings: draft,
            keys: voicen_core::secrets::KeyEdits::default(),
        });
    assert!(
        matches!(
            saved,
            voicen_core::settings::service::SaveOutcome::Saved { .. }
        ),
        "premise: the root is saved"
    );

    let _dictation = start(&rig, store);

    let lines = rig.log_lines();
    let warnings: Vec<String> = headed(&lines, "warning")
        .into_iter()
        .filter(|l| has_pair(l, "kind", "pending_audio_cleanup_failed"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "one pending_audio_cleanup_failed line per failed start cleanup: {warnings:?}"
    );
    let folder_text = folder.to_string_lossy().to_string();
    for line in &lines {
        assert!(
            !line.contains(&folder_text) && !line.contains("pending-6"),
            "a log line names the pending-audio folder or a file: {line}"
        );
    }
}
