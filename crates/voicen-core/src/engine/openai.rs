//! The one OpenAI-compatible transcription client
//! (contracts/openai-transcription.md) for both the transcription API and the
//! local OpenAI-compatible server (T-018), and for the connection test (T-046,
//! `connection_test`). The client setup, the key rule and the capped body read
//! are the shared helpers in [`super::http`], which the chat post-processor
//! (T-020) uses too.
//!
//! The endpoint role is fixed at construction ([`OpenAiCompatibleEngine::new`] for
//! the API, [`OpenAiCompatibleEngine::local_server`] for the local server) and
//! picks the request deadline (`Timeouts::api_transcription` or
//! `Timeouts::local_server`) and `kind()` (`"api"` or `"local_server"`).
//!
//! `POST {base}/audio/transcriptions` (or the stored URL as is when it already
//! ends in `audio/transcriptions`: [`transcription_url`], decision #96), multipart `file` (`audio.wav`, `audio/wav`),
//! `model` (omitted when the engine has none: an unset local-server model),
//! `language` (omitted for auto), `response_format=json` ([`ResponseShape::Plain`])
//! or `response_format=verbose_json` plus `timestamp_granularities[]=segment`
//! ([`ResponseShape::Segments`], the call final pass: [`OpenAiCompatibleEngine::transcribe_timed`]);
//! `Authorization: Bearer <key>` only when a key is stored; a key whose
//! `Bearer <key>` text fails `HeaderValue` validation is `InvalidApiKey` with no
//! request sent ([`TransportError::UnusableKey`]). The URL is joined on the
//! parsed base URL, so a query string (decision #27(2)) survives. The blocking
//! client is built per call from `TranscribeRequest::timeouts` (connect and
//! whole-request). The body is read through a 1 MiB cap ([`super::http::MAX_BODY`]). Every failure goes
//! through `failure::classify`; no `reqwest::Error`, URL or body reaches a
//! `FailureReason`.

use std::error::Error as _;
use std::io::Read;

use reqwest::blocking::multipart;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};

use super::{http, Engine, TranscribeRequest};
use crate::audio::{wav, AudioBuffer};
use crate::call::events::FailReason;
use crate::call::transcript::EngineSegment;
use crate::failure::{classify, FailureReason, TransportError};
use crate::secrets::Secret;
use crate::settings::url::NormalizedUrl;
use crate::settings::LangCode;

/// Room for the multipart framing around the WAV and the text parts: five part
/// headers with a 67-character boundary, the fixed `response_format` and
/// `timestamp_granularities[]` values and the closing boundary are under 1 KiB.
const MULTIPART_OVERHEAD: usize = 1024;

/// What a [`OpenAiCompatibleEngine::transcribe_timed`] request asks the server for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseShape {
    /// `response_format=verbose_json` and `timestamp_granularities[]=segment`.
    Segments,
    /// `response_format=json`, the dictation request.
    Plain,
}

/// A transcription with its optional segment times. `segments` is `Some` exactly
/// for an OK [`ResponseShape::Segments`] reply (possibly empty: silence) and always
/// `None` for [`ResponseShape::Plain`]. `language` is the reply's top-level
/// language mapped onto [`LangCode`] (`None` when absent or unknown); `text` is
/// trimmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub text: String,
    pub language: Option<LangCode>,
    pub segments: Option<Vec<EngineSegment>>,
}

/// Why [`OpenAiCompatibleEngine::transcribe_timed`] returned no [`Timed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimedFailure {
    /// A [`ResponseShape::Segments`] request the server answered without segment
    /// times (status 400 or 422, or a 2xx whose `segments` is absent, null, or
    /// empty while the text is not blank): the caller re-sends as `Plain` (FR-024).
    Timeless,
    /// Any other failure, as the final pass's reason.
    Failed(FailReason),
}

