//! T-022 (review 1 #1 a): the pipeline-to-history wiring and the required sink, through the
//! shell's real dictation wiring (`start_history` + `voicen_lib::dictation::start_dictation_with`
//! over `build_app`, as `run()`), with the session's inputs called directly (no injected keys,
//! the `delivery.rs` pattern) over fakes: a `FakeAudioSource` that speaks, a fixed-text test
//! engine of kind `api`, a `FakePaster` with a start window, a `FakeClipboard`. Windows CI only
//! (decision #5): the first observation is the `ci.yml` run on `wip/T-022` after implement.
//!
//! Why this file exists (review 1 #1; analysis "required, not defaulted", P-013;
//! docs/decisions/history.md "Rejected approaches"): the shell found the recorder in app state
//! and an app without a history part silently got a sink that keeps nothing, so a `run()` that
//! stopped calling `start_history` (or called it after the dictation, or whose call failed) gave
//! a green CI and a history that never fills. The sink of the pipeline is now a required choice
//! of the caller of `start_dictation_with`.
//!
//! Pinned API (the developer builds exactly this; none exists yet, so this file fails to compile
//! until it does, and that unresolved import is the red):
//!
//! ```text
//! voicen_lib::dictation::
//!   enum HistorySink { Managed, Discard }
//!       // Managed: the pipeline records into the app's managed history part
//!       //          (`history::start_history` ran before this call).
//!       // Discard: the explicit keep-nothing sink (`voicen_lib::history::NoHistory`, made
//!       //          public or private to this enum): the tests that look at no history,
//!       //          and `run()` after a failed `start_history` (dictation must go on, NFR-07).
//!   DictationPorts gains `pub history: HistorySink`   // required: no `Default`, no `Option`
//!   start_dictation_with(.., ports, ..) with `HistorySink::Managed` and no history part on the
//!       app is `Err(io::Error)` of kind `NotFound`, writes one `dictation_start_failed`
//!       warning line (as every other refused start), manages no `Arc<DictationSession>` and
//!       starts no hotkey thread. It never falls back to a keep-nothing sink.
//! ```
//!
//! The 16 `DictationPorts { .. }` constructors of the other shell tests (and `run()`'s
//! `release_ports`) get `history: HistorySink::Discard`; `run()` passes `Managed` when
//! `start_history` returned `Ok` and `Discard` (after the `history_start_failed` line, see
//! `history_ipc.rs`) when it failed. `voicen_lib::history::transcript_sink` may then lose its
//! keep-nothing fallback; `history_ipc.rs` uses it only on a started app.
//!
//! Every wait is on state (F-031); a bounded negative can only pass on a stall (T1).
#![cfg(windows)]

mod history_support;
mod timing_support;
mod win32_support;

use std::sync::Arc;
use std::thread;

use history_support::{get, texts_of, HistoryRig};
use serde_json::json;
use tauri::Manager;
use timing_support::now;
use voicen_core::dictation::DictationSession;
use voicen_core::platform::{
    FakeAudioSource, FakeIndicator, FakeNotifier, FakePaster, FakeTempAudioStore, FrameChunk,
    StartWindow, WindowRef,
};
use voicen_core::test_support::fixtures;
use voicen_core::vad::VadError;
use voicen_lib::dictation::{start_dictation_with, DictationHandle, DictationPorts, HistorySink};
use win32_support::{
    assert_hotkey_free, engine_factory, eventually, headed, holds_for, serial, TextEngine, HOLD,
    SETTLE,
};

/// The test engine's transcript: non-ASCII, with a canary the log must never contain.
const TRANSCRIPT: &str = "History wiring TRANSCRIPT-CANARY-9e4 проверка";
const CANARY: &str = "TRANSCRIPT-CANARY-9e4";

/// Three seconds of speech, delivered at once by every capture.
fn speaking_mic() -> Arc<FakeAudioSource> {
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_chunks(vec![FrameChunk::from_buffer(
        &fixtures::speech_3s(),
        now() + std::time::Duration::from_millis(10),
    )]);
    audio
}

/// `start_dictation_with` over fakes with the given history choice; the clipboard is the
/// rig's, so the dictation's one write is visible next to the copies of the history.
fn start(
    rig: &HistoryRig,
    engine: &Arc<TextEngine>,
    history: HistorySink,
) -> std::io::Result<DictationHandle> {
    start_dictation_with(
        rig.handle(),
        DictationPorts {
            audio: speaking_mic(),
            clipboard: Arc::clone(&rig.clipboard) as _,
            paster: Arc::new(FakePaster::new().with_start_window(Some(StartWindow {
                handle: WindowRef(1),
                process_id: 1,
                elevated: false,
            }))),
            engine_factory: Some(engine_factory(engine)),
            builtin: None,
            indicator: Arc::new(FakeIndicator::new()),
            credentials: Arc::clone(&rig.rig.creds),
            hotkeys: Arc::clone(&rig.rig.hotkeys),
            detector: Err(VadError::Unavailable),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            history,
        },
        Arc::new(FakeNotifier::new()),
    )
}

/// A hold of `HOLD` through the session's inputs.
fn hold(session: &DictationSession) {
    session.hotkey_pressed(now());
    thread::sleep(HOLD);
    session.hotkey_released(now());
}

