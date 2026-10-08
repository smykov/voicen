//! "Test connection" for the API and local-server engines (spec 004 US4, FR-017,
//! FR-018; research R-9; decisions #51, #59; T-046).
//!
//! Entered only through [`SettingsService::test_connection`](crate::settings::service::SettingsService::test_connection),
//! which owns the snapshot, the credential store and the #19 `Unavailable` rule.
//! One test sends at most one request, and only through the dictation path:
//! [`engine_for`] -> `OpenAiCompatibleEngine::transcribe` with the bundled speech
//! clip, no language and no VAD. Its settings are the snapshot overlaid by the
//! form's engine, base URL, model and timeouts, then the save's `normalize`; its
//! key follows the save's key rule (`key_step`): a typed key trimmed; a blank one
//! or `Untouched` -> the stored key (one read of the selected slot, none while
//! `Unavailable`); `Clear` -> no key. The form is checked with the save's
//! `validate`, keeping only the fields this test uses. Its durations are
//! [`request_timeouts`] of the form (FR-017: the limits "as currently entered in
//! the form"), never `Timeouts::default()` or the saved ones.
//!
//! A test has no write path: it never calls `save`, the settings file or the
//! snapshot, and the engine sees the key through [`ResolvedKey`], whose `write`
//! and `delete` fail without delegating. A [`ConnectionTestResult`] is built only
//! from a `FailureReason`, the base URL's `host[:port]`, field ids and codes and an
//! `Instant`-measured latency (#47(2)); never from a key, a URL query or a body.

use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::audio::AudioBuffer;
use crate::engine::openai::host_port;
use crate::engine::{engine_for, TranscribeRequest};
use crate::failure::FailureReason;
use crate::models::DownloadedModels;
use crate::secrets::KeyPresence;
use crate::secrets::{CredentialError, CredentialStore, KeyEdit, KeyEdits, KeySlot, Secret};
use crate::settings::service::{key_step, normalize, KeyStep};
use crate::settings::url::check_base_url;
use crate::settings::validate::{validate, KeyEditsWithPresence};
use crate::settings::{EngineKind, ErrorCode, FieldError, FieldId, Settings, TimeoutSettings};
use crate::timeouts::Timeouts;

/// The engines a connection test exists for (closed: `none` and `builtin_local`
/// cannot be tested and are refused when the request is read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestEngine {
    Api,
    LocalServer,
}

/// The form values of one test, unsaved ones included (data-model
/// "ConnectionTestRequest").
///
/// Wire form (UI -> shell, `settings_test_connection { request }`):
/// `{"engine": "api" | "local_server", "base_url": string, "model": string, "key":
/// KeyEdit, "timeouts": TimeoutSettings}`, every field required. Any deserialize
/// error is one fixed text: the request carries a key, and serde's own messages
/// quote the input (T-030 J1). `Debug` prints the key as `***` (`Secret`).
#[derive(Debug)]
pub struct ConnectionTestRequest {
    pub engine: TestEngine,
    pub base_url: String,
    pub model: String,
    /// The selected engine's key field: the save's semantics (#30, #33(a)).
    pub key: KeyEdit,
    /// The form's timeouts (FR-017); converted only by [`request_timeouts`].
    pub timeouts: TimeoutSettings,
}

/// The fixed deserialize error of [`ConnectionTestRequest`].
const REQUEST_WIRE_ERROR: &str = "invalid connection test request: expected {\"engine\": \
     \"api\" | \"local_server\", \"base_url\", \"model\", \"key\": KeyEdit, \"timeouts\": \
     TimeoutSettings}";

#[derive(Deserialize)]
struct ConnectionTestRequestWire {
    engine: TestEngine,
    base_url: String,
    model: String,
    key: KeyEdit,
    timeouts: TimeoutSettings,
}

impl<'de> Deserialize<'de> for ConnectionTestRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match ConnectionTestRequestWire::deserialize(deserializer) {
            Ok(wire) => Ok(ConnectionTestRequest {
                engine: wire.engine,
                base_url: wire.base_url,
                model: wire.model,
                key: wire.key,
                timeouts: wire.timeouts,
            }),
            Err(_) => Err(serde::de::Error::custom(REQUEST_WIRE_ERROR)),
        }
    }
}

