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
            download_no_data: Duration::from_secs(30),
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

    // ---- T-073: the settings are the source of the dictation durations (#97, #99) ----

    use crate::settings::{defaults, TimeoutSettings};

    /// The bounds of decision #99 per role: (default, min, max) in whole seconds.
    const CONNECT: (u32, u32, u32) = (5, 1, 60);
    const API: (u32, u32, u32) = (30, 5, 600);
    const LOCAL: (u32, u32, u32) = (60, 5, 1800);
    const POST: (u32, u32, u32) = (15, 5, 300);
    const BUILTIN: (u32, u32, u32) = (120, 10, 1800);

    fn secs(connect: u32, api: u32, local: u32, post: u32, builtin: u32) -> TimeoutSettings {
        TimeoutSettings {
            connect_s: connect,
            api_transcription_s: api,
            local_server_s: local,
            post_processing_s: post,
            builtin_local_s: builtin,
        }
    }

    /// The `Timeouts` each role should get, download no-data fixed at 30 s (#99 Q1).
    fn expected(connect: u32, api: u32, local: u32, post: u32, builtin: u32) -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(connect.into()),
            api_transcription: Duration::from_secs(api.into()),
            local_server: Duration::from_secs(local.into()),
            post_processing: Duration::from_secs(post.into()),
            builtin: Duration::from_secs(builtin.into()),
            download_no_data: Duration::from_secs(30),
        }
    }

    #[test]
    fn default_is_from_settings_defaults() {
        // Invariant (T-073 analysis): one source of the FR-24 numbers. The settings
        // defaults are the FR-24 values, and Timeouts::default() is exactly the
        // conversion of them. Bite: a default changed on one side only (the settings
        // default or Timeouts::default), two roles swapped in from_settings, or
        // download_no_data taken from a setting.
        let d = defaults(None).timeouts;
        assert_eq!(d, secs(CONNECT.0, API.0, LOCAL.0, POST.0, BUILTIN.0));
        assert_eq!(Timeouts::from_settings(&d), Timeouts::default());
        assert_eq!(
            Timeouts::default(),
            expected(CONNECT.0, API.0, LOCAL.0, POST.0, BUILTIN.0)
        );
    }

    #[test]
    fn from_settings_converts_each_role_in_whole_seconds() {
        // Distinct in-range values per role, so a swapped or misread field shows.
        // Bite: a role read from another setting, milliseconds instead of seconds,
        // or a constant (the default) returned.
        let s = secs(7, 45, 90, 20, 150);
        assert_eq!(Timeouts::from_settings(&s), expected(7, 45, 90, 20, 150));
        let s = secs(2, 9, 11, 6, 33);
        assert_eq!(Timeouts::from_settings(&s), expected(2, 9, 11, 6, 33));
    }

    #[test]
    fn from_settings_clamps() {
        // A hand-edited file is not validated on load (only a save is), so the
        // conversion clamps into the #99 bounds: a `connect_s: 0` must not make
        // every request fail at once, and u32::MAX must not wait forever. The
        // boundaries themselves pass unchanged. Bite: no clamp, a clamp with another
        // table (off by one at a boundary), or download_no_data clamped/changed.
        let mins = expected(CONNECT.1, API.1, LOCAL.1, POST.1, BUILTIN.1);
        let maxs = expected(CONNECT.2, API.2, LOCAL.2, POST.2, BUILTIN.2);
        assert_eq!(Timeouts::from_settings(&secs(0, 0, 0, 0, 0)), mins, "0");
        assert_eq!(
            Timeouts::from_settings(&secs(
                CONNECT.1 - 1,
                API.1 - 1,
                LOCAL.1 - 1,
                POST.1 - 1,
                BUILTIN.1 - 1
            )),
            mins,
            "min - 1"
        );
        assert_eq!(
            Timeouts::from_settings(&secs(CONNECT.1, API.1, LOCAL.1, POST.1, BUILTIN.1)),
            mins,
            "min"
        );
        assert_eq!(
            Timeouts::from_settings(&secs(CONNECT.2, API.2, LOCAL.2, POST.2, BUILTIN.2)),
            maxs,
            "max"
        );
        assert_eq!(
            Timeouts::from_settings(&secs(
                CONNECT.2 + 1,
                API.2 + 1,
                LOCAL.2 + 1,
                POST.2 + 1,
                BUILTIN.2 + 1
            )),
            maxs,
            "max + 1"
        );
        let m = u32::MAX;
        assert_eq!(
            Timeouts::from_settings(&secs(m, m, m, m, m)),
            maxs,
            "u32::MAX"
        );
        // One field out of range clamps that field only.
        assert_eq!(
            Timeouts::from_settings(&secs(0, 45, m, 20, 150)),
            expected(CONNECT.1, 45, LOCAL.2, 20, 150)
        );
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