/// The whisper.cpp language names (`g_lang` of whisper-rs-sys 0.15.0) with their
/// codes, in the order of [`crate::settings::WHISPER_ISO_639_1`]; `haw`, `jw` and
/// `yue` (not in that list) left out.
const WHISPER_NAMES: [(&str, &str); 97] = [
    ("en", "english"),
    ("zh", "chinese"),
    ("de", "german"),
    ("es", "spanish"),
    ("ru", "russian"),
    ("ko", "korean"),
    ("fr", "french"),
    ("ja", "japanese"),
    ("pt", "portuguese"),
    ("tr", "turkish"),
    ("pl", "polish"),
    ("ca", "catalan"),
    ("nl", "dutch"),
    ("ar", "arabic"),
    ("sv", "swedish"),
    ("it", "italian"),
    ("id", "indonesian"),
    ("hi", "hindi"),
    ("fi", "finnish"),
    ("vi", "vietnamese"),
    ("he", "hebrew"),
    ("uk", "ukrainian"),
    ("el", "greek"),
    ("ms", "malay"),
    ("cs", "czech"),
    ("ro", "romanian"),
    ("da", "danish"),
    ("hu", "hungarian"),
    ("ta", "tamil"),
    ("no", "norwegian"),
    ("th", "thai"),
    ("ur", "urdu"),
    ("hr", "croatian"),
    ("bg", "bulgarian"),
    ("lt", "lithuanian"),
    ("la", "latin"),
    ("mi", "maori"),
    ("ml", "malayalam"),
    ("cy", "welsh"),
    ("sk", "slovak"),
    ("te", "telugu"),
    ("fa", "persian"),
    ("lv", "latvian"),
    ("bn", "bengali"),
    ("sr", "serbian"),
    ("az", "azerbaijani"),
    ("sl", "slovenian"),
    ("kn", "kannada"),
    ("et", "estonian"),
    ("mk", "macedonian"),
    ("br", "breton"),
    ("eu", "basque"),
    ("is", "icelandic"),
    ("hy", "armenian"),
    ("ne", "nepali"),
    ("mn", "mongolian"),
    ("bs", "bosnian"),
    ("kk", "kazakh"),
    ("sq", "albanian"),
    ("sw", "swahili"),
    ("gl", "galician"),
    ("mr", "marathi"),
    ("pa", "punjabi"),
    ("si", "sinhala"),
    ("km", "khmer"),
    ("sn", "shona"),
    ("yo", "yoruba"),
    ("so", "somali"),
    ("af", "afrikaans"),
    ("oc", "occitan"),
    ("ka", "georgian"),
    ("be", "belarusian"),
    ("tg", "tajik"),
    ("sd", "sindhi"),
    ("gu", "gujarati"),
    ("am", "amharic"),
    ("yi", "yiddish"),
    ("lo", "lao"),
    ("uz", "uzbek"),
    ("fo", "faroese"),
    ("ht", "haitian creole"),
    ("ps", "pashto"),
    ("tk", "turkmen"),
    ("nn", "nynorsk"),
    ("mt", "maltese"),
    ("sa", "sanskrit"),
    ("lb", "luxembourgish"),
    ("my", "myanmar"),
    ("bo", "tibetan"),
    ("tl", "tagalog"),
    ("mg", "malagasy"),
    ("as", "assamese"),
    ("tt", "tatar"),
    ("ln", "lingala"),
    ("ha", "hausa"),
    ("ba", "bashkir"),
    ("su", "sundanese"),
];

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
    /// when set, then `response_format=json` for [`ResponseShape::Plain`] or
    /// `response_format=verbose_json` and `timestamp_granularities[]=segment` for
    /// [`ResponseShape::Segments`]) and its `Content-Type`.
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
        shape: ResponseShape,
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
        let form = match shape {
            ResponseShape::Plain => form.text("response_format", "json"),
            ResponseShape::Segments => form
                .text("response_format", "verbose_json")
                .text("timestamp_granularities[]", "segment"),
        };
        let content_type = format!("multipart/form-data; boundary={}", form.boundary());
        let mut body = Vec::with_capacity(capacity);
        form.into_reader().read_to_end(&mut body).ok()?;
        Some((content_type, body))
    }

    /// The `Authorization` value for the stored key, by the one shared rule
    /// ([`http::authorization`]): `None` without a key (or with an empty one),
    /// [`TransportError::UnusableKey`] when `Bearer <key>` is not a valid header
    /// value, else the value marked sensitive.
    fn authorization(&self) -> Result<Option<HeaderValue>, TransportError> {
        http::authorization(self.key.as_ref())
    }

    /// Sends the request for `shape` and reads the 2xx body through the 1 MiB cap;
    /// every failure as a [`TransportError`].
    fn send(
        &self,
        url: url::Url,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Vec<u8>, TransportError> {
        // The key is checked before anything is built or sent.
        let authorization = self.authorization()?;
        let client = http::client(req.timeouts.connect)?;
        let (content_type, body) = self
            .multipart_body(audio, req, shape)
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
        // A non-2xx body is never read (P-009); a 2xx one through the 1 MiB cap.
        client.send_capped(request)
    }

    /// The call final pass's engine call (contracts/core-call.md): one request for
    /// `shape`, through the same client, key rule and capped read as `transcribe`.
    /// Exactly one of `Ok(Timed)`, `Err(Timeless)` or `Err(Failed(reason))`, decided
    /// by [`timed_outcome`]; never a retry inside the call.
    pub fn transcribe_timed(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure> {
        let not_set_up = || TimedFailure::Failed(fail_reason(&FailureReason::EngineNotConfigured));
        let base = url::Url::parse(self.base_url.as_str()).map_err(|_| not_set_up())?;
        let url = transcription_url(&base).ok_or_else(not_set_up)?;
        let result = self
            .send(url, audio, req, shape)
            .and_then(|body| match shape {
                ResponseShape::Plain => parse_plain(&body).map(Some),
                ResponseShape::Segments => parse_verbose(&body),
            });
        timed_outcome(result, shape, &host_port(&base))
    }
}

/// The one decision of a `transcribe_timed` outcome. `Ok(None)` is a 2xx
/// `Segments` reply without usable times. Time-less only for `Segments`, and only
/// for that reply or status 400 | 422; everything else goes through
/// `failure::classify` and [`fail_reason`] (401/403 is `Key`, never time-less).
fn timed_outcome(
    result: Result<Option<Timed>, TransportError>,
    shape: ResponseShape,
    host: &str,
) -> Result<Timed, TimedFailure> {
    match result {
        Ok(Some(timed)) => Ok(timed),
        Ok(None) => Err(TimedFailure::Timeless),
        Err(TransportError::Status(400 | 422)) if shape == ResponseShape::Segments => {
            Err(TimedFailure::Timeless)
        }
        Err(e) => Err(TimedFailure::Failed(fail_reason(&classify(&e, host)))),
    }
}

/// A [`FailureReason`] as the final pass's [`FailReason`] (decides_on 4): the
/// reasons `transcribe_timed` cannot produce are `Provider`.
fn fail_reason(reason: &FailureReason) -> FailReason {
    match reason {
        FailureReason::InvalidApiKey => FailReason::Key,
        FailureReason::NetworkUnavailable | FailureReason::CannotReach { .. } => {
            FailReason::Network
        }
        FailureReason::Timeout { .. } => FailReason::Timeout,
        FailureReason::EngineNotConfigured => FailReason::EngineNotSetUp,
        FailureReason::ServerError { .. }
        | FailureReason::UnexpectedResponse
        | FailureReason::KeyStoreUnavailable
        | FailureReason::ClipboardUnavailable
        | FailureReason::MicrophoneUnavailable { .. }
        | FailureReason::NoLocalModel
        | FailureReason::LocalEngineFailed => FailReason::Provider,
    }
}

/// An engine language value onto [`LangCode`], the one mapping (decides_on 3):
/// ASCII-lowercased, then a code of `WHISPER_ISO_639_1`, else a whisper.cpp name
/// of [`WHISPER_NAMES`], else `None`. No trimming, no aliases.
fn engine_lang(raw: &str) -> Option<LangCode> {
    let lower = raw.to_ascii_lowercase();
    LangCode::parse(&lower).or_else(|| {
        WHISPER_NAMES
            .iter()
            .find(|(_, name)| *name == lower)
            .and_then(|(code, _)| LangCode::parse(code))
    })
}

/// A JSON value's language through [`engine_lang`]; a non-string is `None`.
fn value_lang(value: Option<&serde_json::Value>) -> Option<LangCode> {
    value
        .and_then(serde_json::Value::as_str)
        .and_then(engine_lang)
}

/// The body as a JSON object with a string `text`. (A derived struct would also
/// accept a JSON array whose first element is a string.)
fn reply_object(
    body: &[u8],
) -> Result<(serde_json::Map<String, serde_json::Value>, String), TransportError> {
    let parsed: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| TransportError::BadBody)?;
    let serde_json::Value::Object(object) = parsed else {
        return Err(TransportError::BadBody);
    };
    let text = object
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or(TransportError::BadBody)?
        .trim()
        .to_string();
    Ok((object, text))
}

