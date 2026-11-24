//! T-022: the history recorder (the production `TranscriptSink`) and the settings
//! follower, through the real `SettingsService` save path (P-013; spec 005 T022,
//! FR-003, FR-008): saving `history.enabled = false` in the settings deletes the
//! files, lowering `history.size` trims them, and the recorder never fails a job.
//!
//! Pinned API (the developer builds exactly this):
//! - `history::HistoryRecorder<S: HistoryStore>`, `new(history: History<S>, clock:
//!   Arc<dyn Clock>, on_change: Box<dyn Fn(HistoryView) + Send + Sync>)`, implements
//!   `pipeline::TranscriptSink`, `view() -> HistoryView`; `ready_at_ms` is the
//!   clock's UTC milliseconds at the call; `on_change` is called with the new view
//!   after each change (the shell emits it to the `history` window).
//! - `history::follow_settings(recorder: Arc<HistoryRecorder<S>>, rx:
//!   mpsc::Receiver<Arc<Settings>>)`: applies every received snapshot's
//!   `history.{enabled,size}` with `History::apply_settings`; blocks until the
//!   channel is closed (the shell runs it on its own thread).
//!
//! Fake data only: `example.com`, `sk-test-...`.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::history::{
    follow_settings, EngineKind as HistoryEngine, EngineLabel, FileHistoryStore, History,
    HistoryError, HistoryErrorKind, HistoryNotice, HistoryOp, HistoryRecorder, HistoryView,
};
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::TranscriptSink;
use voicen_core::secrets::{FakeCredentialStore, KeyEdit, KeyEdits, Secret};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::TempDir;

const OS: Option<&str> = Some("en-US");
const SENTINEL: &str = "SENTINEL-TEXT-c41d08";

fn label() -> EngineLabel {
    EngineLabel {
        kind: HistoryEngine::Local,
        model: "small".to_string(),
    }
}

fn service() -> SettingsService {
    let bytes = serde_json::to_vec(&defaults(OS)).expect("serialize defaults");
    let (service, _) = SettingsService::load_or_init(
        SettingsDeps {
            file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
            credentials: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(voicen_core::settings::roots::FakeCallRoots::new()),
        },
        OS,
    );
    service
}

/// A valid save: engine `api`, with its key, and the given history settings.
fn save(service: &SettingsService, enabled: bool, size: u32) {
    let mut s: Settings = (*service.snapshot()).clone();
    s.engine = EngineKind::Api;
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.api.model = "whisper-test".to_string();
    s.history.enabled = enabled;
    s.history.size = size;
    let outcome = service.save(SaveRequest {
        settings: s,
        keys: KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new("sk-test-SECRET")),
            local_server: KeyEdit::Untouched,
            post_processing: KeyEdit::Untouched,
        },
    });
    assert!(
        matches!(outcome, SaveOutcome::Saved { .. }),
        "save refused: {outcome:?}"
    );
}

struct Rig {
    dir: TempDir,
    recorder: Arc<HistoryRecorder<FileHistoryStore>>,
    changes: Arc<Mutex<Vec<HistoryView>>>,
    clock: Arc<FakeClock>,
}

fn rig(enabled: bool, size: u8) -> Rig {
    let dir = TempDir::new();
    let (history, warning) = History::open(
        FileHistoryStore::new(dir.path().to_path_buf()),
        enabled,
        size,
    );
    assert_eq!(warning, None);
    let changes: Arc<Mutex<Vec<HistoryView>>> = Arc::default();
    let sink = changes.clone();
    let clock = Arc::new(FakeClock::at(
        UNIX_EPOCH + Duration::from_secs(1_790_000_000),
    ));
    let recorder = Arc::new(HistoryRecorder::new(
        history,
        clock.clone(),
        Box::new(move |view| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(view)
        }),
    ));
    Rig {
        dir,
        recorder,
        changes,
        clock,
    }
}

/// Runs the follower over `rx` on its own thread until the channel closes.
fn follow(
    recorder: &Arc<HistoryRecorder<FileHistoryStore>>,
    rx: Receiver<Arc<Settings>>,
) -> std::thread::JoinHandle<()> {
    let recorder = recorder.clone();
    std::thread::spawn(move || follow_settings(recorder, rx))
}

fn history_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("history.json"))
        .collect()
}

fn texts(view: &HistoryView) -> Vec<String> {
    view.entries.iter().map(|e| e.text.clone()).collect()
}

// ---- the follower ------------------------------------------------------------------------

#[test]
fn saving_history_off_in_the_settings_deletes_the_files() {
    // Bite: the follower not subscribed / not applying, `enabled` ignored, the
    // delete skipped (entries only emptied in memory).
    let r = rig(true, 20);
    r.recorder.delivered(SENTINEL, label());
    assert_eq!(
        history_files(r.dir.path()),
        vec!["history.json".to_string()],
        "premise"
    );

    let service = service();
    let rx = service.subscribe();
    let follower = follow(&r.recorder, rx);
    save(&service, false, 20);
    drop(service); // closes the channel: the follower returns after the last snapshot
    follower.join().expect("the follower must not panic");

    assert_eq!(history_files(r.dir.path()), Vec::<String>::new());
    let view = r.recorder.view();
    assert!(!view.enabled);
    assert_eq!(view.entries, vec![]);
}

