//! The OpenAI-compatible chat post-processor (spec 003 FR-001..FR-007,
//! contracts/core-post-process.md, research R4/R5; T-020, decisions #91, #99).
//!
//! Order per call, first match:
//! 1. `enabled` false -> [`PostProcessOutcome::NotRun`]: no credential read, no
//!    request (FR-002, NFR-05).
//! 2. The base URL fails `check_base_url` -> `Skipped(NotConfigured)` (#91(2)),
//!    still without a credential read (as `engine_for`).
//! 3. One read of `KeySlot::PostProcessing`; a read error is no key (spec 003 Edge
//!    Cases, research R5; deliberately unlike transcription's decision #44).
//! 4. One `POST {base}/chat/completions` ([`chat_completions_url`]) with
//!    `{"model", "messages": [system prompt, user raw]}`, through the shared
//!    helpers of `engine::http` (client with `timeouts.connect`, the one
//!    `Authorization` rule, the 1 MiB capped read); the whole request is bounded
//!    by `timeouts.post_processing`. The default redirect policy drops the key on
//!    a cross-origin redirect.
//! 5. `choices[0].message.content` as a string, trimmed, non-empty -> `Applied`;
//!    anything else -> `Skipped(InvalidResponse)`; a transport failure ->
//!    `failure::classify` -> [`SkipReason::from_failure`].
//!
//! Synchronous (decision #42): call it on a plain std thread, never inside a tokio
//! runtime (the blocking reqwest client panics there). No retries.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;

use super::settings::PostProcessingSettings;
use super::{PostProcessInput, PostProcessOutcome, PostProcessor, SkipReason};
use crate::engine::http;
use crate::engine::openai::host_port;
use crate::failure::{classify, TransportError};
use crate::secrets::{KeySlot, Secret};
use crate::settings::url::check_base_url;
use crate::timeouts::Timeouts;

/// The chat post-processor. Holds no state: everything comes from each call's
/// [`PostProcessInput`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatPostProcessor;

impl ChatPostProcessor {
    pub fn new() -> ChatPostProcessor {
        ChatPostProcessor
    }
}

impl PostProcessor for ChatPostProcessor {
    fn process(&self, raw: &str, input: &PostProcessInput<'_>) -> PostProcessOutcome {
        let settings = input.settings;
        if !settings.enabled {
            return PostProcessOutcome::NotRun;
        }
        let Some(base) = check_base_url(&settings.base_url)
            .ok()
            .and_then(|normalized| url::Url::parse(normalized.as_str()).ok())
        else {
            return PostProcessOutcome::Skipped(SkipReason::NotConfigured);
        };
        let Some(url) = chat_completions_url(&base) else {
            return PostProcessOutcome::Skipped(SkipReason::NotConfigured);
        };
        let key = input
            .credentials
            .read(KeySlot::PostProcessing)
            .ok()
            .flatten();
        let host = host_port(&base);
        match send(url, settings, key.as_ref(), input.timeouts, raw) {
            Ok(text) => PostProcessOutcome::Applied(text),
            Err(e) => {
                PostProcessOutcome::Skipped(SkipReason::from_failure(&classify(&e, &host), &host))
            }
        }
    }
}

/// The request URL for the stored base URL: one empty trailing path segment
/// dropped, then `chat`, `completions` appended; the query (e.g. `api-version`)
/// kept (spec 003 FR-001, decision #27(2)). `None` only for a cannot-be-a-base URL
/// (never `http`/`https`).
pub(crate) fn chat_completions_url(base: &url::Url) -> Option<url::Url> {
    let mut url = base.clone();
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(["chat", "completions"]);
    Some(url)
}

/// Sends the chat request and returns the trimmed, non-empty reply text.
fn send(
    url: url::Url,
    settings: &PostProcessingSettings,
    key: Option<&Secret>,
    timeouts: &Timeouts,
    raw: &str,
) -> Result<String, TransportError> {
    // The key is checked before anything is built or sent.
    let authorization = http::authorization(key)?;
    let client = http::client(timeouts.connect)?;
    let body = serde_json::to_vec(&json!({
        "model": settings.model,
        "messages": [
            { "role": "system", "content": settings.prompt },
            { "role": "user", "content": raw },
        ],
    }))
    .map_err(|_| TransportError::Setup)?;
    let mut request = client
        .post(url)
        .timeout(timeouts.post_processing)
        .header(CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(value) = authorization {
        request = request.header(AUTHORIZATION, value);
    }
    let body = client.send_capped(request)?;
    reply_text(&body).ok_or(TransportError::BadBody)
}

/// `choices[0].message.content` of a chat reply as a trimmed, non-empty string;
/// `None` for anything else (not JSON or not UTF-8, no such path, not a string,
/// blank).
fn reply_text(body: &[u8]) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_slice(body).ok()?;
    let text = parsed
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(raw: &str) -> Option<String> {
        let base = match url::Url::parse(raw) {
            Ok(u) => u,
            Err(e) => panic!("test URL {raw:?}: {e}"),
        };
        chat_completions_url(&base).map(|u| u.to_string())
    }

    #[test]
    fn chat_url_appends_and_keeps_the_query() {
        // spec 003 FR-001: append-only join on the parsed URL. Bite: string
        // concatenation (double slash, the path inside the query), `Url::join`
        // (drops the last segment and the query).
        for (raw, expected) in [
            (
                "https://api.example.com/v1",
                "https://api.example.com/v1/chat/completions",
            ),
            (
                "https://api.example.com/v1/",
                "https://api.example.com/v1/chat/completions",
            ),
            (
                "https://api.example.com",
                "https://api.example.com/chat/completions",
            ),
            (
                "http://127.0.0.1:8080/v1?api-version=2024-06-01",
                "http://127.0.0.1:8080/v1/chat/completions?api-version=2024-06-01",
            ),
        ] {
            assert_eq!(joined(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn reply_text_reads_only_a_non_blank_string_content() {
        // data-model "ChatReply". Bite: content defaulted to "", a number coerced.
        assert_eq!(
            reply_text(br#"{"choices":[{"message":{"content":"  ok \n"}}]}"#).as_deref(),
            Some("ok")
        );
        for body in [
            &b""[..],
            br#"{"choices":[]}"#,
            br#"{"choices":[{"message":{"content":42}}]}"#,
            br#"{"choices":[{"message":{"content":"   "}}]}"#,
        ] {
            assert_eq!(reply_text(body), None, "{body:?}");
        }
    }
}