/// What a test found (data-model "ConnectionTestResult"). Never contains a key, a
/// URL query or a response body.
///
/// Wire form (shell -> UI), tagged by `kind`: `{"kind": "ok", "latency_ms": n}`,
/// `{"kind": "cannot_reach", "host": "host[:port]"}`, `{"kind": "invalid_key"}`,
/// `{"kind": "timeout"}`, `{"kind": "http", "status": n}`,
/// `{"kind": "unexpected_response"}`, `{"kind": "invalid", "errors":
/// [FieldError]}`, `{"kind": "key_store_unavailable"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionTestResult {
    /// A 2xx with a transcription body (even an empty text); the request's
    /// duration in milliseconds.
    Ok { latency_ms: u64 },
    /// DNS failure, connection refused, connect timeout, TLS failure: the base
    /// URL's `host[:port]` (explicit port only).
    CannotReach { host: String },
    /// HTTP 401 / 403, or a key that cannot be sent in a header.
    InvalidKey,
    /// The form's request limit exceeded.
    Timeout,
    /// Any other non-2xx status.
    Http { status: u16 },
    /// A 2xx whose body is not a transcription, or a cut-off answer.
    UnexpectedResponse,
    /// The form cannot form a request: the save's errors on the fields this test
    /// uses (the selected engine's URL, model and key, the connect limit and the
    /// engine's request limit). Nothing was sent.
    Invalid { errors: Vec<FieldError> },
    /// The stored key could not be read (#51); nothing was sent.
    KeyStoreUnavailable,
}

/// The durations of a test: the form's timeouts through the one conversion
/// [`Timeouts::from_settings`] (FR-017, R-9, #99), clamped like a job's.
pub fn request_timeouts(req: &ConnectionTestRequest) -> Timeouts {
    Timeouts::from_settings(&req.timeouts)
}

/// The bundled speech clip: about 1 s of 16 kHz mono signed 16-bit little-endian
/// PCM, a CC0 Lingua Libre recording of "speech bubble" (decision #59; source,
/// speaker and licence in licenses/manual.json).
const CLIP_PCM: &[u8] = include_bytes!("../assets/connection-test-speech.s16le");

/// The clip as the engine takes it.
fn clip() -> AudioBuffer {
    AudioBuffer::from_16k_mono(
        CLIP_PCM
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect(),
    )
}

/// The OS code a [`ResolvedKey`] write or delete reports (`ERROR_ACCESS_DENIED`);
/// the engine never calls either.
const READ_ONLY: CredentialError = CredentialError { os_code: 5 };

/// The credential view the engine of a test reads: the key already resolved by
/// the save's key rule, for the selected slot only. Read-only: `write` and
/// `delete` fail and delegate nothing.
struct ResolvedKey {
    slot: KeySlot,
    key: Option<Secret>,
}

impl fmt::Debug for ResolvedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedKey")
            .field("slot", &self.slot)
            .field("key", &self.key.as_ref().map(|_| "***"))
            .finish()
    }
}

impl CredentialStore for ResolvedKey {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
        Ok(self
            .key
            .as_ref()
            .filter(|_| slot == self.slot)
            .map(|key| Secret::new(key.expose())))
    }

    fn write(&self, _slot: KeySlot, _secret: &Secret) -> Result<(), CredentialError> {
        Err(READ_ONLY)
    }

    fn delete(&self, _slot: KeySlot) -> Result<(), CredentialError> {
        Err(READ_ONLY)
    }
}

/// What a test needs from the service: the snapshot in force, whether the load
/// was `Unavailable` (#19: no credential call then), the credential store and the
/// downloaded models (`validate`'s input).
pub(crate) struct TestContext<'a> {
    pub(crate) snapshot: &'a Settings,
    pub(crate) unavailable: bool,
    pub(crate) credentials: &'a dyn CredentialStore,
    pub(crate) models: &'a dyn DownloadedModels,
}

