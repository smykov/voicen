//! The one OpenAI-compatible transcription client
//! (contracts/openai-transcription.md) for both the transcription API and the
//! local OpenAI-compatible server (T-018), and for T-020's connection test.
//!
//! The endpoint role is fixed at construction ([`OpenAiCompatibleEngine::new`] for
//! the API, [`OpenAiCompatibleEngine::local_server`] for the local server) and
//! picks the request deadline (`Timeouts::api_transcription` or
//! `Timeouts::local_server`) and `kind()` (`"api"` or `"local_server"`).
//!
//! `POST {base}/audio/transcriptions` (or the stored URL as is when it already
//! ends in `audio/transcriptions`: [`transcription_url`], decision #96), multipart `file` (`audio.wav`, `audio/wav`),
//! `model` (omitted when the engine has none: an unset local-server model),
//! `language` (omitted for auto), `response_format=json`;
//! `Authorization: Bearer <key>` only when a key is stored; a key whose
//! `Bearer <key>` text fails `HeaderValue` validation is `InvalidApiKey` with no
//! request sent ([`TransportError::UnusableKey`]). The URL is joined on the
//! parsed base URL, so a query string (decision #27(2)) survives. The blocking
//! client is built per call from `TranscribeRequest::timeouts` (connect and
//! whole-request). The body is read through a 1 MiB cap. Every failure goes
//! through `failure::classify`; no `reqwest::Error`, URL or body reaches a
//! `FailureReason`.

use std::error::Error as _;
use std::io::Read;

use reqwest::blocking::{multipart, Client};
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use zeroize::Zeroizing;

use super::{Engine, TranscribeRequest};
use crate::audio::{wav, AudioBuffer};
use crate::failure::{classify, FailureReason, TransportError};
use crate::secrets::Secret;
use crate::settings::url::NormalizedUrl;

/// Largest accepted response body (contract: "body > 1 MiB" -> `UnexpectedResponse`).
const MAX_BODY: u64 = 1024 * 1024;

/// Room for the multipart framing around the WAV and the text parts: four part
/// headers with a 67-character boundary and the closing boundary are under 1 KiB.
const MULTIPART_OVERHEAD: usize = 1024;

/// Which server the engine talks to; set once at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    /// The transcription API (`kind() == "api"`, `Timeouts::api_transcription`).
    Api,
    /// The local OpenAI-compatible server (`kind() == "local_server"`,
    /// `Timeouts::local_server`).
    LocalServer,
}

/// The OpenAI-compatible engine for one endpoint role, base URL, optional model
/// and optional key.
pub struct OpenAiCompatibleEngine {
    endpoint: Endpoint,
    base_url: NormalizedUrl,
    model: Option<String>,
    key: Option<Secret>,
}

impl OpenAiCompatibleEngine {
    /// The transcription-API engine: `kind() == "api"`, request deadline
    /// `Timeouts::api_transcription`, `model` always sent. `base_url` passed
    /// `check_base_url` (the one URL rule); `key: None` (or an empty key) sends no
    /// `Authorization` header.
    pub fn new(
        base_url: NormalizedUrl,
        model: impl Into<String>,
        key: Option<Secret>,
    ) -> OpenAiCompatibleEngine {
        OpenAiCompatibleEngine {
            endpoint: Endpoint::Api,
            base_url,
            model: Some(model.into()),
            key,
        }
    }

    /// The local-server engine: `kind() == "local_server"`, request deadline
    /// `Timeouts::local_server`. `model: None` omits the `model` part; a `Some`
    /// model is sent as given (the caller, [`super::engine_for`], trims it and
    /// passes `None` for an empty one). Same URL and key rules as [`Self::new`].
    pub fn local_server(
        base_url: NormalizedUrl,
        model: Option<String>,
        key: Option<Secret>,
    ) -> OpenAiCompatibleEngine {
        OpenAiCompatibleEngine {
            endpoint: Endpoint::LocalServer,
            base_url,
            model,
            key,
        }
    }

