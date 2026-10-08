//! The shared OpenAI-compatible HTTP helpers (T-020): one client setup, one
//! `Authorization` rule and one capped send-and-read, used by both
//! [`super::openai::OpenAiCompatibleEngine`] (transcription) and
//! [`crate::post_process::chat::ChatPostProcessor`] (chat completions).
//!
//! Every failure is a [`TransportError`], which `failure::classify` maps to a
//! reason; no `reqwest::Error`, URL, key or body leaves this module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Read;
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::HeaderValue;
use zeroize::Zeroizing;

use super::openai::{body_error, send_error};
use crate::failure::TransportError;
use crate::secrets::Secret;

/// Largest accepted response body (contract: "body > 1 MiB" -> `UnexpectedResponse`).
pub(crate) const MAX_BODY: u64 = 1024 * 1024;

/// The `Authorization` value for `key`: `None` without a key (or with an empty
/// one). The one rule for a usable key: `Bearer <key>` passes `HeaderValue`
/// validation (http 1.5: no control byte other than tab, no DEL); otherwise
/// [`TransportError::UnusableKey`]. Bytes of a non-ASCII key pass that rule and
/// are sent as UTF-8; the server's 401/403 then gives `InvalidApiKey`. The value
/// is marked sensitive (not printed by `Debug`, not HPACK-indexed, and dropped by
/// reqwest's redirect policy on a cross-origin redirect).
pub(crate) fn authorization(key: Option<&Secret>) -> Result<Option<HeaderValue>, TransportError> {
    let Some(key) = key.filter(|k| !k.expose().is_empty()) else {
        return Ok(None);
    };
    let text = Zeroizing::new(format!("Bearer {}", key.expose()));
    let mut value = HeaderValue::from_str(&text).map_err(|_| TransportError::UnusableKey)?;
    value.set_sensitive(true);
    Ok(Some(value))
}

/// The blocking client for one call, with `connect` as its connect timeout and
/// reqwest's default redirect policy. The whole-request deadline is set per
/// request by the caller (`RequestBuilder::timeout`), which bounds connect to the
/// last body byte.
pub(crate) fn client(connect: Duration) -> Result<Client, TransportError> {
    Client::builder()
        .connect_timeout(connect)
        .build()
        .map_err(|_| TransportError::Setup)
}

/// Sends `request` and reads a 2xx body through the [`MAX_BODY`] cap. A non-2xx
/// status is [`TransportError::Status`] and its body is never read (P-009); a body
/// over the cap is [`TransportError::BadBody`].
pub(crate) fn send_capped(request: RequestBuilder) -> Result<Vec<u8>, TransportError> {
    let response = request.send().map_err(|e| send_error(&e))?;
    let status = response.status();
    if !status.is_success() {
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
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_absent_key_is_no_header() {
        // The key rule with no engine around it (the chat path calls it directly).
        // Bite: `Bearer ` sent for an empty key.
        assert!(matches!(authorization(None), Ok(None)));
        let empty = Secret::new("");
        assert!(matches!(authorization(Some(&empty)), Ok(None)));
    }
}
