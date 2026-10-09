//! Why a transcription failed, as the user and the log see it (spec 001 T038,
//! data-model "FailureReason", decision #44).
//!
//! A [`FailureReason`] is built only from an HTTP status, the transport
//! classification flags ([`TransportError`]) and the base URL's `host[:port]`, or
//! from the closed microphone cause ([`MicCause`]); never from a `reqwest::Error`,
//! a URL, a response body, a key or OS error text (P-009). So neither its
//! `Display` nor its `Debug` can carry a URL query, a key or a transcript.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::ErrorKind;

use crate::i18n::{self, MessageId};
use crate::recording::MicCause;

/// One failure of a transcription job. T-001 adds the delivery reasons.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FailureReason {
    /// HTTP 401 / 403; or a stored key that cannot be sent (`Bearer <key>` fails
    /// HTTP header value validation), in which case no request is sent.
    #[error("invalid API key")]
    InvalidApiKey,
    /// DNS failure; OS "network unreachable" / "host unreachable".
    #[error("network unavailable")]
    NetworkUnavailable,
    /// Connection refused, connect timeout, TLS handshake failure, client setup.
    /// `host` is the base URL's `host[:port]` (explicit port only).
    #[error("cannot reach {host}")]
    CannotReach { host: String },
    /// The whole request exceeded its `Timeouts` duration.
    #[error("the server did not answer in time")]
    Timeout,
    /// Any other non-2xx status.
    #[error("server error (HTTP {status})")]
    ServerError { status: u16 },
    /// A 2xx body that is not `{"text": "<string>"}`, larger than 1 MiB, not
    /// UTF-8, or cut off while reading.
    #[error("unexpected response from the server")]
    UnexpectedResponse,
    /// The key could not be read from the credential store (decision #44).
    #[error("the API key could not be read")]
    KeyStoreUnavailable,
    /// The selected engine cannot be built from the current settings (decision #44).
    #[error("the transcription engine is not set up")]
    EngineNotConfigured,
    /// The clipboard could not be written (T-001). Retryable: the recording is kept.
    #[error("clipboard unavailable")]
    ClipboardUnavailable,
    /// The microphone could not be opened or failed (T-042). Not retryable.
    /// Carries only the closed cause, never the OS error text.
    #[error("microphone unavailable")]
    MicrophoneUnavailable { cause: MicCause },
}

impl FailureReason {
    /// Stable code for events and logs (data-model "Code" column).
    pub fn code(&self) -> &'static str {
        match self {
            FailureReason::InvalidApiKey => "InvalidApiKey",
            FailureReason::NetworkUnavailable => "NetworkUnavailable",
            FailureReason::CannotReach { .. } => "CannotReach",
            FailureReason::Timeout => "Timeout",
            FailureReason::ServerError { .. } => "ServerError",
            FailureReason::UnexpectedResponse => "UnexpectedResponse",
            FailureReason::KeyStoreUnavailable => "KeyStoreUnavailable",
            FailureReason::EngineNotConfigured => "EngineNotConfigured",
            FailureReason::ClipboardUnavailable => "ClipboardUnavailable",
            FailureReason::MicrophoneUnavailable { .. } => "MicrophoneUnavailable",
        }
    }

    /// The catalog message (`failure.*`).
    pub fn message_id(&self) -> MessageId {
        match self {
            FailureReason::InvalidApiKey => i18n::FAILURE_INVALID_API_KEY,
            FailureReason::NetworkUnavailable => i18n::FAILURE_NETWORK_UNAVAILABLE,
            FailureReason::CannotReach { .. } => i18n::FAILURE_CANNOT_REACH,
            FailureReason::Timeout => i18n::FAILURE_TIMEOUT,
            FailureReason::ServerError { .. } => i18n::FAILURE_SERVER_ERROR,
            FailureReason::UnexpectedResponse => i18n::FAILURE_UNEXPECTED_RESPONSE,
            FailureReason::KeyStoreUnavailable => i18n::FAILURE_KEY_STORE_UNAVAILABLE,
            FailureReason::EngineNotConfigured => i18n::FAILURE_ENGINE_NOT_CONFIGURED,
            FailureReason::ClipboardUnavailable => i18n::FAILURE_CLIPBOARD_UNAVAILABLE,
            FailureReason::MicrophoneUnavailable { .. } => i18n::FAILURE_MICROPHONE_UNAVAILABLE,
        }
    }

    /// Placeholder values for [`message_id`](Self::message_id): `host` for
    /// `CannotReach`, `status` for `ServerError`, `reason` for
    /// `MicrophoneUnavailable` (the cause's `mic_reason.*` id, which
    /// [`i18n::text`] renders in the same language), none otherwise.
    pub fn message_params(&self) -> Vec<(&'static str, String)> {
        match self {
            FailureReason::CannotReach { host } => vec![("host", host.clone())],
            FailureReason::ServerError { status } => vec![("status", status.to_string())],
            FailureReason::MicrophoneUnavailable { cause } => {
                vec![("reason", cause.message_id().nested_arg())]
            }
            FailureReason::InvalidApiKey
            | FailureReason::NetworkUnavailable
            | FailureReason::Timeout
            | FailureReason::UnexpectedResponse
            | FailureReason::KeyStoreUnavailable
            | FailureReason::EngineNotConfigured
            | FailureReason::ClipboardUnavailable => Vec::new(),
        }
    }

    /// Whether the failed recording is kept as the pending recording (data-model
    /// "Retryable": every reason from `InvalidApiKey` through
    /// `ClipboardUnavailable`; not `MicrophoneUnavailable`).
    pub fn retryable(&self) -> bool {
        match self {
            FailureReason::InvalidApiKey
            | FailureReason::NetworkUnavailable
            | FailureReason::CannotReach { .. }
            | FailureReason::Timeout
            | FailureReason::ServerError { .. }
            | FailureReason::UnexpectedResponse
            | FailureReason::KeyStoreUnavailable
            | FailureReason::EngineNotConfigured
            | FailureReason::ClipboardUnavailable => true,
            FailureReason::MicrophoneUnavailable { .. } => false,
        }
    }
}

