//! T-098 (spec 002 T019; NFR-03, NFR-01, spec FR-013–FR-015, FR-021–FR-023): the
//! `ModelResidency` on a fake monotonic clock. Acceptance "Model unloads exactly
//! 10 min after the last activity (fake clock); never while recording or
//! transcribing".
//!
//! Pinned API (`voicen_core::local_models::residency`, analysis seam (3); the
//! contract's `async acquire`, `Arc<dyn Clock>` and `EngineError` are stale):
//! - `ModelResidency::new(loader: Arc<dyn SpeechModelLoader>, clock: Arc<dyn
//!   MonotonicClock>, idle: Duration) -> ModelResidency`;
//! - `acquire(&self, id: ModelId, path: PathBuf) -> Result<(guard, Warmth),
//!   FailureReason>`: loads `path` if `id` is not loaded (waiting for an in-flight
//!   load of it), holds the model until the guard drops; a load error leaves the
//!   residency empty;
//! - `pub enum Warmth { Cold { load_ms: u32 }, Warm }` (`Debug + PartialEq`);
//! - `prewarm(&self, id: ModelId, path: PathBuf)`: starts the load and returns
//!   without waiting for it; idempotent;
//! - `recording_started(&self) -> guard`: holds the countdown while it lives;
//! - `on_engine_or_model_changed(&self, selected: Option<ModelId>)`: unloads at once
//!   unless `selected` is the loaded model;
//! - `tick(&self)`: unloads when the idle deadline (last activity end + `idle`) is
//!   reached; `loaded(&self) -> Option<ModelId>`;
//! - `impl ModelRelease for ModelResidency` (T-019's port): `InUse` while a guard
//!   from `acquire` lives; otherwise unloads at once and, while the returned
//!   `ReleaseGuard` lives, an `acquire` of that id loads nothing and fails with
//!   `FailureReason::NoLocalModel` (the model is being deleted);
//! - `voicen_core::timeouts::MODEL_IDLE_UNLOAD: Duration` = 600 s.
//!
//! Every instant the residency reads comes from `local_support::FakeMono`; the
//! tests never sleep and hold no wall-clock bound (docs/decisions/core-tests.md).

mod common;
mod local_support;

use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use local_support::{FakeLoader, FakeMono, Script};
use voicen_core::clock::MonotonicClock;
use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::residency::{ModelResidency, Warmth};
use voicen_core::local_models::service::{InUse, ModelRelease};
use voicen_core::timeouts::MODEL_IDLE_UNLOAD;

/// Ends a hung test only (a prewarm that blocks on its own load); never a verdict
/// on time.
const HANG: Duration = Duration::from_secs(30);
const TEN_MIN: Duration = Duration::from_secs(600);
const ONE_MS: Duration = Duration::from_millis(1);

fn path_of(id: ModelId) -> PathBuf {
    PathBuf::from(format!("models/ggml-{}.bin", id.as_str()))
}

struct Rig {
    clock: Arc<FakeMono>,
    loader: Arc<FakeLoader>,
    residency: Arc<ModelResidency>,
}

fn rig() -> Rig {
    rig_with(Script::default())
}

fn rig_with(script: Script) -> Rig {
    let clock = FakeMono::starting_at(common::timing::now());
    let loader = FakeLoader::with(&clock, script);
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    Rig {
        clock,
        loader,
        residency,
    }
}

impl Rig {
    /// One whole activity: acquire `id`, hold it for `hold` of fake time, release.
    fn use_for(&self, id: ModelId, hold: Duration) -> Warmth {
        let (guard, warmth) = self
            .residency
            .acquire(id, path_of(id))
            .unwrap_or_else(|e| panic!("acquire {id:?}: {e:?}"));
        self.clock.advance(hold);
        drop(guard);
        warmth
    }

    fn tick_after(&self, d: Duration) {
        self.clock.advance(d);
        self.residency.tick();
    }
}

#[test]
fn idle_unload_is_ten_minutes() {
    // NFR-03 (r1#13): 10 min idle. Bite: another constant.
    assert_eq!(MODEL_IDLE_UNLOAD, TEN_MIN);
}

