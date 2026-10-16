//! The one owner of a loaded built-in model (spec 002 data-model "ModelResidency",
//! contracts/core-traits.md; T-098; NFR-01, NFR-03).
//!
//! Invariants:
//! - at most one model is in memory; a model is loaded only by [`ModelResidency::acquire`]
//!   or [`ModelResidency::prewarm`], never at start and never by [`ModelResidency::tick`];
//! - a transcription holds its model through the [`ModelGuard`] `acquire` returns; a
//!   held model is never unloaded (idle timer, settings change and delete wait for
//!   the last guard);
//! - every time read comes from the injected [`MonotonicClock`]: the idle deadline
//!   is the end of the last activity (a guard or a [`RecordingGuard`] dropped, a
//!   load finished) plus `idle`, reached when `now >= deadline`; the cold load's
//!   `load_ms` is measured on the same port;
//! - while a [`ReleaseGuard`] from [`ModelRelease::release_for_delete`] lives, its
//!   model is not loaded: `acquire` of it is `NoLocalModel`.
//!
//! Synchronous, std `Mutex` + `Condvar` (decision #42). A load runs outside the
//! lock; a second `acquire` or `prewarm` during it shares it. Models are dropped
//! outside the lock.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::clock::MonotonicClock;
use crate::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use crate::failure::FailureReason;
use crate::local_models::catalog::ModelId;
use crate::local_models::service::{InUse, ModelRelease, ReleaseGuard};

/// Whether a transcription found its model loaded (spec FR-015; the dictation
/// log's `local=cold|warm`, `load_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warmth {
    /// The model was loaded for this transcription (or by the prewarm of its
    /// recording); `load_ms` is the load's duration on the monotonic port.
    Cold { load_ms: u32 },
    /// The model was already loaded.
    Warm,
}

type SharedModel = Arc<Mutex<Box<dyn SpeechModel>>>;

struct Loaded {
    id: ModelId,
    model: SharedModel,
    /// The duration of a prewarm load no transcription has used yet: the first
    /// acquire takes it and is `Cold`.
    fresh_load_ms: Option<u32>,
    /// Unload when the last guard drops (the settings moved away while it was held).
    drop_on_release: bool,
}

struct Loading {
    id: ModelId,
    /// The settings moved away during the load: a prewarm drops the result.
    stale: bool,
}

#[derive(Default)]
struct State {
    loaded: Option<Loaded>,
    loading: Option<Loading>,
    /// One entry per live [`ModelGuard`].
    held: Vec<ModelId>,
    /// Live [`RecordingGuard`]s.
    recordings: usize,
    last_activity_end: Option<Instant>,
    /// One entry per live delete [`ReleaseGuard`].
    blocked: Vec<ModelId>,
}

impl State {
    fn holds(&self, id: ModelId) -> bool {
        self.held.contains(&id)
    }

    /// The loaded model if nothing holds it (its `Arc` dropped by the caller,
    /// outside the lock).
    fn take_unheld(&mut self) -> Option<Loaded> {
        let unheld = self.loaded.as_ref().is_some_and(|l| !self.holds(l.id));
        if unheld {
            self.loaded.take()
        } else {
            None
        }
    }
}

struct Inner {
    loader: Arc<dyn SpeechModelLoader>,
    clock: Arc<dyn MonotonicClock>,
    idle: Duration,
    state: Mutex<State>,
    changed: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs the loader (outside the lock), timing it on the monotonic port.
    fn load(&self, path: &std::path::Path) -> Result<(Box<dyn SpeechModel>, u32), FailureReason> {
        let start = self.clock.now();
        let model = self.loader.load(path).map_err(|e| match e {
            FailureReason::NoLocalModel => FailureReason::NoLocalModel,
            _ => FailureReason::LocalEngineFailed,
        })?;
        let took = self.clock.now().saturating_duration_since(start);
        let load_ms = u32::try_from(took.as_millis()).unwrap_or(u32::MAX);
        Ok((model, load_ms))
    }
}

/// The claim on the one in-flight load. Dropped without [`disarm`](Self::disarm)
/// (a failed load, a panicking loader, a prewarm thread that did not start), it
/// clears the claim and wakes the waiters, so no acquire waits forever.
struct LoadClaim {
    inner: Arc<Inner>,
    armed: bool,
}

