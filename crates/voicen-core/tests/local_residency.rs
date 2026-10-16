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
//! tests hold no wall-clock bound (docs/decisions/core-tests.md). One test sleeps
//! `SETTLE` to let a waiter reach its wait (T-098 review 1 #6); a slow host only
//! makes that test weaker, never red.
//!
//! T-098 review 1 (#3, #5; research R-8, NFR-07): the residency checks the file
//! before every load, the prewarm's included: a missing file is `NoLocalModel`, a
//! file without the ggml magic (`0x67676d6c` little-endian, `b"lmgg"`) is
//! `LocalEngineFailed`; neither reaches the loader (whisper.cpp may abort the
//! process on a malformed file and `panic = "abort"` makes that uncatchable).

mod common;
mod local_support;

use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use local_support::{file_name, place_models, wrong_magic_bytes, FakeLoader, FakeMono, Script};
use voicen_core::clock::MonotonicClock;
use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::residency::{ModelResidency, Warmth};
use voicen_core::local_models::service::{InUse, ModelRelease};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::MODEL_IDLE_UNLOAD;

/// Ends a hung test only (a prewarm that blocks on its own load); never a verdict
/// on time.
const HANG: Duration = Duration::from_secs(30);
const TEN_MIN: Duration = Duration::from_secs(600);
const ONE_MS: Duration = Duration::from_millis(1);
/// Lets a spawned acquire reach its wait for a load in flight. Too short only
/// weakens `a_failed_prewarm_wakes_an_acquire_waiting_on_it`; it never turns it red.
const SETTLE: Duration = Duration::from_millis(300);

struct Rig {
    _tmp: TempDir,
    /// Holds a ggml model file (`local_support::ggml_bytes`) of every `ModelId`:
    /// the residency may check the file before its load (R-8).
    dir: PathBuf,
    clock: Arc<FakeMono>,
    loader: Arc<FakeLoader>,
    residency: Arc<ModelResidency>,
}

fn rig() -> Rig {
    rig_with(Script::default())
}

fn rig_with(script: Script) -> Rig {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    place_models(&dir, &ModelId::ALL);
    let clock = FakeMono::starting_at(common::timing::now());
    let loader = FakeLoader::with(&clock, script);
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    Rig {
        _tmp: tmp,
        dir,
        clock,
        loader,
        residency,
    }
}

impl Rig {
    fn path(&self, id: ModelId) -> PathBuf {
        self.dir.join(file_name(id))
    }

    /// One whole activity: acquire `id`, hold it for `hold` of fake time, release.
    fn use_for(&self, id: ModelId, hold: Duration) -> Warmth {
        let (guard, warmth) = self
            .residency
            .acquire(id, self.path(id))
            .unwrap_or_else(|e| panic!("acquire {id:?}: {e:?}"));
        self.clock.advance(hold);
        drop(guard);
        warmth
    }

    /// `acquire(id)` on its own thread, which waits for the prewarm's load claim;
    /// the error it ends with. A claim never released fails the test at `HANG`.
    fn acquire_err_after_prewarm(&self, id: ModelId) -> Option<FailureReason> {
        let residency = Arc::clone(&self.residency);
        let path = self.path(id);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(residency.acquire(id, path).err());
        });
        match rx.recv_timeout(HANG) {
            Ok(got) => got,
            Err(e) => panic!("the acquire after the prewarm never returned: {e:?}"),
        }
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
    assert_eq!(r.loader.loads(), vec![r.path(ModelId::Small)]);
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
        .acquire(ModelId::Base, r.path(ModelId::Base))
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
        .acquire(ModelId::Small, r.path(ModelId::Small))
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
        .acquire(ModelId::Small, r.path(ModelId::Small))
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
        .acquire(ModelId::Tiny, r.path(ModelId::Tiny))
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
        .acquire(ModelId::Tiny, r.path(ModelId::Tiny))
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
        vec![r.path(ModelId::Tiny), r.path(ModelId::Base)]
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
    let small = r.path(ModelId::Small);
    std::thread::spawn(move || {
        residency.prewarm(ModelId::Small, small);
        let _ = done_tx.send(());
    });
    let entered = gate.entered.recv_timeout(HANG);
    assert_eq!(
        entered,
        Ok(r.path(ModelId::Small)),
        "prewarm started the load"
    );
    let returned = done_rx.recv_timeout(HANG);
    if returned == Err(RecvTimeoutError::Timeout) {
        let _ = gate.release.send(());
        panic!("prewarm blocked until its load ended");
    }

    // A second prewarm during the load starts nothing.
    r.residency.prewarm(ModelId::Small, r.path(ModelId::Small));

    let residency = Arc::clone(&r.residency);
    let small = r.path(ModelId::Small);
    let job = std::thread::spawn(move || {
        residency
            .acquire(ModelId::Small, small)
            .map(|(_guard, warmth)| warmth)
    });
    let _ = gate.release.send(());
    drop(gate);
    let got = job.join().expect("acquire must not panic");
    assert!(got.is_ok(), "the acquire got the prewarmed model: {got:?}");
    assert_eq!(r.loader.loads(), vec![r.path(ModelId::Small)], "one load");
    assert_eq!(r.residency.loaded(), Some(ModelId::Small));
}

