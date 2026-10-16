//! Transcription engines (spec 001 T018, contracts/core-traits.md "Engine",
//! decisions #42, #44).
//!
//! Engines are synchronous (OQ-05 (a)): `transcribe` blocks its thread and must not
//! run inside a tokio runtime (the blocking reqwest client panics there).
//! [`engine_for`] is the one factory, called per job so a retry uses the current
//! settings and key (Clarification 4). The transcription API and the local
//! OpenAI-compatible server (T-018) share [`openai::OpenAiCompatibleEngine`]; its
//! endpoint role, fixed by the factory, picks the request deadline and `kind()`.
//! The built-in engine ([`builtin::BuiltinEngine`], T-098) is built by the same
//! factory from the [`builtin::BuiltinDeps`] its caller passes; its model comes
//! through [`speech_model`]'s port (implemented outside the core, decision #12).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod builtin;
pub(crate) mod http;
pub mod lookup;
pub mod openai;
pub mod speech_model;

use crate::audio::AudioBuffer;
use crate::failure::FailureReason;
use crate::local_models::residency::Warmth;
use crate::secrets::{CredentialStore, KeySlot};
use crate::settings::url::check_base_url;
use crate::settings::{EngineKind, Settings};
use crate::timeouts::Timeouts;

/// A transcription engine (NFR-11; shared with specs 002 and 003).
pub trait Engine: Send + Sync {
    /// Short stable name for logs: "api", "local_server", "builtin".
    fn kind(&self) -> &'static str;
    /// Transcribe 16 kHz mono audio. Empty or whitespace-only text is `Ok("")`.
    /// Never panics on server or engine data.
    fn transcribe(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, FailureReason>;

    /// Whether the last `transcribe` found its model loaded (the dictation log's
    /// `local=cold|warm`, spec 002 FR-015). `None` for an engine without a local
    /// model, and when the last call got no model.
    fn local_warmth(&self) -> Option<Warmth> {
        None
    }
}

/// Per-call input besides the audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeRequest {
    /// ISO 639-1 code (`Settings::speech_language`); `None` = auto, the field is omitted.
    pub language: Option<String>,
    /// The only source of the request's durations.
    pub timeouts: Timeouts,
}