#[test]
fn nothing_is_loaded_at_start_and_ticks_load_nothing() {
    // Spec FR-014 / data-model "never prewarm at app start": a new residency holds
    // nothing, and a timer firing for an hour loads nothing. Bite: a load in `new`,
    // or `tick` loading the selected model.
    let r = rig();
    assert_eq!(r.residency.loaded(), None);
    for _ in 0..6 {
        r.tick_after(TEN_MIN);
    }
    assert_eq!(r.residency.loaded(), None);
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "no load at start");
}

#[test]
fn the_first_acquire_is_cold_with_its_load_time_and_the_next_is_warm() {
    // Spec FR-015 / NFR-01: the dictation that loaded the model is Cold with the
    // load's duration on the injected clock (the fake load takes 1234 ms of fake
    // time); the next one finds it loaded: Warm, no second load. Bite: always
    // Warm/Cold, load_ms from the wall clock (~0), a reload per acquire, the path
    // not passed to the loader.
    let r = rig();
    assert_eq!(
        r.use_for(ModelId::Small, Duration::from_secs(3)),
        Warmth::Cold { load_ms: 1_234 }
    );
    assert_eq!(r.residency.loaded(), Some(ModelId::Small));
    assert_eq!(
        r.use_for(ModelId::Small, Duration::from_secs(3)),
        Warmth::Warm
    );
    assert_eq!(r.loader.loads(), vec![path_of(ModelId::Small)]);
    assert_eq!(r.loader.live(), 1);
}

#[test]
fn unloads_exactly_600_s_after_the_last_activity_end_and_not_a_millisecond_before() {
    // Acceptance 1. The transcription holds the model for 5 s; the countdown starts
    // when it ends. At +599.999 s the model is still loaded, at +600 s it is
    // dropped. Bite: a deadline from the acquire or the load (unloads 5 s early), a
    // longer or shorter idle, `>` vs `>=` mixed up the other way round (still
    // loaded at 600 s), unload without dropping the model.
    let r = rig();
    r.use_for(ModelId::Tiny, Duration::from_secs(5));

    r.tick_after(TEN_MIN - ONE_MS);
    assert_eq!(r.residency.loaded(), Some(ModelId::Tiny), "at 599.999 s");
    assert_eq!(r.loader.dropped(), 0, "at 599.999 s");

    r.tick_after(ONE_MS);
    assert_eq!(r.residency.loaded(), None, "at 600 s");
    assert_eq!(r.loader.dropped(), 1, "the model is dropped at 600 s");
    assert_eq!(r.loader.live(), 0);
}

#[test]
fn a_later_activity_restarts_the_countdown() {
    // "10 min after the LAST activity": a second dictation 5 min later moves the
    // deadline. Bite: the deadline kept from the first activity or the load.
    let r = rig();
    r.use_for(ModelId::Tiny, Duration::from_secs(1));
    r.tick_after(Duration::from_secs(300));
    assert_eq!(
        r.use_for(ModelId::Tiny, Duration::from_secs(1)),
        Warmth::Warm
    );

    r.tick_after(TEN_MIN - ONE_MS);
    assert_eq!(r.residency.loaded(), Some(ModelId::Tiny));
    r.tick_after(ONE_MS);
    assert_eq!(r.residency.loaded(), None);
    assert_eq!(r.loader.loads().len(), 1);
}

#[test]
fn never_unloads_while_a_transcription_holds_the_model() {
    // Acceptance 1, "never while transcribing": an hour-long hold survives every
    // tick; the countdown starts when the guard drops. Bite: unload by deadline
    // regardless of the guard, a deadline from the acquire.
    let r = rig();
    let (guard, _) = r
        .residency
        .acquire(ModelId::Base, path_of(ModelId::Base))
        .expect("acquire");
    for _ in 0..6 {
        r.tick_after(TEN_MIN);
    }
    assert_eq!(r.residency.loaded(), Some(ModelId::Base), "held");
    assert_eq!(r.loader.dropped(), 0, "held");
    drop(guard);

    r.tick_after(TEN_MIN - ONE_MS);
    assert_eq!(
        r.residency.loaded(),
        Some(ModelId::Base),
        "599.999 s after release"
    );
    r.tick_after(ONE_MS);
    assert_eq!(r.residency.loaded(), None, "600 s after release");
    assert_eq!(r.loader.dropped(), 1);
}