// ---- T-098 review 1 -------------------------------------------------------------------

#[test]
fn a_failed_prewarm_wakes_an_acquire_waiting_on_it() {
    // Review 1 #6: an acquire that arrives while a prewarm load is in flight waits
    // for it; when that load fails the claim's drop must clear it and wake the
    // waiter, which then loads again itself. Bite: no `notify_all` in
    // `LoadClaim::drop` (the waiter sleeps forever: no second load reaches the gate
    // within HANG), the failed load left in flight, the prewarm's failure cached.
    let r = rig_with(Script {
        load_fails: true,
        ..Script::default()
    });
    let gate = r.loader.gate();
    r.residency.prewarm(ModelId::Small, r.path(ModelId::Small));
    assert_eq!(
        gate.entered.recv_timeout(HANG),
        Ok(r.path(ModelId::Small)),
        "the prewarm load started"
    );

    let residency = Arc::clone(&r.residency);
    let small = r.path(ModelId::Small);
    let job = std::thread::spawn(move || {
        residency
            .acquire(ModelId::Small, small)
            .map(|(_guard, warmth)| warmth)
    });
    std::thread::sleep(SETTLE);
    gate.release.send(()).expect("release the prewarm load");

    let second = gate.entered.recv_timeout(HANG);
    assert_eq!(
        second,
        Ok(r.path(ModelId::Small)),
        "the waiting acquire was never woken after the prewarm load failed"
    );
    r.loader.script(|s| s.load_fails = false);
    drop(gate);
    let got = job.join().expect("acquire must not panic");
    assert_eq!(got, Ok(Warmth::Cold { load_ms: 1_234 }));
    assert_eq!(r.loader.loads().len(), 2, "the prewarm load and the retry");
    assert_eq!(r.residency.loaded(), Some(ModelId::Small));
}

#[test]
fn a_file_without_the_ggml_magic_is_local_engine_failed_and_never_reaches_the_loader() {
    // Review 1 #3 / R-8 / NFR-07 (spec 002 T025 "size and ggml-magic check before
    // load"): a file of the right size whose first four bytes are not the ggml
    // magic, a file shorter than the magic, and an empty file are refused before
    // the loader runs; the failure is the engine failure (spec FR-012), nothing is
    // loaded, and a good file loads normally afterwards. Bite: no check (the loader
    // gets the file), the magic compared in the wrong byte order (`b"ggml"`
    // accepted, the real `b"lmgg"` refused), a check of the size only, a cached
    // refusal.
    let r = rig();
    let bad: [(&str, Vec<u8>); 3] = [
        ("wrong magic", wrong_magic_bytes()),
        ("shorter than the magic", b"lmg".to_vec()),
        ("empty", Vec::new()),
    ];
    for (what, bytes) in bad {
        std::fs::write(r.path(ModelId::Base), &bytes).expect("write the bad file");
        let got = r
            .residency
            .acquire(ModelId::Base, r.path(ModelId::Base))
            .err();
        assert_eq!(got, Some(FailureReason::LocalEngineFailed), "{what}");
        assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "{what}: no load");
        assert_eq!(r.residency.loaded(), None, "{what}");
    }

    assert_eq!(
        r.use_for(ModelId::Tiny, Duration::from_secs(1)),
        Warmth::Cold { load_ms: 1_234 },
        "a good file still loads"
    );
    assert_eq!(r.loader.loads(), vec![r.path(ModelId::Tiny)]);
}

#[test]
fn a_prewarm_of_a_file_without_the_ggml_magic_never_reaches_the_loader() {
    // Review 1 #3 / R-8: the prewarm loads too (on its own thread, while the user
    // speaks), so the check covers it as well. The acquire after it waits for any
    // load in flight, so once it returns the prewarm is over. Bite: the check only
    // in `acquire` or only in `BuiltinEngine` (the prewarm loads the bad file).
    let r = rig();
    std::fs::write(r.path(ModelId::Small), wrong_magic_bytes()).expect("write the bad file");
    r.residency.prewarm(ModelId::Small, r.path(ModelId::Small));
    let got = r.acquire_err_after_prewarm(ModelId::Small);
    assert_eq!(got, Some(FailureReason::LocalEngineFailed));
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "no load at all");
    assert_eq!(r.residency.loaded(), None);
}

#[test]
fn a_model_file_gone_before_the_load_is_no_local_model_without_a_load() {
    // Review 1 #5: a delete that completes between the engine's path resolution
    // and the acquire leaves no file and no delete block; the acquire must answer
    // NoLocalModel (the model is not downloaded, spec FR-011), not the engine
    // failure, and the loader is never asked for a missing path. A prewarm of a
    // missing file loads nothing either. Bite: the missing file handed to the
    // loader (its error mapped to LocalEngineFailed), NotFound mapped to
    // LocalEngineFailed by the file check.
    let r = rig();
    std::fs::remove_file(r.path(ModelId::Base)).expect("delete base");
    r.residency.prewarm(ModelId::Base, r.path(ModelId::Base));
    let got = r.acquire_err_after_prewarm(ModelId::Base);
    assert_eq!(got, Some(FailureReason::NoLocalModel));
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "no load");
    assert_eq!(r.residency.loaded(), None);
}