/// What went wrong in the transport, reduced to the facts the classification uses
/// (the reqwest adapter in `engine::openai` fills it; no URL, no body, no message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The HTTP client could not be built.
    Setup,
    /// The stored key cannot be an `Authorization` header value (`Bearer <key>`
    /// fails `HeaderValue` validation: a control byte other than tab, or DEL).
    /// Detected before the client or the request is built; nothing is sent.
    UnusableKey,
    /// A non-2xx status.
    Status(u16),
    /// The request failed before a response: reqwest's `is_dns` / `is_connect` /
    /// `is_timeout` flags, and the first `std::io::ErrorKind` in the source chain.
    Send {
        dns: bool,
        connect: bool,
        timeout: bool,
        io: Option<std::io::ErrorKind>,
    },
    /// Reading a 2xx body failed (stall past the timeout, or a reset / early close).
    BodyRead { timeout: bool },
    /// A 2xx body that is not the expected JSON, over 1 MiB, or not UTF-8.
    BadBody,
}

/// The one mapping from a transport failure to a reason. `host` is the base URL's
/// `host[:port]`.
///
/// Order (T-040 Investigation): status, or an unusable key -> `InvalidApiKey`; DNS or OS network/host unreachable ->
/// `NetworkUnavailable`; connect (refused, connect timeout, TLS) -> `CannotReach`;
/// timeout -> `Timeout`; body errors -> `UnexpectedResponse`. A DNS failure is
/// also `is_connect`, so DNS is checked first.
///
/// A send error with none of the flags (the connection closed before a response,
/// a protocol error) is `UnexpectedResponse`.
pub fn classify(err: &TransportError, host: &str) -> FailureReason {
    let cannot_reach = || FailureReason::CannotReach {
        host: host.to_string(),
    };
    match *err {
        TransportError::Status(401 | 403) | TransportError::UnusableKey => {
            FailureReason::InvalidApiKey
        }
        TransportError::Status(status) => FailureReason::ServerError { status },
        TransportError::Send { dns, io, .. }
            if dns
                || matches!(
                    io,
                    Some(ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable)
                ) =>
        {
            FailureReason::NetworkUnavailable
        }
        TransportError::Send { connect: true, .. } | TransportError::Setup => cannot_reach(),
        TransportError::Send { timeout: true, .. } | TransportError::BodyRead { timeout: true } => {
            FailureReason::Timeout
        }
        TransportError::Send { .. }
        | TransportError::BodyRead { timeout: false }
        | TransportError::BadBody => FailureReason::UnexpectedResponse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{text, UiLanguage, MESSAGE_IDS};
    use std::io::ErrorKind;

    const HOST: &str = "api.example.com:8443";

    fn send(dns: bool, connect: bool, timeout: bool, io: Option<ErrorKind>) -> TransportError {
        TransportError::Send {
            dns,
            connect,
            timeout,
            io,
        }
    }

    fn cannot_reach() -> FailureReason {
        FailureReason::CannotReach {
            host: HOST.to_string(),
        }
    }

    #[test]
    fn classify_table() {
        // Rows from the T-040 Investigation probe (reqwest 0.13.5 flags) and
        // contracts/openai-transcription.md. Bite: checking is_connect before
        // is_dns turns the `.invalid` row (dns AND connect) into CannotReach;
        // checking is_timeout before is_connect turns the connect-timeout row into
        // Timeout; a body-read timeout mapped to UnexpectedResponse breaks FR-24.
        let rows: Vec<(&str, TransportError, FailureReason)> = vec![
            (
                "401",
                TransportError::Status(401),
                FailureReason::InvalidApiKey,
            ),
            (
                "403",
                TransportError::Status(403),
                FailureReason::InvalidApiKey,
            ),
            (
                "key fails header value validation",
                TransportError::UnusableKey,
                FailureReason::InvalidApiKey,
            ),
            (
                "400",
                TransportError::Status(400),
                FailureReason::ServerError { status: 400 },
            ),
            (
                "404",
                TransportError::Status(404),
                FailureReason::ServerError { status: 404 },
            ),
            (
                "413",
                TransportError::Status(413),
                FailureReason::ServerError { status: 413 },
            ),
            (
                "429",
                TransportError::Status(429),
                FailureReason::ServerError { status: 429 },
            ),
            (
                "500",
                TransportError::Status(500),
                FailureReason::ServerError { status: 500 },
            ),
            (
                "503",
                TransportError::Status(503),
                FailureReason::ServerError { status: 503 },
            ),
            (
                "nohost.invalid: dns + connect",
                send(true, true, false, None),
                FailureReason::NetworkUnavailable,
            ),
            (
                "dns only",
                send(true, false, false, None),
                FailureReason::NetworkUnavailable,
            ),
            (
                "connect + io NetworkUnreachable",
                send(false, true, false, Some(ErrorKind::NetworkUnreachable)),
                FailureReason::NetworkUnavailable,
            ),
            (
                "connect + io HostUnreachable",
                send(false, true, false, Some(ErrorKind::HostUnreachable)),
                FailureReason::NetworkUnavailable,
            ),
            (
                "refused 127.0.0.1:9: connect + io ConnectionRefused",
                send(false, true, false, Some(ErrorKind::ConnectionRefused)),
                cannot_reach(),
            ),
            (
                "192.0.2.1 connect_timeout: connect + timeout",
                send(false, true, true, None),
                cannot_reach(),
            ),
            (
                "lookup unanswered at the connect deadline (T-079, #106): dns (resolver record) + connect + timeout",
                send(true, true, true, None),
                FailureReason::NetworkUnavailable,
            ),
            (
                "lookup unanswered at a whole-request deadline below connect (T-079, #113): dns (resolver record) + timeout",
                send(true, false, true, None),
                FailureReason::NetworkUnavailable,
            ),
            (
                "connect, no io kind (TLS handshake)",
                send(false, true, false, None),
                cannot_reach(),
            ),
            (
                "silent listener: timeout only",
                send(false, false, true, None),
                FailureReason::Timeout,
            ),
            // Contract openai-transcription.md › Classification 5: a send error with
            // none of the flags (closed before a response, protocol error) is
            // UnexpectedResponse, also with an io kind that is not a network one.
            // Bite: the flagless Send arm mapped to Timeout or CannotReach, or
            // ConnectionReset treated as a network/connect kind.
            (
                "send, no flags, no io kind",
                send(false, false, false, None),
                FailureReason::UnexpectedResponse,
            ),
            (
                "send, no flags, io ConnectionReset",
                send(false, false, false, Some(ErrorKind::ConnectionReset)),
                FailureReason::UnexpectedResponse,
            ),
            (
                "stall mid-body",
                TransportError::BodyRead { timeout: true },
                FailureReason::Timeout,
            ),
            (
                "reset mid-body",
                TransportError::BodyRead { timeout: false },
                FailureReason::UnexpectedResponse,
            ),
            (
                "bad body",
                TransportError::BadBody,
                FailureReason::UnexpectedResponse,
            ),
            ("client setup", TransportError::Setup, cannot_reach()),
        ];
        let mut wrong = Vec::new();
        for (name, err, expected) in rows {
            let got = classify(&err, HOST);
            if got != expected {
                wrong.push(format!("{name}: {err:?} -> {got:?}, expected {expected:?}"));
            }
        }
        assert!(wrong.is_empty(), "misclassified:\n{}", wrong.join("\n"));
    }

    /// Every variant once. The exhaustive match makes a new variant a compile error
    /// here until it is added to this list (and so to `message_ids_per_reason`).
    fn every_reason() -> Vec<FailureReason> {
        let all = vec![
            FailureReason::InvalidApiKey,
            FailureReason::NetworkUnavailable,
            FailureReason::CannotReach {
                host: HOST.to_string(),
            },
            FailureReason::Timeout,
            FailureReason::ServerError { status: 503 },
            FailureReason::UnexpectedResponse,
            FailureReason::KeyStoreUnavailable,
            FailureReason::EngineNotConfigured,
            FailureReason::ClipboardUnavailable,
            FailureReason::MicrophoneUnavailable {
                cause: MicCause::NoDevice,
            },
        ];
        for r in &all {
            match r {
                FailureReason::InvalidApiKey
                | FailureReason::NetworkUnavailable
                | FailureReason::CannotReach { .. }
                | FailureReason::Timeout
                | FailureReason::ServerError { .. }
                | FailureReason::UnexpectedResponse
                | FailureReason::KeyStoreUnavailable
                | FailureReason::EngineNotConfigured
                | FailureReason::ClipboardUnavailable
                | FailureReason::MicrophoneUnavailable { .. } => {}
            }
        }
        all
    }

    /// (code, message id, en text, ru text) per reason: data-model "FailureReason",
    /// contracts/messages.md, and the T-040 Investigation texts for the two
    /// decision-#44 reasons. Params as in `every_reason`.
    fn expected(r: &FailureReason) -> (&'static str, &'static str, &'static str, &'static str) {
        match r {
            FailureReason::InvalidApiKey => (
                "InvalidApiKey",
                "failure.invalid_api_key",
                "Invalid API key",
                "Неверный API-ключ",
            ),
            FailureReason::NetworkUnavailable => (
                "NetworkUnavailable",
                "failure.network_unavailable",
                "Network unavailable",
                "Сеть недоступна",
            ),
            FailureReason::CannotReach { .. } => (
                "CannotReach",
                "failure.cannot_reach",
                "Cannot reach api.example.com:8443",
                "Не удаётся подключиться к api.example.com:8443",
            ),
            FailureReason::Timeout => (
                "Timeout",
                "failure.timeout",
                "The server did not answer in time",
                "Сервер не ответил вовремя",
            ),
            FailureReason::ServerError { .. } => (
                "ServerError",
                "failure.server_error",
                "Server error (HTTP 503)",
                "Ошибка сервера (HTTP 503)",
            ),
            FailureReason::UnexpectedResponse => (
                "UnexpectedResponse",
                "failure.unexpected_response",
                "Unexpected response from the server",
                "Неожиданный ответ сервера",
            ),
            FailureReason::KeyStoreUnavailable => (
                "KeyStoreUnavailable",
                "failure.key_store_unavailable",
                "The API key could not be read from Windows Credential Manager.",
                "Не удалось прочитать API-ключ из диспетчера учётных данных Windows.",
            ),
            FailureReason::EngineNotConfigured => (
                "EngineNotConfigured",
                "failure.engine_not_configured",
                "The transcription engine is not set up. Check the Engine settings.",
                "Движок распознавания не настроен. Проверьте настройки движка.",
            ),
            // contracts/messages.md (T-001).
            FailureReason::ClipboardUnavailable => (
                "ClipboardUnavailable",
                "failure.clipboard_unavailable",
                "Clipboard unavailable",
                "Буфер обмена недоступен",
            ),
            // contracts/messages.md: `failure.microphone_unavailable` with
            // `{reason}` = the `mic_reason.*` text of the cause (NoDevice here).
            FailureReason::MicrophoneUnavailable { .. } => (
                "MicrophoneUnavailable",
                "failure.microphone_unavailable",
                "Microphone unavailable: no input device",
                "Микрофон недоступен: нет устройства ввода",
            ),
        }
    }

    #[test]
    fn message_ids_per_reason() {
        // Each reason has its stable code, its own `failure.*` id declared with
        // messages! (so `message_ids_exist_in_both_catalogs` covers it), and that
        // id renders the contract text in en and ru with the reason's params.
        // Bite: a shared/wrong id, an id not in MESSAGE_IDS, a missing catalog
        // entry (text() falls back to the id), `host`/`status` not passed.
        let mut wrong = Vec::new();
        for r in every_reason() {
            let (code, id, en, ru) = expected(&r);
            if r.code() != code {
                wrong.push(format!("{r:?}: code {:?}, expected {code:?}", r.code()));
            }
            let got_id = serde_json::to_value(r.message_id()).unwrap_or_default();
            if got_id != serde_json::Value::String(id.to_string()) {
                wrong.push(format!("{r:?}: message id {got_id}, expected {id:?}"));
            }
            if !MESSAGE_IDS.contains(&r.message_id()) {
                wrong.push(format!("{r:?}: message id not declared with messages!"));
            }
            let params = r.message_params();
            let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            for (lang, want) in [(UiLanguage::En, en), (UiLanguage::Ru, ru)] {
                let got = text(lang, r.message_id(), &args);
                if got != want {
                    wrong.push(format!("{r:?} {lang:?}: {got:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn retryable_table() {
        // data-model "FailureReason": every code from InvalidApiKey through
        // ClipboardUnavailable creates a pending recording; MicrophoneUnavailable
        // does not (no audio to retry). Written against every_reason, so a new
        // variant needs a row here. Bite: a constant answer, ClipboardUnavailable
        // or the decision-#44 reasons left out, MicrophoneUnavailable kept.
        let mut wrong = Vec::new();
        for r in every_reason() {
            let want = match r {
                FailureReason::InvalidApiKey
                | FailureReason::NetworkUnavailable
                | FailureReason::CannotReach { .. }
                | FailureReason::Timeout
                | FailureReason::ServerError { .. }
                | FailureReason::UnexpectedResponse
                | FailureReason::KeyStoreUnavailable
                | FailureReason::EngineNotConfigured
                | FailureReason::ClipboardUnavailable => true,
                FailureReason::MicrophoneUnavailable { .. } => false,
            };
            if r.retryable() != want {
                wrong.push(format!(
                    "{r:?}: retryable {}, expected {want}",
                    r.retryable()
                ));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn codes_are_distinct() {
        // Codes are what T-001's JobFinished event carries; two reasons with one
        // code cannot be told apart in the log. Bite: a shared fallback code.
        let codes: Vec<&str> = every_reason().iter().map(FailureReason::code).collect();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "codes not distinct: {codes:?}");
        assert!(codes.iter().all(|c| !c.is_empty()), "empty code: {codes:?}");
    }

    #[test]
    fn microphone_unavailable_renders_each_cause() {
        // contracts/messages.md: "Microphone unavailable: {reason}" with the
        // closed `mic_reason.*` set, in en and ru, one text per cause. Bite: one
        // reason text for every cause, the reason left as a raw placeholder or an
        // id, a cause rendered in the wrong language.
        let rows = [
            (
                MicCause::NoDevice,
                "no input device",
                "нет устройства ввода",
            ),
            (
                MicCause::AccessDenied,
                "access denied in Windows privacy settings",
                "доступ запрещён в настройках конфиденциальности Windows",
            ),
            (MicCause::Busy, "the device is busy", "устройство занято"),
            (
                MicCause::Other,
                "the device could not be opened",
                "не удалось открыть устройство",
            ),
        ];
        let mut wrong = Vec::new();
        for (cause, en, ru) in rows {
            let r = FailureReason::MicrophoneUnavailable { cause };
            let params = r.message_params();
            let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            for (lang, want) in [
                (UiLanguage::En, format!("Microphone unavailable: {en}")),
                (UiLanguage::Ru, format!("Микрофон недоступен: {ru}")),
            ] {
                let got = text(lang, r.message_id(), &args);
                if got != want {
                    wrong.push(format!("{cause:?} {lang:?}: {got:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }
}