/// A `json` reply: the trimmed `text`, the mapped top-level `language`, no
/// segments (whatever else the reply carries).
fn parse_plain(body: &[u8]) -> Result<Timed, TransportError> {
    let (object, text) = reply_object(body)?;
    Ok(Timed {
        text,
        language: value_lang(object.get("language")),
        segments: None,
    })
}

/// A `verbose_json` reply. `Ok(None)` when it has no usable times: `segments`
/// absent or null, or empty while `text` is not blank. A `segments` value that is
/// not an array of objects with finite, non-negative numeric `start <= end` and a
/// string `text` is [`TransportError::BadBody`].
fn parse_verbose(body: &[u8]) -> Result<Option<Timed>, TransportError> {
    let (object, text) = reply_object(body)?;
    let raw = match object.get("segments") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Array(raw)) => raw,
        Some(_) => return Err(TransportError::BadBody),
    };
    let segments = raw
        .iter()
        .map(parse_segment)
        .collect::<Result<Vec<_>, _>>()?;
    if segments.is_empty() && !text.is_empty() {
        return Ok(None);
    }
    Ok(Some(Timed {
        text,
        language: value_lang(object.get("language")),
        segments: Some(segments),
    }))
}

/// One `verbose_json` segment (seconds); see [`parse_verbose`] for what is malformed.
fn parse_segment(value: &serde_json::Value) -> Result<EngineSegment, TransportError> {
    let object = value.as_object().ok_or(TransportError::BadBody)?;
    let seconds = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .filter(|s| s.is_finite() && *s >= 0.0)
            .ok_or(TransportError::BadBody)
    };
    let start_s = seconds("start")?;
    let end_s = seconds("end")?;
    if end_s < start_s {
        return Err(TransportError::BadBody);
    }
    let text = object
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or(TransportError::BadBody)?
        .to_string();
    Ok(EngineSegment {
        start_s,
        end_s,
        text,
        lang: value_lang(object.get("language")),
    })
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
        self.send(url, audio, req, ResponseShape::Plain)
            .and_then(|body| parse_plain(&body))
            .map(|timed| timed.text)
            .map_err(|e| classify(&e, &host_port(&base)))
    }
}