#[test]
fn a_delivery_after_history_was_turned_off_writes_nothing() {
    let r = rig(true, 20);
    let service = service();
    let follower = follow(&r.recorder, service.subscribe());
    save(&service, false, 20);
    drop(service);
    follower.join().expect("follower");

    r.recorder.delivered(SENTINEL, label());
    assert_eq!(history_files(r.dir.path()), Vec::<String>::new());
    assert_eq!(r.recorder.view().entries, vec![]);
}

#[test]
fn saving_off_then_on_again_starts_empty_and_writes_nothing() {
    let r = rig(true, 20);
    for i in 1..=5 {
        r.recorder.delivered(&format!("t{i}"), label());
    }
    let service = service();
    let follower = follow(&r.recorder, service.subscribe());
    save(&service, true, 2);
    save(&service, false, 2);
    save(&service, true, 2);
    drop(service);
    follower.join().expect("follower");
    let view = r.recorder.view();
    assert!(view.enabled);
    assert_eq!(view.entries, vec![], "off then on: empty");
    assert_eq!(
        history_files(r.dir.path()),
        Vec::<String>::new(),
        "nothing written on turning on"
    );
}

#[test]
fn saving_a_smaller_size_trims_memory_and_the_stored_document() {
    let r = rig(true, 20);
    for i in 1..=5 {
        r.recorder.delivered(&format!("t{i}"), label());
    }
    let service = service();
    let follower = follow(&r.recorder, service.subscribe());
    save(&service, true, 2);
    drop(service);
    follower.join().expect("follower");
    assert_eq!(
        texts(&r.recorder.view()),
        vec!["t5".to_string(), "t4".to_string()]
    );
    let reopened = History::open(FileHistoryStore::new(r.dir.path().to_path_buf()), true, 20).0;
    assert_eq!(
        texts(&reopened.view()),
        vec!["t5".to_string(), "t4".to_string()],
        "stored too"
    );
}

// ---- the recorder --------------------------------------------------------------------------

#[test]
fn the_recorder_stamps_the_clock_time_and_the_label_and_announces_the_change() {
    let r = rig(true, 20);
    r.recorder.delivered("hello", label());
    let view = r.recorder.view();
    assert_eq!(view.entries.len(), 1);
    assert_eq!(view.entries[0].text, "hello");
    assert_eq!(view.entries[0].ready_at_ms, 1_790_000_000_000);
    assert_eq!(view.entries[0].engine, label());

    r.clock.set(UNIX_EPOCH + Duration::from_secs(1_790_000_060));
    r.recorder.delivered("later", label());
    assert_eq!(r.recorder.view().entries[0].ready_at_ms, 1_790_000_060_000);

    let changes = r
        .changes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(changes.len(), 2, "one announcement per recorded text");
    assert_eq!(
        texts(&changes[1]),
        vec!["later".to_string(), "hello".to_string()]
    );
}

#[test]
fn the_recorder_survives_a_disk_failure_and_shows_the_notice() {
    // NFR-07, SC-004: a delivery never fails because of the history. The data
    // directory is a file, so every save fails.
    let dir = TempDir::new();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("blocker");
    let (history, _) = History::open(FileHistoryStore::new(blocker.join("data")), true, 20);
    let recorder = HistoryRecorder::new(
        history,
        Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        )),
        Box::new(|_| {}),
    );
    recorder.delivered(SENTINEL, label());
    let view = recorder.view();
    assert_eq!(texts(&view), vec![SENTINEL.to_string()], "kept in memory");
    assert_eq!(view.notice, Some(HistoryNotice::SaveFailed));
}

#[test]
fn the_recorder_hands_every_disk_error_to_its_error_sink_without_text() {
    // FR-011 (T-022 failure-branch bite, mutant 14): the shell logs `history_failed`
    // from the recorder's error callback, so a disk error that never reaches it is a
    // silent failure. One callback call per failed operation, op and kind only.
    let dir = TempDir::new();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("blocker");
    let (history, _) = History::open(FileHistoryStore::new(blocker.join("data")), true, 20);
    let errors: Arc<Mutex<Vec<HistoryError>>> = Arc::default();
    let seen = errors.clone();
    let recorder = HistoryRecorder::new(
        history,
        Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        )),
        Box::new(|_| {}),
    )
    .with_errors(Box::new(move |error| {
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error.clone())
    }));

    recorder.delivered(SENTINEL, label());
    let got = errors
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(got.len(), 1, "one error for the one failed save: {got:?}");
    assert_eq!(got[0].op, HistoryOp::Save);
    assert!(matches!(got[0].kind, HistoryErrorKind::Io(_)), "{got:?}");
    for error in &got {
        assert!(!error.to_string().contains(SENTINEL));
        assert!(!format!("{error:?}").contains(SENTINEL));
    }

    // A successful operation reports nothing more.
    let ok = TempDir::new();
    let (history, _) = History::open(FileHistoryStore::new(ok.path().to_path_buf()), true, 20);
    let quiet: Arc<Mutex<usize>> = Arc::default();
    let count = quiet.clone();
    let recorder = HistoryRecorder::new(
        history,
        Arc::new(FakeClock::at(UNIX_EPOCH)),
        Box::new(|_| {}),
    )
    .with_errors(Box::new(move |_| {
        *count.lock().unwrap_or_else(PoisonError::into_inner) += 1
    }));
    recorder.delivered(SENTINEL, label());
    recorder.clear();
    assert_eq!(*quiet.lock().unwrap_or_else(PoisonError::into_inner), 0);
}