#[test]
fn never_unloads_while_a_recording_is_active() {
    // Acceptance 1, "never while recording": a long recording (Toggle mode) holds
    // the countdown; it restarts when the recording ends. Bite: recording_started
    // ignored, or its guard's drop not counted as the activity end.
    let r = rig();
    r.use_for(ModelId::Tiny, Duration::from_secs(1));
    r.tick_after(Duration::from_secs(590));

    let recording = r.residency.recording_started();
    for _ in 0..6 {
        r.tick_after(TEN_MIN);
    }
    assert_eq!(r.residency.loaded(), Some(ModelId::Tiny), "while recording");
    drop(recording);

    r.tick_after(TEN_MIN - ONE_MS);
    assert_eq!(
        r.residency.loaded(),
        Some(ModelId::Tiny),
        "599.999 s after the recording"
    );
    r.tick_after(ONE_MS);
    assert_eq!(r.residency.loaded(), None, "600 s after the recording");
}

#[test]
fn release_for_delete_while_a_transcription_holds_the_model_is_in_use() {
    // T-019's port with the real residency: a held model is refused with InUse and
    // stays loaded; once the guard drops the release unloads it. Bite: a release
    // that ignores the guard (unloads a model in use), InUse forever.
    let r = rig();
    let (guard, _) = r
        .residency
        .acquire(ModelId::Small, path_of(ModelId::Small))
        .expect("acquire");
    match r.residency.release_for_delete(ModelId::Small) {
        Err(InUse) => {}
        Ok(_) => panic!("release_for_delete of a held model must be InUse"),
    }
    assert_eq!(r.residency.loaded(), Some(ModelId::Small));
    assert_eq!(r.loader.dropped(), 0);
    drop(guard);

    let release = r.residency.release_for_delete(ModelId::Small);
    assert!(release.is_ok(), "released after the guard dropped");
    assert_eq!(r.residency.loaded(), None, "unloaded by the release");
    assert_eq!(r.loader.dropped(), 1);
}

#[test]
fn release_for_delete_unloads_at_once_and_no_load_of_that_model_until_its_guard_drops() {
    // Spec FR-021–FR-023 / T-019: the release unloads at once (no waiting for the
    // idle timer), and while the delete holds its guard an acquire of that model
    // loads nothing and answers NoLocalModel; after the guard another model or the
    // same one loads normally. Bite: release not unloading, a load during the
    // delete (a load of a file being removed), the block never lifted.
    let r = rig();
    r.use_for(ModelId::Small, Duration::from_secs(1));
    let release = match r.residency.release_for_delete(ModelId::Small) {
        Ok(g) => g,
        Err(InUse) => panic!("nothing holds the model"),
    };
    assert_eq!(r.residency.loaded(), None);
    assert_eq!(r.loader.dropped(), 1);

    let during = r
        .residency
        .acquire(ModelId::Small, path_of(ModelId::Small))
        .err();
    assert_eq!(during, Some(FailureReason::NoLocalModel));
    assert_eq!(r.loader.loads().len(), 1, "no load during the delete");

    drop(release);
    assert_eq!(
        r.use_for(ModelId::Small, Duration::from_secs(1)),
        Warmth::Cold { load_ms: 1_234 }
    );
    assert_eq!(r.loader.loads().len(), 2);
}

#[test]
fn release_for_delete_of_another_model_leaves_the_loaded_one() {
    // Only the named model is released: deleting `base` while `tiny` is held is
    // not InUse and does not unload `tiny`. Bite: InUse for any held model, an
    // unload of whatever is loaded.
    let r = rig();
    let (guard, _) = r
        .residency
        .acquire(ModelId::Tiny, path_of(ModelId::Tiny))
        .expect("acquire");
    let release = r.residency.release_for_delete(ModelId::Base);
    assert!(release.is_ok(), "base is not in use");
    assert_eq!(r.residency.loaded(), Some(ModelId::Tiny));
    assert_eq!(r.loader.dropped(), 0);
    drop(release);
    drop(guard);
}

