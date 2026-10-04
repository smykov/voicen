//! The one source of network and engine durations (FR-24; spec 001 T006).
//!
//! `Timeouts::default()` is the only constructor with production values; tests
//! override single fields with milliseconds
//! (`Timeouts { api_transcription: Duration::from_millis(300), ..Timeouts::default() }`).
//! The engine stores no `Duration` of its own: it reads them from the
//! `TranscribeRequest` of each call.

use std::time::Duration;

/// Durations per role (data-model "Timeouts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// TCP/TLS connect, every HTTP role (FR-24: 5 s).
    pub connect: Duration,
    /// The whole API transcription request, connect to last body byte (FR-24: 30 s).
    pub api_transcription: Duration,
    /// The whole local-server transcription request (spec 002: 60 s).
    pub local_server: Duration,
    /// The whole post-processing request (spec 003: 15 s).
    pub post_processing: Duration,
    /// Built-in whisper.cpp transcription (decision #7: 120 s).
    pub builtin: Duration,
    /// Model download: the longest wait for the next body bytes (or the response
    /// headers), a per-read timeout, never a total one (spec 002 Clarification 5:
    /// 30 s; decision #49).
    pub download_no_data: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(5),
            api_transcription: Duration::from_secs(30),
            local_server: Duration::from_secs(60),
            post_processing: Duration::from_secs(15),
            builtin: Duration::from_secs(120),
            // Skeleton (T-016 red tests): the production value is set by the
            // implementation (test `download_no_data_default_is_30s`).
            download_no_data: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_fr24() {
        // FR-24 (connect 5 s, API 30 s), spec 002 (local server 60 s), spec 003
        // (post-processing 15 s), decision #7 (built-in 120 s). Bite: any value
        // changed, or two roles swapped.
        let t = Timeouts::default();
        assert_eq!(t.connect, Duration::from_secs(5), "connect");
        assert_eq!(
            t.api_transcription,
            Duration::from_secs(30),
            "api_transcription"
        );
        assert_eq!(t.local_server, Duration::from_secs(60), "local_server");
        assert_eq!(
            t.post_processing,
            Duration::from_secs(15),
            "post_processing"
        );
        assert_eq!(t.builtin, Duration::from_secs(120), "builtin");
    }

    #[test]
    fn download_no_data_default_is_30s() {
        // Spec 002 Clarification 5 (no data for 30 s fails the download), decision
        // #49. Bite: the field left at another value, or set to a total-download
        // duration such as 60 s / 120 s.
        assert_eq!(
            Timeouts::default().download_no_data,
            Duration::from_secs(30),
            "download_no_data"
        );
    }
}
