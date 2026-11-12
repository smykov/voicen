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
//! - `voicen_core::pending::FsTempAudioStore::new(dir, roots)`, with `roots` the
//!   service's `SettingsService::call_roots()`.
//!
//! Data: a `TempDir` per test (the rig's data dir); the store's folder is
//! `<data>/tmp/audio`. No network, no real audio.
#![cfg(windows)]

mod timing_support;
mod win32_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::RunEvent;
use voicen_core::pending::FsTempAudioStore;
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster, TempAudioStore,
};
use voicen_core::settings::{EngineKind, LoadOutcome};
use voicen_lib::dictation::{start_dictation, DictationHandle, DictationPorts};
use win32_support::{assert_hotkey_free, engine_factory, serial, AppRig, TextEngine};

/// The store over `<data>/tmp/audio`, with the roots of the rig's own settings service.
fn store_of(rig: &AppRig) -> (Arc<dyn TempAudioStore>, PathBuf) {
    let folder = rig.dir.path().join("tmp").join("audio");
    let service = Arc::clone(&rig.service);
    let store = FsTempAudioStore::new(folder.clone(), Arc::new(move || service.call_roots()));
    (Arc::new(store), folder)
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