/// The classification flags of a send error: reqwest's flags and the first
/// `io::ErrorKind` in the source chain. `lookup_unanswered` is the client's lookup
/// record (`engine::http::LookupRecord`) when the send failed: a lookup still
/// without an answer makes it a DNS failure, whichever timer ended the send
/// (T-079, decisions #106, #113). Nothing of the error's text is kept (its
/// `Display` contains the URL, query included).
pub(crate) fn send_error(e: &reqwest::Error, lookup_unanswered: bool) -> TransportError {
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
        dns: e.is_dns() || lookup_unanswered,
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
                "http://localhost:8000/v1/audio/transcriptions",
                "http://localhost:8000/v1/audio/transcriptions",
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
            let Some((_, body)) = engine.multipart_body(&audio, &req, ResponseShape::Plain) else {
                panic!("the body is encoded");
            };
            let expected = body_capacity(wav::encode(&audio).len(), Some("whisper-1"), language);
            assert_eq!(body.capacity(), expected, "language {language:?}");
            assert!(body.len() <= expected);
        }
    }

    #[test]
    fn multipart_body_is_sized_once_from_the_wav_for_both_shapes() {
        // T-136: the Segments form has a fifth part (`timestamp_granularities[]`);
        // the buffer is still reserved once from the WAV length, the framing still
        // fits MULTIPART_OVERHEAD, and the form never regrows it. The shape decides
        // the `response_format` text and the extra part. Bite: a Segments body
        // outgrowing `body_capacity` (capacity differs), `Vec::new()`, the shape
        // ignored (the same text for both).
        let audio = AudioBuffer::from_16k_mono(vec![0; 16_000 * 10]);
        for (shape, language) in [
            (ResponseShape::Segments, None),
            (ResponseShape::Segments, Some("de")),
            (ResponseShape::Plain, None),
            (ResponseShape::Plain, Some("de")),
        ] {
            let req = TranscribeRequest {
                language: language.map(str::to_string),
                timeouts: crate::timeouts::Timeouts::default(),
            };
            let Some((content_type, body)) = engine(None).multipart_body(&audio, &req, shape)
            else {
                panic!("the body is encoded");
            };
            let expected = body_capacity(wav::encode(&audio).len(), Some("whisper-1"), language);
            assert_eq!(body.capacity(), expected, "{shape:?} {language:?}");
            assert!(body.len() <= expected, "{shape:?} {language:?}");
            assert!(content_type.starts_with("multipart/form-data; boundary="));

            let text = String::from_utf8_lossy(&body);
            let has_granularity = text.contains("name=\"timestamp_granularities[]\"");
            let has_verbose = text.contains("verbose_json");
            match shape {
                ResponseShape::Segments => {
                    assert!(
                        has_verbose && has_granularity,
                        "{shape:?}: {has_verbose} {has_granularity}"
                    );
                }
                ResponseShape::Plain => {
                    assert!(
                        !has_verbose && !has_granularity,
                        "{shape:?}: {has_verbose} {has_granularity}"
                    );
                    assert!(text.contains("name=\"response_format\""));
                }
            }
        }
    }

    // ---- T-136: the language mapping and the failure mapping (pure) ----------

    fn lang(code: &str) -> Option<crate::settings::LangCode> {
        crate::settings::LangCode::parse(code)
    }

    #[test]
    fn whisper_name_table_code_column_is_whisper_iso_639_1() {
        // #196(4): the 97 names of whisper.cpp's g_lang carry exactly the codes of
        // WHISPER_ISO_639_1, in that order, so the two lists cannot drift. Bite: a
        // dropped or added row (count), a reordered or mistyped code, haw/jw/yue
        // left in.
        assert_eq!(
            WHISPER_NAMES.len(),
            crate::settings::WHISPER_ISO_639_1.len()
        );
        assert_eq!(WHISPER_NAMES.len(), 97);
        let codes: Vec<&str> = WHISPER_NAMES.iter().map(|(code, _)| *code).collect();
        assert_eq!(codes, crate::settings::WHISPER_ISO_639_1.to_vec());
    }

    #[test]
    fn whisper_names_are_lowercase_unique_and_each_maps_to_its_code() {
        // Every row round-trips through engine_lang, whatever the case the server
        // used ("English", "ENGLISH"), and no two rows share a name. Bite: a name
        // typed with a capital or a trailing space (not found), a duplicate name
        // shadowing another code, engine_lang matching case-sensitively.
        let mut seen = std::collections::BTreeSet::new();
        for (code, name) in WHISPER_NAMES.iter() {
            assert_eq!(*name, name.to_ascii_lowercase(), "{name:?} is lowercase");
            assert_eq!(*name, name.trim(), "{name:?} is trimmed");
            assert!(!name.is_empty());
            assert!(seen.insert(*name), "{name:?} twice");
            assert_eq!(engine_lang(name), lang(code), "{name:?}");
            assert_eq!(
                engine_lang(&name.to_ascii_uppercase()),
                lang(code),
                "{name:?} upper"
            );
        }
        // The spellings of g_lang that differ from the common English name.
        assert_eq!(engine_lang("haitian creole"), lang("ht"));
        assert_eq!(engine_lang("myanmar"), lang("my"));
    }

    #[test]
    fn engine_lang_maps_names_and_codes_and_drops_the_rest() {
        // Acceptance: "english", "English" and `en` map to `en`; an unknown value
        // maps to None. Bite: no ASCII-lowercase, no LangCode::parse arm ('en'), a
        // fallback to "en" or to the raw text, haw (Hawaiian is not in the 97) mapped.
        for raw in ["english", "English", "ENGLISH", "en"] {
            assert_eq!(engine_lang(raw), lang("en"), "{raw:?}");
        }
        assert_eq!(engine_lang("german"), lang("de"));
        // The value is ASCII-lowercased first (decides_on 3), codes included.
        assert_eq!(engine_lang("DE"), lang("de"));
        for raw in [
            "klingon", "haw", "hawaiian", "jw", "yue", "", " ", "english ", "xx",
        ] {
            assert_eq!(engine_lang(raw), None, "{raw:?}");
        }
    }

    /// Every `FailureReason` once; the exhaustive match makes a new reason a
    /// compile error here until the mapping table below has a row for it.
    fn every_reason() -> Vec<FailureReason> {
        let all = vec![
            FailureReason::InvalidApiKey,
            FailureReason::NetworkUnavailable,
            FailureReason::CannotReach {
                host: "api.example.com:8443".to_string(),
            },
            FailureReason::Timeout {
                origin: crate::failure::TimeoutOrigin::Server,
            },
            FailureReason::ServerError { status: 503 },
            FailureReason::UnexpectedResponse,
            FailureReason::KeyStoreUnavailable,
            FailureReason::EngineNotConfigured,
            FailureReason::ClipboardUnavailable,
            FailureReason::MicrophoneUnavailable {
                cause: crate::recording::MicCause::NoDevice,
            },
            FailureReason::NoLocalModel,
            FailureReason::LocalEngineFailed,
        ];
        for reason in &all {
            match reason {
                FailureReason::InvalidApiKey
                | FailureReason::NetworkUnavailable
                | FailureReason::CannotReach { .. }
                | FailureReason::Timeout { .. }
                | FailureReason::ServerError { .. }
                | FailureReason::UnexpectedResponse
                | FailureReason::KeyStoreUnavailable
                | FailureReason::EngineNotConfigured
                | FailureReason::ClipboardUnavailable
                | FailureReason::MicrophoneUnavailable { .. }
                | FailureReason::NoLocalModel
                | FailureReason::LocalEngineFailed => {}
            }
        }
        all
    }

    #[test]
    fn failure_reason_maps_onto_fail_reason_for_every_reason() {
        // decides_on (4): InvalidApiKey -> Key; NetworkUnavailable and CannotReach ->
        // Network; Timeout -> Timeout; ServerError and UnexpectedResponse ->
        // Provider; EngineNotConfigured -> EngineNotSetUp; the variants
        // transcribe_timed cannot produce -> Provider. Bite: 401 left as Provider,
        // CannotReach as Provider, a wildcard arm hiding a row, Timeout as Network.
        use crate::call::events::FailReason;
        let reasons = every_reason();
        assert_eq!(reasons.len(), 12);
        for reason in &reasons {
            let expected = match reason {
                FailureReason::InvalidApiKey => FailReason::Key,
                FailureReason::NetworkUnavailable | FailureReason::CannotReach { .. } => {
                    FailReason::Network
                }
                FailureReason::Timeout { .. } => FailReason::Timeout,
                FailureReason::EngineNotConfigured => FailReason::EngineNotSetUp,
                FailureReason::ServerError { .. }
                | FailureReason::UnexpectedResponse
                | FailureReason::KeyStoreUnavailable
                | FailureReason::ClipboardUnavailable
                | FailureReason::MicrophoneUnavailable { .. }
                | FailureReason::NoLocalModel
                | FailureReason::LocalEngineFailed => FailReason::Provider,
            };
            assert_eq!(fail_reason(reason), expected, "{reason:?}");
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