/// One connection test with the given durations (the service passes
/// [`request_timeouts`] of `req`; tests may inject shorter ones).
pub(crate) fn run(
    ctx: &TestContext<'_>,
    req: ConnectionTestRequest,
    timeouts: Timeouts,
) -> ConnectionTestResult {
    let ConnectionTestRequest {
        engine,
        base_url,
        model,
        key,
        timeouts: form_timeouts,
    } = req;
    let (slot, kind) = match engine {
        TestEngine::Api => (KeySlot::TranscriptionApi, EngineKind::Api),
        TestEngine::LocalServer => (KeySlot::LocalServer, EngineKind::LocalServer),
    };

    // (1) The form over the snapshot, then the save's normalize.
    let mut overlaid = ctx.snapshot.clone();
    overlaid.engine = kind;
    match engine {
        TestEngine::Api => {
            overlaid.api.base_url = base_url;
            overlaid.api.model = model;
        }
        TestEngine::LocalServer => {
            overlaid.local_server.base_url = base_url;
            overlaid.local_server.model = model;
        }
    }
    overlaid.timeouts = form_timeouts;
    let overlaid = normalize(overlaid);

    // (2) The save's key rule on the selected slot.
    let edits = edits_with(slot, key);
    let step = key_step(&edits, slot);

    // (3) The save's validation before any key read: a stored key is assumed for
    // `Keep`, so a URL, model or limit error refuses with no credential call.
    let assumed = match step {
        KeyStep::Keep | KeyStep::Store(_) => true,
        KeyStep::Delete => false,
    };
    let errors = test_errors(ctx, &overlaid, engine, slot, assumed);
    if !errors.is_empty() {
        return ConnectionTestResult::Invalid { errors };
    }

    // (4) The key: typed (trimmed), none, or the stored one (one read, none while
    // Unavailable; #19, #33(b)).
    let resolved = match step {
        KeyStep::Store(typed) => Some(Secret::new(typed)),
        KeyStep::Delete => None,
        KeyStep::Keep if ctx.unavailable => None,
        KeyStep::Keep => match ctx.credentials.read(slot) {
            Ok(stored) => stored,
            Err(_) => return ConnectionTestResult::KeyStoreUnavailable,
        },
    };
    if matches!(step, KeyStep::Keep) {
        // The key is now known: the save's `key.required` with its real presence.
        let errors = test_errors(ctx, &overlaid, engine, slot, resolved.is_some());
        if !errors.is_empty() {
            return ConnectionTestResult::Invalid { errors };
        }
    }

    // (5) The one request through the dictation path, timed with `Instant`.
    let view = ResolvedKey {
        slot,
        key: resolved,
    };
    let built = match engine_for(&overlaid, &view) {
        Ok(built) => built,
        Err(reason) => return from_failure(reason, &overlaid, engine),
    };
    let request = TranscribeRequest {
        language: None,
        timeouts,
    };
    let audio = clip();
    let started = Instant::now();
    let outcome = built.transcribe(&audio, &request);
    let elapsed = started.elapsed();
    match outcome {
        Ok(_text) => ConnectionTestResult::Ok {
            latency_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        },
        Err(reason) => from_failure(reason, &overlaid, engine),
    }
}

/// `key` in `slot`, every other slot untouched (the save's `KeyEdits` shape, so
/// `key_step` is the save's own).
fn edits_with(slot: KeySlot, key: KeyEdit) -> KeyEdits {
    let mut edits = KeyEdits::default();
    match slot {
        KeySlot::TranscriptionApi => edits.transcription_api = key,
        KeySlot::LocalServer => edits.local_server = key,
        KeySlot::PostProcessing => edits.post_processing = key,
    }
    edits
}

/// The save's `validate` of `settings`, with `has_key` as the selected slot's
/// presence, keeping only the fields this test uses (choice (i): the connect
/// limit and the engine's request limit too).
fn test_errors(
    ctx: &TestContext<'_>,
    settings: &Settings,
    engine: TestEngine,
    slot: KeySlot,
    has_key: bool,
) -> Vec<FieldError> {
    let used: &[FieldId] = match engine {
        TestEngine::Api => &[
            FieldId::EngineApiBaseUrl,
            FieldId::EngineApiModel,
            FieldId::EngineApiKey,
            FieldId::TimeoutsConnect,
            FieldId::TimeoutsApiTranscription,
        ],
        TestEngine::LocalServer => &[
            FieldId::EngineLocalServerBaseUrl,
            FieldId::EngineLocalServerModel,
            FieldId::EngineLocalServerKey,
            FieldId::TimeoutsConnect,
            FieldId::TimeoutsLocalServer,
        ],
    };
    let mut presence = KeyPresence::default();
    match slot {
        KeySlot::TranscriptionApi => presence.transcription_api = has_key,
        KeySlot::LocalServer => presence.local_server = has_key,
        KeySlot::PostProcessing => presence.post_processing = has_key,
    }
    // Untouched edits: the key is already resolved, so presence alone decides.
    let edits = KeyEdits::default();
    validate(
        settings,
        &KeyEditsWithPresence {
            edits: &edits,
            presence,
        },
        ctx.models,
    )
    .into_iter()
    .filter(|e| used.contains(&e.field))
    .collect()
}