impl LoadClaim {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for LoadClaim {
    fn drop(&mut self) {
        if self.armed {
            self.inner.lock().loading = None;
            self.inner.changed.notify_all();
        }
    }
}

/// The residency (one per app, shared by every built-in engine and the delete).
pub struct ModelResidency {
    inner: Arc<Inner>,
}

impl ModelResidency {
    /// An empty residency: nothing is loaded until the first `acquire` or `prewarm`.
    /// `idle` is [`crate::timeouts::MODEL_IDLE_UNLOAD`] in production.
    pub fn new(
        loader: Arc<dyn SpeechModelLoader>,
        clock: Arc<dyn MonotonicClock>,
        idle: Duration,
    ) -> ModelResidency {
        ModelResidency {
            inner: Arc::new(Inner {
                loader,
                clock,
                idle,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    /// The monotonic port the residency reads (the built-in engine's budget uses
    /// the same one).
    pub(crate) fn clock(&self) -> Arc<dyn MonotonicClock> {
        Arc::clone(&self.inner.clock)
    }

    /// The model `id` (file `path`), held until the guard drops. Loads it when
    /// another or no model is loaded (dropping the other one first, once nothing
    /// holds it), waits for a load in flight. `Cold` with the load's duration for
    /// the transcription that loaded it (or the first after its prewarm), `Warm`
    /// otherwise. `NoLocalModel` while a delete of `id` holds its guard; a load
    /// error is `LocalEngineFailed` and leaves the residency empty.
    pub fn acquire(
        &self,
        id: ModelId,
        path: PathBuf,
    ) -> Result<(ModelGuard, Warmth), FailureReason> {
        let inner = &self.inner;
        let mut st = inner.lock();
        loop {
            if st.blocked.contains(&id) {
                return Err(FailureReason::NoLocalModel);
            }
            let state = &mut *st;
            if let Some(l) = state
                .loaded
                .as_mut()
                .filter(|l| l.id == id && !l.drop_on_release)
            {
                let warmth = match l.fresh_load_ms.take() {
                    Some(load_ms) => Warmth::Cold { load_ms },
                    None => Warmth::Warm,
                };
                let model = Arc::clone(&l.model);
                state.held.push(id);
                return Ok((self.guard(id, model), warmth));
            }
            let other_held = st.loaded.as_ref().is_some_and(|l| st.holds(l.id));
            if st.loading.is_some() || other_held {
                st = inner.wait(st);
                continue;
            }
            break;
        }
        let old = st.loaded.take();
        st.loading = Some(Loading { id, stale: false });
        let claim = LoadClaim {
            inner: Arc::clone(inner),
            armed: true,
        };
        drop(st);
        drop(old);

        let (model, load_ms) = inner.load(&path)?;
        let model: SharedModel = Arc::new(Mutex::new(model));
        let mut st = inner.lock();
        let stale = st.loading.take().is_some_and(|l| l.stale);
        claim.disarm();
        st.loaded = Some(Loaded {
            id,
            model: Arc::clone(&model),
            fresh_load_ms: None,
            drop_on_release: stale,
        });
        st.held.push(id);
        st.last_activity_end = Some(inner.clock.now());
        drop(st);
        inner.changed.notify_all();
        Ok((self.guard(id, model), Warmth::Cold { load_ms }))
    }

    fn guard(&self, id: ModelId, model: SharedModel) -> ModelGuard {
        ModelGuard {
            inner: Arc::clone(&self.inner),
            id,
            model,
        }
    }

    /// Starts loading `id` on its own thread and returns at once (spec FR-015: the
    /// load runs while the user speaks). Does nothing when `id` is loaded, a load
    /// is in flight, another model is held, or a delete of `id` is running.
    pub fn prewarm(&self, id: ModelId, path: PathBuf) {
        let inner = &self.inner;
        let mut st = inner.lock();
        if st.blocked.contains(&id) || st.loading.is_some() {
            return;
        }
        if let Some(l) = &st.loaded {
            if (l.id == id && !l.drop_on_release) || st.holds(l.id) {
                return;
            }
        }
        let old = st.loaded.take();
        st.loading = Some(Loading { id, stale: false });
        let claim = LoadClaim {
            inner: Arc::clone(inner),
            armed: true,
        };
        drop(st);
        drop(old);

        let worker = Arc::clone(inner);
        // A spawn error drops the closure and with it the claim: nothing loads.
        let _ = std::thread::Builder::new()
            .name("voicen-model-prewarm".to_string())
            .spawn(move || {
                let Ok((model, load_ms)) = worker.load(&path) else {
                    return; // the claim's drop clears the load
                };
                let mut st = worker.lock();
                let stale = st.loading.take().is_some_and(|l| l.stale);
                claim.disarm();
                let unused = if stale {
                    Some(model)
                } else {
                    st.loaded = Some(Loaded {
                        id,
                        model: Arc::new(Mutex::new(model)),
                        fresh_load_ms: Some(load_ms),
                        drop_on_release: false,
                    });
                    st.last_activity_end = Some(worker.clock.now());
                    None
                };
                drop(st);
                worker.changed.notify_all();
                drop(unused);
            });
    }

    /// Holds the idle countdown while the recording runs; its drop is an activity
    /// end (spec FR-013: never unload while recording).
    pub fn recording_started(&self) -> RecordingGuard {
        self.inner.lock().recordings += 1;
        RecordingGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The engine or the built-in model changed in the settings: a loaded model
    /// other than `selected` is unloaded at once (when its last guard drops if a
    /// transcription holds it); a load in flight of another model is discarded.
    pub fn on_engine_or_model_changed(&self, selected: Option<ModelId>) {
        let mut st = self.inner.lock();
        if let Some(loading) = st.loading.as_mut() {
            if Some(loading.id) != selected {
                loading.stale = true;
            }
        }
        let other = st.loaded.as_ref().is_some_and(|l| Some(l.id) != selected);
        let mut unloaded = None;
        if other {
            unloaded = st.take_unheld();
            if let Some(l) = st.loaded.as_mut() {
                l.drop_on_release = true;
            }
        }
        drop(st);
        drop(unloaded);
    }

    /// The idle timer: unloads the model when nothing holds it, no recording runs
    /// and `now >= last activity end + idle`. Never loads.
    pub fn tick(&self) {
        let inner = &self.inner;
        let mut st = inner.lock();
        let quiet =
            st.recordings == 0 && st.loaded.as_ref().is_some_and(|l| !st.held.contains(&l.id));
        let due = st
            .last_activity_end
            .and_then(|end| end.checked_add(inner.idle))
            .is_some_and(|deadline| inner.clock.now() >= deadline);
        let unloaded = if quiet && due { st.loaded.take() } else { None };
        drop(st);
        drop(unloaded);
    }

    /// The model in memory, if any.
    pub fn loaded(&self) -> Option<ModelId> {
        self.inner.lock().loaded.as_ref().map(|l| l.id)
    }
}

impl ModelRelease for ModelResidency {
    /// `InUse` while a transcription holds `id` or a load of it runs; otherwise
    /// unloads `id` if it is loaded and refuses its loading until the guard drops.
    fn release_for_delete(&self, id: ModelId) -> Result<ReleaseGuard, InUse> {
        let mut st = self.inner.lock();
        if st.holds(id) || st.loading.as_ref().is_some_and(|l| l.id == id) {
            return Err(InUse);
        }
        let is_loaded = st.loaded.as_ref().is_some_and(|l| l.id == id);
        let unloaded = if is_loaded { st.loaded.take() } else { None };
        st.blocked.push(id);
        drop(st);
        drop(unloaded);
        Ok(ReleaseGuard::new(Unblock {
            inner: Arc::clone(&self.inner),
            id,
        }))
    }
}

/// Lifts one delete's load block when the delete's `ReleaseGuard` drops.
struct Unblock {
    inner: Arc<Inner>,
    id: ModelId,
}

impl Drop for Unblock {
    fn drop(&mut self) {
        let mut st = self.inner.lock();
        if let Some(at) = st.blocked.iter().position(|b| *b == self.id) {
            st.blocked.swap_remove(at);
        }
    }
}

/// A transcription's hold on the loaded model; dropping it is an activity end.
pub struct ModelGuard {
    inner: Arc<Inner>,
    id: ModelId,
    model: SharedModel,
}

impl ModelGuard {
    /// Runs the held model (see [`SpeechModel::transcribe`]).
    pub fn transcribe(
        &self,
        pcm: &[f32],
        language: Option<&str>,
        abort: &Abort,
    ) -> Result<String, FailureReason> {
        self.model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transcribe(pcm, language, abort)
    }
}

impl Drop for ModelGuard {
    fn drop(&mut self) {
        let inner = &self.inner;
        let mut st = inner.lock();
        if let Some(at) = st.held.iter().position(|h| *h == self.id) {
            st.held.swap_remove(at);
        }
        st.last_activity_end = Some(inner.clock.now());
        let doomed = st
            .loaded
            .as_ref()
            .is_some_and(|l| l.drop_on_release && l.id == self.id);
        let unloaded = if doomed { st.take_unheld() } else { None };
        drop(st);
        inner.changed.notify_all();
        drop(unloaded);
    }
}

/// A running recording's hold on the idle countdown.
pub struct RecordingGuard {
    inner: Arc<Inner>,
}

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        let inner = &self.inner;
        let mut st = inner.lock();
        st.recordings = st.recordings.saturating_sub(1);
        st.last_activity_end = Some(inner.clock.now());
    }
}