#[test]
fn a_load_error_leaves_the_residency_empty_and_the_next_acquire_loads_again() {
    // T019 "a load error leaves the residency empty": the error is the engine
    // failure, nothing is loaded, the timer has nothing to unload, and the next
    // acquire tries again. Bite: a cached failure, a half-loaded entry reported as
    // loaded, a panic.
    let r = rig_with(Script {
        load_fails: true,
        ..Script::default()
    });
    let got = r
        .residency
        .acquire(ModelId::Tiny, path_of(ModelId::Tiny))
        .err();
    assert_eq!(got, Some(FailureReason::LocalEngineFailed));
    assert_eq!(r.residency.loaded(), None);
    r.tick_after(TEN_MIN);
    assert_eq!(r.residency.loaded(), None);

    r.loader.script(|s| s.load_fails = false);
    assert_eq!(
        r.use_for(ModelId::Tiny, Duration::from_secs(1)),
        Warmth::Cold { load_ms: 1_234 }
    );
    assert_eq!(r.loader.loads().len(), 2);
}

#[test]
fn an_engine_or_model_change_unloads_a_different_model_at_once() {
    // Spec FR-013 / T019 "unloads at once on engine change, on another model
    // selected": same model -> kept; another model or no built-in model -> dropped
    // without waiting for the timer. Bite: change ignored, unload of the selected
    // model, unload only by the timer.
    let r = rig();
    r.use_for(ModelId::Tiny, Duration::from_secs(1));
    r.residency.on_engine_or_model_changed(Some(ModelId::Tiny));
    assert_eq!(r.residency.loaded(), Some(ModelId::Tiny), "same model kept");

    r.residency.on_engine_or_model_changed(Some(ModelId::Base));
    assert_eq!(r.residency.loaded(), None, "another model selected");
    assert_eq!(r.loader.dropped(), 1);

    r.use_for(ModelId::Tiny, Duration::from_secs(1));
    r.residency.on_engine_or_model_changed(None);
    assert_eq!(
        r.residency.loaded(),
        None,
        "engine changed away from built-in"
    );
    assert_eq!(r.loader.dropped(), 2);
}

#[test]
fn acquiring_another_model_replaces_the_loaded_one() {
    // One model in memory at a time: the residency owns one loaded model, so a job
    // with another model drops the old one. Bite: two models kept (RAM doubles), the
    // old model handed out for the new id.
    let r = rig();
    r.use_for(ModelId::Tiny, Duration::from_secs(1));
    assert_eq!(
        r.use_for(ModelId::Base, Duration::from_secs(1)),
        Warmth::Cold { load_ms: 1_234 }
    );
    assert_eq!(r.residency.loaded(), Some(ModelId::Base));
    assert_eq!(r.loader.live(), 1, "only one model in memory");
    assert_eq!(
        r.loader.loads(),
        vec![path_of(ModelId::Tiny), path_of(ModelId::Base)]
    );
}

#[test]
fn prewarm_returns_before_its_load_ends_and_an_acquire_uses_that_one_load() {
    // Spec FR-015: the load starts with the recording, in parallel with it, so
    // prewarm must not block the caller (the hotkey path); a second prewarm and a
    // transcription arriving during the load share it (one load). Bite: a
    // synchronous prewarm (never returns while the load is held: the test fails at
    // HANG), a second load per prewarm or per acquire.
    let r = rig();
    let gate = r.loader.gate();

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let residency = Arc::clone(&r.residency);
    std::thread::spawn(move || {
        residency.prewarm(ModelId::Small, path_of(ModelId::Small));
        let _ = done_tx.send(());
    });
    let entered = gate.entered.recv_timeout(HANG);
    assert_eq!(
        entered,
        Ok(path_of(ModelId::Small)),
        "prewarm started the load"
    );
    let returned = done_rx.recv_timeout(HANG);
    if returned == Err(RecvTimeoutError::Timeout) {
        let _ = gate.release.send(());
        panic!("prewarm blocked until its load ended");
    }

    // A second prewarm during the load starts nothing.
    r.residency.prewarm(ModelId::Small, path_of(ModelId::Small));

    let residency = Arc::clone(&r.residency);
    let job = std::thread::spawn(move || {
        residency
            .acquire(ModelId::Small, path_of(ModelId::Small))
            .map(|(_guard, warmth)| warmth)
    });
    let _ = gate.release.send(());
    drop(gate);
    let got = job.join().expect("acquire must not panic");
    assert!(got.is_ok(), "the acquire got the prewarmed model: {got:?}");
    assert_eq!(r.loader.loads(), vec![path_of(ModelId::Small)], "one load");
    assert_eq!(r.residency.loaded(), Some(ModelId::Small));
}