    /// The multipart body of the contract (`file`, `model` and `language` only
    /// when set, `response_format=json`) and its `Content-Type`.
    ///
    /// Encoded into one buffer instead of `RequestBuilder::multipart`: reqwest's
    /// blocking client streams a multipart reader through a channel and reports a
    /// failed connect as the body channel's `Disconnected` error when the connect
    /// fails first (no `is_connect` flag: a refused port would classify as
    /// `UnexpectedResponse`). A buffered body has no such channel.
    ///
    /// The buffer is sized once from the WAV length ([`body_capacity`]), so reading
    /// the form never regrows it. The WAV moves into the form without a copy; the
    /// transient peak per call is the audio, the WAV and this body (about
    /// 3 x 19.2 MB at the 10-minute cap).
    fn multipart_body(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Option<(String, Vec<u8>)> {
        let wav = wav::encode(audio);
        let capacity = body_capacity(wav.len(), self.model.as_deref(), req.language.as_deref());
        let file = multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .ok()?;
        let mut form = multipart::Form::new().part("file", file);
        if let Some(model) = &self.model {
            form = form.text("model", model.clone());
        }
        if let Some(language) = &req.language {
            form = form.text("language", language.clone());
        }
        let form = form.text("response_format", "json");
        let content_type = format!("multipart/form-data; boundary={}", form.boundary());
        let mut body = Vec::with_capacity(capacity);
        form.into_reader().read_to_end(&mut body).ok()?;
        Some((content_type, body))
    }

    /// The `Authorization` value for the stored key: `None` without a key (or with
    /// an empty one). The one rule for a usable key: `Bearer <key>` passes
    /// `HeaderValue` validation (http 1.5: no control byte other than tab, no DEL);
    /// otherwise [`TransportError::UnusableKey`]. Bytes of a non-ASCII key pass that
    /// rule and are sent as UTF-8; the server's 401/403 then gives `InvalidApiKey`.
    /// The value is marked sensitive (not printed by `Debug`, not HPACK-indexed).
    fn authorization(&self) -> Result<Option<HeaderValue>, TransportError> {
        let Some(key) = self.key.as_ref().filter(|k| !k.expose().is_empty()) else {
            return Ok(None);
        };
        let text = Zeroizing::new(format!("Bearer {}", key.expose()));
        let mut value = HeaderValue::from_str(&text).map_err(|_| TransportError::UnusableKey)?;
        value.set_sensitive(true);
        Ok(Some(value))
    }

    /// Sends the request and reads the body; every failure as a [`TransportError`].
    fn send(
        &self,
        url: url::Url,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
    ) -> Result<String, TransportError> {
        // The key is checked before anything is built or sent.
        let authorization = self.authorization()?;
        let client = Client::builder()
            .connect_timeout(req.timeouts.connect)
            .build()
            .map_err(|_| TransportError::Setup)?;
        let (content_type, body) = self
            .multipart_body(audio, req)
            .ok_or(TransportError::Setup)?;
        // The per-request timeout covers connect to the last body byte (FR-24); the
        // endpoint role picks which deadline.
        let deadline = match self.endpoint {
            Endpoint::Api => req.timeouts.api_transcription,
            Endpoint::LocalServer => req.timeouts.local_server,
        };
        let mut request = client
            .post(url)
            .timeout(deadline)
            .header(CONTENT_TYPE, content_type)
            .body(body);
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
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
        match self.endpoint {
            Endpoint::Api => "api",
            Endpoint::LocalServer => "local_server",
        }
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
pub(crate) fn send_error(e: &reqwest::Error) -> TransportError {
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
pub(crate) fn body_error(e: &std::io::Error) -> TransportError {
    let timeout = e.kind() == std::io::ErrorKind::TimedOut
        || e.get_ref()
            .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout);
    TransportError::BodyRead { timeout }
}

/// The multipart buffer size for a WAV of `wav_len` bytes: the WAV, the text parts
/// and [`MULTIPART_OVERHEAD`] for the framing.
fn body_capacity(wav_len: usize, model: Option<&str>, language: Option<&str>) -> usize {
    wav_len
        .saturating_add(model.map_or(0, str::len))
        .saturating_add(language.map_or(0, str::len))
        .saturating_add(MULTIPART_OVERHEAD)
}

/// The request URL for the stored URL `base`, query always kept (decision #96):
/// one empty trailing path segment is dropped; then, if the last two path
/// segments equal `audio`, `transcriptions` (ASCII case-insensitive), the path is
/// kept as typed (the user gave the full endpoint), otherwise `audio`,
/// `transcriptions` are appended (the user gave a base). Only whole segments
/// decide: `/v1/xaudio/transcriptions` or `/v1/audio%2Ftranscriptions` is a base.
/// The stored setting is never rewritten; this is the only place the request URL
/// is built. `None` only for a cannot-be-a-base URL (never `http`/`https`).
pub(crate) fn transcription_url(base: &url::Url) -> Option<url::Url> {
    let mut url = base.clone();
    url.path_segments_mut().ok()?.pop_if_empty();
    if !names_endpoint(&url) {
        url.path_segments_mut()
            .ok()?
            .extend(["audio", "transcriptions"]);
    }
    Some(url)
}

/// Whether the last two path segments of `url` are `audio`, `transcriptions`,
/// compared ASCII case-insensitively on the segment text as stored.
fn names_endpoint(url: &url::Url) -> bool {
    let Some(segments) = url.path_segments() else {
        return false;
    };
    let mut last_two = segments.rev();
    matches!(
        (last_two.next(), last_two.next()),
        (Some(last), Some(prev))
            if last.eq_ignore_ascii_case("transcriptions") && prev.eq_ignore_ascii_case("audio")
    )
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
    fn join_full_endpoint_kept() {
        // T-072, decision #96: a URL whose last two path segments (after one empty
        // trailing segment is dropped) are `audio`, `transcriptions` is requested as
        // is, query kept, path in the case typed (segments compared ASCII
        // case-insensitively, orchestrator note). A doubled path is kept too: the
        // join never strips. Bite: the unconditional append (today), an exact-case
        // match (the mixed-case rows double), a "normalize to base then append"
        // (the doubled row loses a pair), a dropped query.
        for (raw, expected) in [
            (
                "http://10.10.10.110:8000/v1/audio/transcriptions",
                "http://10.10.10.110:8000/v1/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio/transcriptions/",
                "https://api.example.com/v1/audio/transcriptions",
            ),
            (
                "https://api.example.com/audio/transcriptions",
                "https://api.example.com/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio/transcriptions?api-version=2024-06-01",
                "https://api.example.com/v1/audio/transcriptions?api-version=2024-06-01",
            ),
            (
                "https://api.example.com/v1/audio/transcriptions/?api-version=2024-06-01",
                "https://api.example.com/v1/audio/transcriptions?api-version=2024-06-01",
            ),
            (
                "https://api.example.com/openai/deployments/w/audio/transcriptions?api-version=2024-06-01",
                "https://api.example.com/openai/deployments/w/audio/transcriptions?api-version=2024-06-01",
            ),
            (
                "https://api.example.com/v1/Audio/Transcriptions",
                "https://api.example.com/v1/Audio/Transcriptions",
            ),
            (
                "https://api.example.com/v1/AUDIO/transcriptions",
                "https://api.example.com/v1/AUDIO/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio/transcriptions/audio/transcriptions",
                "https://api.example.com/v1/audio/transcriptions/audio/transcriptions",
            ),
        ] {
            assert_eq!(joined(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn join_near_misses_append() {
        // T-072: only the two whole last segments decide; anything else is a base
        // and gets `audio`, `transcriptions` appended (query kept). Bite: a string
        // `ends_with("transcriptions")` / `ends_with("audio/transcriptions")` test
        // on the path text (the `transcriptions`, `xaudio`, `%2F` rows), a match on
        // the last segment only, a match anywhere in the path (`/x` row).
        for (raw, expected) in [
            (
                "https://api.example.com/v1/audio",
                "https://api.example.com/v1/audio/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/transcriptions",
                "https://api.example.com/v1/transcriptions/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/xaudio/transcriptions",
                "https://api.example.com/v1/xaudio/transcriptions/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio/transcriptions2",
                "https://api.example.com/v1/audio/transcriptions2/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio%2Ftranscriptions",
                "https://api.example.com/v1/audio%2Ftranscriptions/audio/transcriptions",
            ),
            (
                "https://api.example.com/audio/transcriptions/x",
                "https://api.example.com/audio/transcriptions/x/audio/transcriptions",
            ),
            (
                "https://api.example.com/v1/audio?api-version=2024-06-01",
                "https://api.example.com/v1/audio/audio/transcriptions?api-version=2024-06-01",
            ),
        ] {
            assert_eq!(joined(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    fn engine(key: Option<&str>) -> OpenAiCompatibleEngine {
        let base = match crate::settings::url::check_base_url("https://api.example.com/v1") {
            Ok(u) => u,
            Err(e) => panic!("accepted by the URL rule: {e:?}"),
        };
        OpenAiCompatibleEngine::new(base, "whisper-1", key.map(Secret::new))
    }

    #[test]
    fn multipart_body_is_sized_once_from_the_wav() {
        // Review 1 #5: the buffer is reserved from the WAV length and never regrown
        // while the form is read. Bite: `Vec::new()` (doubling growth leaves a
        // different capacity) or an overhead too small for the framing (regrowth).
        let audio = AudioBuffer::from_16k_mono(vec![0; 16_000 * 10]);
        for language in [None, Some("de")] {
            let req = TranscribeRequest {
                language: language.map(str::to_string),
                timeouts: crate::timeouts::Timeouts::default(),
            };
            let engine = engine(None);
            let Some((_, body)) = engine.multipart_body(&audio, &req) else {
                panic!("the body is encoded");
            };
            let expected = body_capacity(wav::encode(&audio).len(), Some("whisper-1"), language);
            assert_eq!(body.capacity(), expected, "language {language:?}");
            assert!(body.len() <= expected);
        }
    }

    #[test]
    fn authorization_is_the_header_value_rule() {
        // Review 1 #4: one rule, `Bearer <key>` passes `HeaderValue` validation.
        // Tab and non-ASCII bytes pass it (sent as given); other control bytes and
        // DEL do not. No key or an empty key: no header. Bite: a hand-made list
        // (refusing tab or non-ASCII), or the value not marked sensitive.
        assert!(matches!(engine(None).authorization(), Ok(None)));
        assert!(matches!(engine(Some("")).authorization(), Ok(None)));
        for key in ["sk-test-ok", "sk-test-a\tb", "sk-test-\u{e9}"] {
            match engine(Some(key)).authorization() {
                Ok(Some(value)) => {
                    assert!(value.is_sensitive(), "{key:?}");
                    assert_eq!(value.as_bytes(), format!("Bearer {key}").as_bytes());
                }
                other => panic!("{key:?}: {other:?}"),
            }
        }
        for key in ["sk-test-a\nb", "sk-test-a\u{0}b", "sk-test-a\u{7f}b"] {
            assert!(
                matches!(
                    engine(Some(key)).authorization(),
                    Err(TransportError::UnusableKey)
                ),
                "{key:?}"
            );
        }
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

    #[test]
    fn default_local_server_url_names_localhost_8000() {
        // Characterization (T-018 Acceptance 2, "cannot reach localhost:8000"): the
        // default local-server URL passes the URL rule and its CannotReach host is
        // `localhost:8000`. Bite: the default URL changed, or host_port dropping
        // the explicit port.
        let raw = crate::settings::defaults(None).local_server.base_url;
        let base = match crate::settings::url::check_base_url(&raw) {
            Ok(u) => u,
            Err(e) => panic!("default local-server URL {raw:?} refused: {e:?}"),
        };
        assert_eq!(host_port(&parsed(base.as_str())), "localhost:8000");
    }
}