/// The result of a failed build or request. Exhaustive: a new reason must be
/// placed here. A DNS failure (or OS network unreachable) is `CannotReach` in a
/// test (FR-017, #51); `classify` keeps `NetworkUnavailable` for dictation.
fn from_failure(
    reason: FailureReason,
    settings: &Settings,
    engine: TestEngine,
) -> ConnectionTestResult {
    let (raw, url_field) = match engine {
        TestEngine::Api => (settings.api.base_url.as_str(), FieldId::EngineApiBaseUrl),
        TestEngine::LocalServer => (
            settings.local_server.base_url.as_str(),
            FieldId::EngineLocalServerBaseUrl,
        ),
    };
    match reason {
        FailureReason::InvalidApiKey => ConnectionTestResult::InvalidKey,
        FailureReason::NetworkUnavailable => {
            ConnectionTestResult::CannotReach { host: host_of(raw) }
        }
        FailureReason::CannotReach { host } => ConnectionTestResult::CannotReach { host },
        FailureReason::Timeout => ConnectionTestResult::Timeout,
        FailureReason::ServerError { status } => ConnectionTestResult::Http { status },
        FailureReason::UnexpectedResponse => ConnectionTestResult::UnexpectedResponse,
        FailureReason::KeyStoreUnavailable => ConnectionTestResult::KeyStoreUnavailable,
        // The URL passed validation, so the engine cannot refuse it; kept total.
        FailureReason::EngineNotConfigured => ConnectionTestResult::Invalid {
            errors: vec![FieldError {
                field: url_field,
                code: ErrorCode::UrlMalformed,
            }],
        },
        // Not produced by an engine; kept total.
        FailureReason::ClipboardUnavailable | FailureReason::MicrophoneUnavailable { .. } => {
            ConnectionTestResult::UnexpectedResponse
        }
    }
}

/// `host[:port]` of a base URL that passed `check_base_url` (empty otherwise).
fn host_of(raw: &str) -> String {
    check_base_url(raw)
        .ok()
        .and_then(|url| url::Url::parse(url.as_str()).ok())
        .map(|url| host_port(&url))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_is_about_one_second_of_speech() {
        // R-9 / #59: about 1 s at 16 kHz, and not silent (some servers reject or
        // hallucinate on silence). Bite: an empty, truncated or zeroed asset, or
        // the bytes read big-endian.
        let audio = clip();
        let samples = audio.samples();
        assert_eq!(CLIP_PCM.len() % 2, 0, "whole 16-bit samples");
        assert!(
            (14_000..=18_000).contains(&samples.len()),
            "{} samples",
            samples.len()
        );
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak >= 8_000, "peak {peak}");
        let silent_tenths = samples
            .chunks(1_600)
            .filter(|tenth| tenth.iter().all(|s| s.unsigned_abs() < 200))
            .count();
        assert!(silent_tenths <= 2, "{silent_tenths} silent 100 ms windows");
    }

    #[test]
    fn resolved_key_reads_only_its_slot_and_never_writes() {
        // The read-only view: the resolved key for the selected slot, nothing for
        // another, and write/delete refused. Bite: a delegating view.
        let view = ResolvedKey {
            slot: KeySlot::LocalServer,
            key: Some(Secret::new("sk-test-view")),
        };
        let got = view.read(KeySlot::LocalServer).ok().flatten();
        assert_eq!(got.as_ref().map(Secret::expose), Some("sk-test-view"));
        assert!(matches!(view.read(KeySlot::TranscriptionApi), Ok(None)));
        assert!(view.write(KeySlot::LocalServer, &Secret::new("x")).is_err());
        assert!(view.delete(KeySlot::LocalServer).is_err());
        assert!(!format!("{view:?}").contains("sk-test"));
    }
}
