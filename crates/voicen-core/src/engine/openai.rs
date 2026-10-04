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

use std::error::Error as _;
use std::io::Read;

use reqwest::blocking::{multipart, Client};
use reqwest::header::CONTENT_TYPE;

use super::{Engine, TranscribeRequest};
use crate::audio::{wav, AudioBuffer};
use crate::failure::{classify, FailureReason, TransportError};
use crate::secrets::Secret;
use crate::settings::url::NormalizedUrl;

/// Largest accepted response body (contract: "body > 1 MiB" -> `UnexpectedResponse`).
const MAX_BODY: u64 = 1024 * 1024;

/// The OpenAI-compatible engine for one base URL, model and optional key.
pub struct OpenAiCompatibleEngine {
    base_url: NormalizedUrl,
    model: String,
    key: Option<Secret>,
}

impl OpenAiCompatibleEngine {
    /// `base_url` passed `check_base_url` (the one URL rule); `key: None` (or an
    /// empty key) sends no `Authorization` header.
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

    /// The multipart body of the contract (`file`, `model`, `language` only when
    /// set, `response_format=json`) and its `Content-Type`.
    ///
    /// Encoded into one buffer instead of `RequestBuilder::multipart`: reqwest's
    /// blocking client streams a multipart reader through a channel and reports a
    /// failed connect as the body channel's `Disconnected` error when the connect
    /// fails first (no `is_connect` flag: a refused port would classify as
    /// `UnexpectedResponse`). A buffered body has no such channel.
    fn multipart_body(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Option<(String, Vec<u8>)> {
        let file = multipart::Part::bytes(wav::encode(audio))
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .ok()?;
        let mut form = multipart::Form::new()
            .part("file", file)
            .text("model", self.model.clone());
        if let Some(language) = &req.language {
            form = form.text("language", language.clone());
        }
        let form = form.text("response_format", "json");
        let content_type = format!("multipart/form-data; boundary={}", form.boundary());
        let mut body = Vec::new();
        form.into_reader().read_to_end(&mut body).ok()?;
        Some((content_type, body))
    }

    /// Sends the request and reads the body; every failure as a [`TransportError`].
    fn send(
        &self,
        url: url::Url,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, TransportError> {
        let client = Client::builder()
            .connect_timeout(req.timeouts.connect)
            .build()
            .map_err(|_| TransportError::Setup)?;
        let (content_type, body) = self
            .multipart_body(audio, req)
            .ok_or(TransportError::Setup)?;
        // The per-request timeout covers connect to the last body byte (FR-24).
        let mut request = client
            .post(url)
            .timeout(req.timeouts.api_transcription)
            .header(CONTENT_TYPE, content_type)
            .body(body);
        if let Some(key) = self.key.as_ref().filter(|k| !k.expose().is_empty()) {
            request = request.bearer_auth(key.expose());
        }
        let response = request.send().map_err(|e| send_error(&e))?;
        let status = response.status();
        if !status.is_success() {
            // The error body is never read (P-009).
            return Err(TransportError::Status(status.as_u16()));
        }
        let mut body = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|e| body_error(&e))?;
        if !u64::try_from(body.len()).is_ok_and(|len| len <= MAX_BODY) {
            return Err(TransportError::BadBody);
        }
        // An object with a string `text`; other fields are ignored. (A derived struct
        // would also accept a JSON array whose first element is a string.)
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| TransportError::BadBody)?;
        let text = parsed
            .as_object()
            .and_then(|object| object.get("text"))
            .and_then(serde_json::Value::as_str)
            .ok_or(TransportError::BadBody)?;
        Ok(text.trim().to_string())
    }
}

impl Engine for OpenAiCompatibleEngine {
    fn kind(&self) -> &'static str {
        "api"
    }

    fn transcribe(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        // `check_base_url` parsed this text already; a failure here means the engine
        // cannot be built from what it was given.
        let base = url::Url::parse(self.base_url.as_str())
            .map_err(|_| FailureReason::EngineNotConfigured)?;
        let url = transcription_url(&base).ok_or(FailureReason::EngineNotConfigured)?;
        self.send(url, audio, req)
            .map_err(|e| classify(&e, &host_port(&base)))
    }
}

/// The classification flags of a send error: reqwest's flags and the first
/// `io::ErrorKind` in the source chain. Nothing of the error's text is kept (its
/// `Display` contains the URL, query included).
fn send_error(e: &reqwest::Error) -> TransportError {
    if e.is_builder() {
        return TransportError::Setup;
    }
    let mut io = None;
    let mut source = e.source();
    while let Some(err) = source {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            io = Some(io_err.kind());
            break;
        }
        source = err.source();
    }
    TransportError::Send {
        dns: e.is_dns(),
        connect: e.is_connect(),
        timeout: e.is_timeout(),
        io,
    }
}

/// A body read error: the blocking reader wraps the `reqwest::Error` in an
/// `io::Error`; its timeout flag tells a stall past the deadline from a reset or an
/// early close.
fn body_error(e: &std::io::Error) -> TransportError {
    let timeout = e.kind() == std::io::ErrorKind::TimedOut
        || e.get_ref()
            .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout);
    TransportError::BodyRead { timeout }
}

/// `{base}/audio/transcriptions` with the base's query kept: one empty trailing
/// path segment dropped, then `audio`, `transcriptions` appended. `None` only for
/// a cannot-be-a-base URL (never `http`/`https`).
pub(crate) fn transcription_url(base: &url::Url) -> Option<url::Url> {
    let mut url = base.clone();
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(["audio", "transcriptions"]);
    Some(url)
}

/// The `host[:port]` shown in `CannotReach`: the host, plus the port only when the
/// URL names a non-default one. No scheme, userinfo, path or query.
pub(crate) fn host_port(base: &url::Url) -> String {
    let host = base.host_str().unwrap_or_default();
    match base.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
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
