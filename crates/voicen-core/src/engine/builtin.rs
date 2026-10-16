//! The built-in engine (spec 002 contracts/core-traits.md "BuiltinEngine", T-098;
//! FR-07, FR-24, NFR-01; decisions #42, #133).
//!
//! Built by [`super::engine_for`] for `EngineKind::BuiltinLocal` from the job's
//! settings snapshot and the shared [`BuiltinDeps`]; it reads no key and loads
//! nothing when built. At `transcribe` it resolves the snapshot's
//! `builtin_local.model_id` through [`ModelStore::path_if_downloaded`] (none ->
//! `NoLocalModel`, nothing loaded), takes the model from the one
//! [`ModelResidency`], and runs it with an [`Abort`](super::speech_model::Abort) hook that turns true once
//! `req.timeouts.builtin` has passed on the residency's monotonic port, counted from
//! when the job holds the model (after the acquire and after any other job on the
//! same model): the limit covers the transcription only, never the load or a wait
//! (#133). A run the hook stopped is `Timeout`; any other load or model error is
//! `LocalEngineFailed`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex, PoisonError};

use super::{Engine, TranscribeRequest};
use crate::audio::AudioBuffer;
use crate::failure::FailureReason;
use crate::local_models::catalog::ModelId;
use crate::local_models::residency::{ModelResidency, Warmth};
use crate::local_models::store::ModelStore;

/// What the built-in engine needs besides the settings: the app's one residency
/// and the model store. Cheap to clone (two `Arc`s); the shell builds it once.
#[derive(Clone)]
pub struct BuiltinDeps {
    pub residency: Arc<ModelResidency>,
    pub store: Arc<ModelStore>,
}

/// whisper.cpp's input scale: i16 -> f32 in `[-1, 1)`.
const I16_SCALE: f32 = 32_768.0;

/// The built-in engine of one job.
pub struct BuiltinEngine {
    deps: BuiltinDeps,
    /// `builtin_local.model_id` of the job's settings snapshot.
    model_id: Option<String>,
    /// The warmth of the last transcription that got its model.
    warmth: Mutex<Option<Warmth>>,
}

impl BuiltinEngine {
    pub(crate) fn new(deps: &BuiltinDeps, model_id: Option<String>) -> BuiltinEngine {
        BuiltinEngine {
            deps: deps.clone(),
            model_id,
            warmth: Mutex::new(None),
        }
    }

    fn set_warmth(&self, warmth: Option<Warmth>) {
        *self.warmth.lock().unwrap_or_else(PoisonError::into_inner) = warmth;
    }
}

impl Engine for BuiltinEngine {
    fn kind(&self) -> &'static str {
        "builtin"
    }

    fn transcribe(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        self.set_warmth(None);
        let id = self
            .model_id
            .as_deref()
            .and_then(ModelId::parse)
            .ok_or(FailureReason::NoLocalModel)?;
        let path = self
            .deps
            .store
            .path_if_downloaded(id)
            .ok_or(FailureReason::NoLocalModel)?;
        let (model, warmth) = self.deps.residency.acquire(id, path)?;
        self.set_warmth(Some(warmth));

        let pcm: Vec<f32> = audio
            .samples()
            .iter()
            .map(|&s| f32::from(s) / I16_SCALE)
            .collect();
        let result = model.transcribe_within(&pcm, req.language.as_deref(), req.timeouts.builtin);
        drop(model);
        result.map(|text| text.trim().to_string())
    }

    fn local_warmth(&self) -> Option<Warmth> {
        *self.warmth.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
