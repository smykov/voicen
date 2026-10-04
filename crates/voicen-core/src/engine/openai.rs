//! The one OpenAI-compatible transcription client
//! (contracts/openai-transcription.md; reused by T-018 for the local server and by
//! T-020's connection test).
//!
//! `POST {base}/audio/transcriptions`, multipart `file` (`audio.wav`, `audio/wav`),
//! `model`, `language` (omitted for auto), `response_format=json`;
//! `Authorization: Bearer <key>` only when a key is stored. The URL is joined on the
//! parsed base URL, so a query string (decision #27(2)) survives. The blocking
//! client is built per call from `TranscribeRequest::timeouts` (connect and
//! whole-request). The body is read through a 1 MiB cap. Every failure goes
//! through `failure::classify`; no `reqwest::Error`, URL or body reaches a
//! `FailureReason`.

use super::{Engine, TranscribeRequest};
use crate::audio::AudioBuffer;
use crate::failure::FailureReason;
use crate::secrets::Secret;
use crate::settings::url::NormalizedUrl;

/// The OpenAI-compatible engine for one base URL, model and optional key.
// T-040 skeleton: fields unused until implemented.
#[allow(dead_code)]
pub struct OpenAiCompatibleEngine {
    base_url: NormalizedUrl,
    model: String,
    key: Option<Secret>,
}

impl OpenAiCompatibleEngine {
    /// `base_url` passed `check_base_url` (the one URL rule); `key: None` sends no
    /// `Authorization` header.
    pub fn new(
        base_url: NormalizedUrl,
        model: impl Into<String>,
        key: Option<Secret>,
    ) -> OpenAiCompatibleEngine {
        OpenAiCompatibleEngine {
            base_url,
            model: model.into(),
            key,
        }
    }
}

impl Engine for OpenAiCompatibleEngine {
    fn kind(&self) -> &'static str {
        // T-040 skeleton: wrong on purpose until implemented (red tests first).
        ""
    }

    fn transcribe(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        // T-040 skeleton: wrong on purpose until implemented (red tests first).
        let _ = (audio, req);
        Err(FailureReason::EngineNotConfigured)
    }
}

/// `{base}/audio/transcriptions` with the base's query kept: one empty trailing
/// path segment dropped, then `audio`, `transcriptions` appended. `None` only for
/// a cannot-be-a-base URL (never `http`/`https`).
// T-040 skeleton: unused until implemented.
#[allow(dead_code)]
pub(crate) fn transcription_url(base: &url::Url) -> Option<url::Url> {
    // T-040 skeleton: wrong on purpose until implemented (red tests first).
    let _ = base;
    None
}

/// The `host[:port]` shown in `CannotReach`: the host, plus the port only when the
/// URL names a non-default one. No scheme, userinfo, path or query.
// T-040 skeleton: unused until implemented.
#[allow(dead_code)]
pub(crate) fn host_port(base: &url::Url) -> String {
    // T-040 skeleton: wrong on purpose until implemented (red tests first).
    let _ = base;
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str) -> url::Url {
        match url::Url::parse(raw) {
            Ok(u) => u,
            Err(e) => panic!("test URL {raw:?}: {e}"),
        }
    }

    fn joined(raw: &str) -> Option<String> {
        transcription_url(&parsed(raw)).map(|u| u.to_string())
    }

    #[test]
    fn join_keeps_query() {
        // Decision #27(2): Azure-style `?api-version=` stays after the path.
        // Bite: string concatenation (`{base}/audio/transcriptions` puts the path
        // inside the query) or `Url::join` (drops the query and the last segment).
        assert_eq!(
            joined("https://api.example.com/v1?api-version=2024-06-01").as_deref(),
            Some("https://api.example.com/v1/audio/transcriptions?api-version=2024-06-01")
        );
    }

    #[test]
    fn join_root_base() {
        // Bite: a double slash (`//audio/...`) from the root path's empty segment.
        assert_eq!(
            joined("https://api.example.com").as_deref(),
            Some("https://api.example.com/audio/transcriptions")
        );
        assert_eq!(
            joined("http://127.0.0.1:8080/v1").as_deref(),
            Some("http://127.0.0.1:8080/v1/audio/transcriptions")
        );
    }

    #[test]
    fn join_one_trailing_slash() {
        // `check_base_url` strips one `/`, so `…/v1//` arrives as `…/v1/`; the join
        // drops that one empty segment. Bite: no pop_if_empty (`/v1//audio/...`).
        assert_eq!(
            joined("https://api.example.com/v1/").as_deref(),
            Some("https://api.example.com/v1/audio/transcriptions")
        );
        let normalized = match crate::settings::url::check_base_url("https://api.example.com/v1//")
        {
            Ok(u) => u,
            Err(e) => panic!("accepted by the URL rule: {e:?}"),
        };
        assert_eq!(
            joined(normalized.as_str()).as_deref(),
            Some("https://api.example.com/v1/audio/transcriptions")
        );
    }

    #[test]
    fn host_port_is_host_and_explicit_port_only() {
        // `CannotReach{host}` carries host[:port] of the base URL and nothing else
        // (no query: P-009). Bite: the whole URL, `Url::authority` with userinfo,
        // or a default port printed.
        for (raw, expected) in [
            ("https://api.example.com/v1", "api.example.com"),
            ("https://api.example.com:443/v1", "api.example.com"),
            ("https://api.example.com:8443/v1", "api.example.com:8443"),
            ("http://127.0.0.1:8080/v1", "127.0.0.1:8080"),
            ("http://[::1]:8000/v1", "[::1]:8000"),
            (
                "https://api.example.com/v1?api-version=SECRETQ",
                "api.example.com",
            ),
        ] {
            assert_eq!(host_port(&parsed(raw)), expected, "{raw}");
        }
    }
}