fn dictation_start_failed_lines(rig: &HistoryRig) -> Vec<String> {
    headed(&rig.rig.log_lines(), "warning")
        .into_iter()
        .filter(|l| {
            l.split_whitespace()
                .any(|w| w == "kind=dictation_start_failed")
        })
        .collect()
}

#[test]
fn a_delivered_dictation_is_listed_by_history_get_once_with_its_text_and_stored() {
    // Acceptance 1 and 2 (review 1 #1 a): `start_history`, then `start_dictation_with(Managed)`
    // (the order `run()` has); one delivered job (a hold, the engine's text, pasted) ends with
    // `history_get` listing exactly that text, once, with the engine label `api`, the file
    // written, the one clipboard write being the dictation's (the history wrote none), and the
    // text nowhere in the log (FR-20). Bite: the pipeline given a keep-nothing sink (nothing
    // listed: the "defaulted no-op"); the sink bound to a recorder of another part; the sink
    // called twice or with a preview (a second row, a changed text); the history text written
    // to the log.
    let _serial = serial();
    assert_hotkey_free();
    let rig = HistoryRig::started();
    let engine = TextEngine::new(TRANSCRIPT);
    let _dictation = start(&rig, &engine, HistorySink::Managed).expect("start_dictation_with");

    hold(&rig.rig.session());

    let window = rig.open_window();
    assert!(
        eventually(|| texts_of(&get(&window)) == vec![TRANSCRIPT.to_string()]),
        "history_get after a delivered job: {} (engine calls {})",
        get(&window),
        engine.calls()
    );
    let view = get(&window);
    assert_eq!(view["entries"][0]["engine"]["kind"], json!("api"), "{view}");
    assert!(
        holds_for(SETTLE, || texts_of(&get(&window)).len() == 1),
        "the job was recorded more than once: {}",
        get(&window)
    );
    assert_eq!(rig.history_files(), vec!["history.json".to_string()]);
    assert_eq!(
        rig.clipboard.texts(),
        vec![TRANSCRIPT.to_string()],
        "the dictation's own write only"
    );
    let leaked: Vec<String> = rig
        .rig
        .log_lines()
        .into_iter()
        .filter(|l| l.contains(CANARY))
        .collect();
    assert!(
        leaked.is_empty(),
        "the transcript reached the log: {leaked:?}"
    );
}

#[test]
fn managed_history_without_a_started_history_part_refuses_the_dictation_start() {
    // Review 1 #1 a (the required sink, P-013; expected RED today: the start succeeds with a
    // keep-nothing sink): `start_dictation_with(Managed)` on an app whose `start_history` did
    // not run (never called, called after, or failed) is `Err` of kind `NotFound`, writes one
    // `warning kind=dictation_start_failed` line, and leaves no `Arc<DictationSession>`
    // managed, so no dictation runs with a history that can never fill. Bite: `Managed`
    // falling back to a keep-nothing sink and returning `Ok` (the shipped behaviour); the
    // error written without the line; a half-started session left managed.
    let _serial = serial();
    assert_hotkey_free();
    let rig = HistoryRig::unstarted();
    let engine = TextEngine::new(TRANSCRIPT);

    let failed = start(&rig, &engine, HistorySink::Managed)
        .err()
        .expect("Managed without a history part must be refused");

    assert_eq!(failed.kind(), std::io::ErrorKind::NotFound);
    assert!(
        eventually(|| dictation_start_failed_lines(&rig).len() == 1),
        "want one dictation_start_failed line: {:?}",
        dictation_start_failed_lines(&rig)
    );
    assert!(
        rig.handle().try_state::<Arc<DictationSession>>().is_none(),
        "a refused start left a session managed"
    );
}

#[test]
fn the_explicit_discard_does_not_feed_a_started_history_and_still_delivers() {
    // Review 1 #1 a (NFR-07: a history that could not start never stops a dictation; the
    // explicit opt-out `run()` takes after a failed `start_history`, and the tests' choice):
    // `start_dictation_with(Discard)` is `Ok`; a hold still delivers (the clipboard gets the
    // text); and the history of the app, started here, is not fed by a `Discard` pipeline
    // (the choice is the caller's, not a fallback of the part's presence). Bite: `Discard`
    // refused like `Managed`; `Discard` recording into the managed part anyway (an alias of
    // `Managed`).
    let _serial = serial();
    assert_hotkey_free();
    let rig = HistoryRig::started();
    let engine = TextEngine::new(TRANSCRIPT);
    let _dictation = start(&rig, &engine, HistorySink::Discard).expect("start_dictation_with");

    hold(&rig.rig.session());

    assert!(
        eventually(|| rig.clipboard.texts() == vec![TRANSCRIPT.to_string()]),
        "the job was not delivered: {:?} (engine calls {})",
        rig.clipboard.texts(),
        engine.calls()
    );
    let window = rig.open_window();
    assert!(
        holds_for(SETTLE, || texts_of(&get(&window)).is_empty()),
        "a Discard pipeline recorded: {}",
        get(&window)
    );
    assert_eq!(rig.history_files(), Vec::<String>::new());
}