/// The engine for the current settings, or why there is none. Total: no panic and
/// no request for any settings state.
///
/// - `Api`: `check_base_url(api.base_url)` (else `EngineNotConfigured`, no key
///   read) -> `creds.read(TranscriptionApi)` once (`Err` -> `KeyStoreUnavailable`;
///   `Ok(None)` = no `Authorization` header) -> [`openai::OpenAiCompatibleEngine`].
/// - `LocalServer`: `check_base_url(local_server.base_url)` (else
///   `EngineNotConfigured`, no key read) -> `creds.read(LocalServer)` once (`Err`
///   -> `KeyStoreUnavailable`; `Ok(None)` = no `Authorization` header) ->
///   [`openai::OpenAiCompatibleEngine::local_server`] with the model trimmed, or
///   no model (the part omitted) when it is empty or whitespace-only.
/// - `BuiltinLocal` (T-098): with `builtin` -> [`builtin::BuiltinEngine`] over the
///   snapshot's `builtin_local.model_id` and the given residency and store, no key
///   read, nothing loaded (the model is resolved and loaded at `transcribe`);
///   without `builtin` (a caller that has no residency wired) ->
///   `EngineNotConfigured`, no key read.
/// - `None`: `EngineNotConfigured`, no key read.
pub fn engine_for(
    settings: &Settings,
    creds: &dyn CredentialStore,
    builtin: Option<&builtin::BuiltinDeps>,
) -> Result<Box<dyn Engine>, FailureReason> {
    match settings.engine {
        EngineKind::Api => {
            let base_url = check_base_url(&settings.api.base_url)
                .map_err(|_| FailureReason::EngineNotConfigured)?;
            let key = creds
                .read(KeySlot::TranscriptionApi)
                .map_err(|_| FailureReason::KeyStoreUnavailable)?;
            Ok(Box::new(openai::OpenAiCompatibleEngine::new(
                base_url,
                settings.api.model.clone(),
                key,
            )))
        }
        EngineKind::LocalServer => {
            let base_url = check_base_url(&settings.local_server.base_url)
                .map_err(|_| FailureReason::EngineNotConfigured)?;
            let key = creds
                .read(KeySlot::LocalServer)
                .map_err(|_| FailureReason::KeyStoreUnavailable)?;
            let model = settings.local_server.model.trim();
            let model = (!model.is_empty()).then(|| model.to_string());
            Ok(Box::new(openai::OpenAiCompatibleEngine::local_server(
                base_url, model, key,
            )))
        }
        EngineKind::BuiltinLocal => match builtin {
            Some(deps) => Ok(Box::new(builtin::BuiltinEngine::new(
                deps,
                settings.builtin_local.model_id.clone(),
            ))),
            None => Err(FailureReason::EngineNotConfigured),
        },
        EngineKind::None => Err(FailureReason::EngineNotConfigured),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        CredentialCall, CredentialError, CredentialOp, FakeCredentialStore, KeySlot,
    };
    use crate::settings::{defaults, EngineKind};

    const KEY: &str = "sk-test-SECRET";

    fn api_settings(base_url: &str) -> Settings {
        let mut s = defaults(None);
        s.engine = EngineKind::Api;
        s.api.base_url = base_url.to_string();
        s
    }

    fn reason(result: Result<Box<dyn Engine>, FailureReason>) -> Option<FailureReason> {
        result.err()
    }

    fn read_api() -> CredentialCall {
        CredentialCall {
            op: CredentialOp::Read,
            slot: KeySlot::TranscriptionApi,
        }
    }

    #[test]
    fn key_store_error_is_key_store_unavailable() {
        // Decision #44 (1): a credential read error gives the retryable
        // KeyStoreUnavailable and no engine, so no request can be sent; the key is
        // read once, not retried. Bite: treating Err like Ok(None) (an engine
        // without a key would send a request and get 401 -> InvalidApiKey).
        let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY);
        creds.fail(
            CredentialOp::Read,
            KeySlot::TranscriptionApi,
            CredentialError { os_code: 1312 },
        );
        let got = reason(engine_for(
            &api_settings("https://api.example.com/v1"),
            &creds,
            None,
        ));
        assert_eq!(got, Some(FailureReason::KeyStoreUnavailable));
        assert_eq!(creds.calls(), vec![read_api()]);
    }

    // ---- T-098: BuiltinLocal --------------------------------------------------------

    /// A loader that must never run: the factory builds the engine without a load
    /// (the residency loads at the job's first transcription).
    struct NoLoad;

    impl crate::engine::speech_model::SpeechModelLoader for NoLoad {
        fn load(
            &self,
            _path: &std::path::Path,
        ) -> Result<Box<dyn crate::engine::speech_model::SpeechModel>, FailureReason> {
            panic!("engine_for must not load a model");
        }
    }

    struct StillClock(std::time::Instant);

    impl crate::clock::MonotonicClock for StillClock {
        fn now(&self) -> std::time::Instant {
            self.0
        }
    }

    /// The built-in deps over `dir`, which holds a downloaded `tiny` (the file of
    /// its catalog size).
    fn builtin_deps(dir: &std::path::Path) -> builtin::BuiltinDeps {
        use crate::local_models::catalog::{CatalogEntry, ModelId};
        use crate::test_support::local_models::{catalog, model_bytes, SHA256, SIZE};
        let entries: &'static [CatalogEntry] = catalog(vec![CatalogEntry {
            id: ModelId::Tiny,
            file_name: "ggml-tiny.bin",
            url: "https://models.example.com/ggml-tiny.bin",
            size_bytes: SIZE,
            sha256: SHA256,
            recommended: false,
        }]);
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::fs::write(dir.join("ggml-tiny.bin"), model_bytes())
            .unwrap_or_else(|e| panic!("write model: {e}"));
        let clock: std::sync::Arc<dyn crate::clock::MonotonicClock> =
            std::sync::Arc::new(StillClock(crate::clock::instant()));
        builtin::BuiltinDeps {
            residency: std::sync::Arc::new(crate::local_models::residency::ModelResidency::new(
                std::sync::Arc::new(NoLoad),
                clock,
                crate::timeouts::MODEL_IDLE_UNLOAD,
            )),
            store: std::sync::Arc::new(crate::local_models::store::ModelStore::new(
                dir.to_path_buf(),
                entries,
            )),
        }
    }

    #[test]
    fn unset_engine_is_engine_not_configured() {
        // Decision #44 (2): None gives EngineNotConfigured without reading any key,
        // with or without the built-in deps. Bite: a panic / todo!() arm, an
        // engine built for None (the built-in one, since its deps are there), a
        // key read first.
        let tmp = crate::test_support::TempDir::new();
        let deps = builtin_deps(&tmp.path().join("models"));
        for with in [None, Some(&deps)] {
            let creds = FakeCredentialStore::new()
                .with_key(KeySlot::TranscriptionApi, KEY)
                .with_key(KeySlot::LocalServer, KEY);
            let mut s = api_settings("https://api.example.com/v1");
            s.engine = EngineKind::None;
            s.builtin_local.model_id = Some("tiny".to_string());
            let got = reason(engine_for(&s, &creds, with));
            assert_eq!(got, Some(FailureReason::EngineNotConfigured));
            assert_eq!(creds.calls(), vec![], "no credential call");
        }
    }

    #[test]
    fn builtin_local_builds_the_builtin_engine_without_a_key_or_a_load() {
        // T-098 flips decision #44 (2) for BuiltinLocal: the one factory builds
        // the built-in engine (kind "builtin") from the deps, reads no key and
        // loads nothing at build time (NoLoad panics). Bite: the arm left at
        // EngineNotConfigured, an API / local-server engine built instead (their
        // sections are valid here), a key read, a load in the factory.
        let tmp = crate::test_support::TempDir::new();
        let deps = builtin_deps(&tmp.path().join("models"));
        let creds = FakeCredentialStore::new()
            .with_key(KeySlot::TranscriptionApi, KEY)
            .with_key(KeySlot::LocalServer, KEY);
        let mut s = api_settings("https://api.example.com/v1");
        s.engine = EngineKind::BuiltinLocal;
        s.builtin_local.model_id = Some("tiny".to_string());
        match engine_for(&s, &creds, Some(&deps)) {
            Ok(engine) => assert_eq!(engine.kind(), "builtin"),
            Err(e) => panic!("built-in engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![], "no credential call");
    }

    #[test]
    fn builtin_local_without_deps_is_engine_not_configured() {
        // A caller that has no residency (no built-in engine wired) gets the
        // settled EngineNotConfigured, never a panic or a key read. Bite: an
        // unwrap of the deps, a fall-through to the API arm.
        let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY);
        let mut s = api_settings("https://api.example.com/v1");
        s.engine = EngineKind::BuiltinLocal;
        s.builtin_local.model_id = Some("tiny".to_string());
        assert_eq!(
            reason(engine_for(&s, &creds, None)),
            Some(FailureReason::EngineNotConfigured)
        );
        assert_eq!(creds.calls(), vec![], "no credential call");
    }

    #[test]
    fn bad_stored_base_url_is_engine_not_configured() {
        // A hand-edited settings file reaches dictation (load_or_init accepts any
        // string; check_base_url runs only at save). The factory applies the one
        // URL rule first, so a bad URL gives EngineNotConfigured and the key is
        // never read (no key sent to a userinfo URL). Bite: no check_base_url in
        // the factory, or the key read before it.
        for bad in [
            "",
            "   ",
            "not a url",
            "ftp://api.example.com/v1",
            "https://",
            "https://user:pass@api.example.com/v1",
            "https://user@api.example.com/v1",
        ] {
            let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY);
            let got = reason(engine_for(&api_settings(bad), &creds, None));
            assert_eq!(got, Some(FailureReason::EngineNotConfigured), "{bad:?}");
            assert_eq!(creds.calls(), vec![], "{bad:?}: no credential call");
        }
    }

    #[test]
    fn api_engine_reads_transcription_slot_once() {
        // Bite: reading another slot, reading twice, or not building the engine.
        let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY);
        match engine_for(
            &api_settings("https://api.example.com/v1?api-version=2024-06-01"),
            &creds,
            None,
        ) {
            Ok(engine) => assert_eq!(engine.kind(), "api"),
            Err(e) => panic!("API engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![read_api()]);
    }

    // ---- T-018: LocalServer ------------------------------------------------------

    fn local_settings(base_url: &str) -> Settings {
        let mut s = defaults(None);
        s.engine = EngineKind::LocalServer;
        s.local_server.base_url = base_url.to_string();
        // The API section stays valid, so an arm that builds from it would succeed.
        s.api.base_url = "https://api.example.com/v1".to_string();
        s
    }

    fn read_local() -> CredentialCall {
        CredentialCall {
            op: CredentialOp::Read,
            slot: KeySlot::LocalServer,
        }
    }

    #[test]
    fn local_server_engine_reads_only_local_server_slot_once() {
        // T-018: engine = local server builds the OpenAI-compatible engine with
        // kind "local_server" (diag EngineTag), reading only the `local-server`
        // slot, once. Bite: the arm left at EngineNotConfigured, kind() "api", the
        // API slot read, a second read.
        let creds = FakeCredentialStore::new()
            .with_key(KeySlot::TranscriptionApi, KEY)
            .with_key(KeySlot::LocalServer, "sk-test-LOCAL");
        match engine_for(&local_settings("http://localhost:8000/v1"), &creds, None) {
            Ok(engine) => assert_eq!(engine.kind(), "local_server"),
            Err(e) => panic!("local-server engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![read_local()]);
    }

    #[test]
    fn local_server_default_settings_build_without_key_or_model() {
        // The defaults (http://localhost:8000/v1, model "", no key) are a usable
        // local-server configuration: the key and the model are optional (FR-13,
        // decision #28(b)). Bite: Ok(None) or an empty model treated as
        // EngineNotConfigured / KeyStoreUnavailable.
        let mut s = defaults(None);
        s.engine = EngineKind::LocalServer;
        let creds = FakeCredentialStore::new();
        match engine_for(&s, &creds, None) {
            Ok(engine) => assert_eq!(engine.kind(), "local_server"),
            Err(e) => panic!("local-server engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![read_local()]);
    }

    #[test]
    fn local_server_key_store_error_is_key_store_unavailable() {
        // Decision #44 (1) for the local slot: a read error is the retryable
        // KeyStoreUnavailable, not "no key" (which would send an unauthenticated
        // request). Read once. Bite: Err treated like Ok(None).
        let creds = FakeCredentialStore::new().with_key(KeySlot::LocalServer, "sk-test-LOCAL");
        creds.fail(
            CredentialOp::Read,
            KeySlot::LocalServer,
            CredentialError { os_code: 1312 },
        );
        let got = reason(engine_for(
            &local_settings("http://localhost:8000/v1"),
            &creds,
            None,
        ));
        assert_eq!(got, Some(FailureReason::KeyStoreUnavailable));
        assert_eq!(creds.calls(), vec![read_local()]);
    }

    #[test]
    fn local_server_bad_stored_base_url_is_engine_not_configured() {
        // The one URL rule applies to the stored local-server URL in the factory
        // (a hand-edited file bypasses save): EngineNotConfigured, no key read.
        // Bite: no check_base_url in the arm, the API URL checked instead, or the
        // key read first.
        for bad in [
            "",
            "   ",
            "not a url",
            "ftp://localhost:8000/v1",
            "http://user:pass@localhost:8000/v1",
        ] {
            let creds = FakeCredentialStore::new().with_key(KeySlot::LocalServer, "sk-test-LOCAL");
            let got = reason(engine_for(&local_settings(bad), &creds, None));
            assert_eq!(got, Some(FailureReason::EngineNotConfigured), "{bad:?}");
            assert_eq!(creds.calls(), vec![], "{bad:?}: no credential call");
        }
    }

    #[test]
    fn api_engine_without_stored_key_is_built() {
        // Ok(None) is "no key stored": the engine is built and sends no
        // Authorization header (the server's 401 then gives InvalidApiKey).
        // Bite: Ok(None) turned into KeyStoreUnavailable / EngineNotConfigured.
        let creds = FakeCredentialStore::new();
        match engine_for(&api_settings("https://api.example.com/v1"), &creds, None) {
            Ok(engine) => assert_eq!(engine.kind(), "api"),
            Err(e) => panic!("API engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![read_api()]);
    }
}
