//! Transcription engines (spec 001 T018, contracts/core-traits.md "Engine",
//! decisions #42, #44).
//!
//! Engines are synchronous (OQ-05 (a)): `transcribe` blocks its thread and must not
//! run inside a tokio runtime (the blocking reqwest client panics there).
//! [`engine_for`] is the one factory, called per job so a retry uses the current
//! settings and key (Clarification 4). The transcription API and the local
//! OpenAI-compatible server (T-018) share [`openai::OpenAiCompatibleEngine`]; its
//! endpoint role, fixed by the factory, picks the request deadline and `kind()`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub(crate) mod http;
pub mod openai;

use crate::audio::AudioBuffer;
use crate::failure::FailureReason;
use crate::secrets::{CredentialStore, KeySlot};
use crate::settings::url::check_base_url;
use crate::settings::{EngineKind, Settings};
use crate::timeouts::Timeouts;

/// A transcription engine (NFR-11; shared with specs 002 and 003).
pub trait Engine: Send + Sync {
    /// Short stable name for logs: "api", "local_server" (later "builtin").
    fn kind(&self) -> &'static str;
    /// Transcribe 16 kHz mono audio. Empty or whitespace-only text is `Ok("")`.
    /// Never panics on server or engine data.
    fn transcribe(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, FailureReason>;
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
/// - `BuiltinLocal` (built by the shell, T-017), `None`: `EngineNotConfigured`, no
///   key read.
pub fn engine_for(
    settings: &Settings,
    creds: &dyn CredentialStore,
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
        EngineKind::BuiltinLocal | EngineKind::None => Err(FailureReason::EngineNotConfigured),
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
        ));
        assert_eq!(got, Some(FailureReason::KeyStoreUnavailable));
        assert_eq!(creds.calls(), vec![read_api()]);
    }

    #[test]
    fn unbuilt_or_unset_engine_is_engine_not_configured() {
        // Decision #44 (2): BuiltinLocal (shell, T-017) and None give
        // EngineNotConfigured without reading any key (LocalServer is built since
        // T-018: local_server_* below). Bite: a panic / todo!() arm, an API engine
        // built for another kind, or a key read first.
        for kind in [EngineKind::BuiltinLocal, EngineKind::None] {
            let creds = FakeCredentialStore::new()
                .with_key(KeySlot::TranscriptionApi, KEY)
                .with_key(KeySlot::LocalServer, KEY);
            let mut s = api_settings("https://api.example.com/v1");
            s.engine = kind;
            let got = reason(engine_for(&s, &creds));
            assert_eq!(got, Some(FailureReason::EngineNotConfigured), "{kind:?}");
            assert_eq!(creds.calls(), vec![], "{kind:?}: no credential call");
        }
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
            let got = reason(engine_for(&api_settings(bad), &creds));
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
        match engine_for(&local_settings("http://localhost:8000/v1"), &creds) {
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
        match engine_for(&s, &creds) {
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
            let got = reason(engine_for(&local_settings(bad), &creds));
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
        match engine_for(&api_settings("https://api.example.com/v1"), &creds) {
            Ok(engine) => assert_eq!(engine.kind(), "api"),
            Err(e) => panic!("API engine expected, got {e:?}"),
        }
        assert_eq!(creds.calls(), vec![read_api()]);
    }
}
