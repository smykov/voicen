//! The one source of network and engine durations (FR-24; spec 001 T006;
//! decisions #97, #99).
//!
//! The dictation durations are settings ([`TimeoutSettings`], whole seconds). The
//! bounds table [`TimeoutRole::bounds`] (default, min, max per role) is the only source of their
//! defaults (`settings::defaults` reads it), of the save-time range rule
//! (`settings::validate`) and of the clamp in [`Timeouts::from_settings`], which the
//! pipeline calls once per job on that job's settings snapshot.
//! `Timeouts::default()` is that conversion of the defaults. The model download's
//! no-data limit is not a setting (decision #99 Q1): it stays a fixed 30 s.
//!
//! Tests override single fields with milliseconds
//! (`Timeouts { api_transcription: Duration::from_millis(300), ..Timeouts::default() }`)
//! through `Pipeline::with_timeouts`. The engine stores no `Duration` of its own: it
//! reads them from the `TranscribeRequest` of each call.

use std::time::Duration;

use crate::settings::TimeoutSettings;

/// Model download: the longest wait for the next bytes (spec 002 Clarification 5,
/// decision #49); fixed, not a setting (decision #99 Q1).
const DOWNLOAD_NO_DATA: Duration = Duration::from_secs(30);

/// A user-configurable duration (one field of [`TimeoutSettings`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeoutRole {
    Connect,
    ApiTranscription,
    LocalServer,
    PostProcessing,
    BuiltinLocal,
}

/// The allowed range and default of one role, in whole seconds; `min..=max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutBounds {
    pub default: u32,
    pub min: u32,
    pub max: u32,
}

impl TimeoutBounds {
    /// Whether `secs` is allowed (both ends inclusive).
    pub fn contains(self, secs: u32) -> bool {
        (self.min..=self.max).contains(&secs)
    }

    /// `secs` moved into `min..=max`.
    pub fn clamp(self, secs: u32) -> u32 {
        secs.clamp(self.min, self.max)
    }
}

impl TimeoutRole {
    /// Every role, in settings order.
    pub const ALL: [TimeoutRole; 5] = [
        TimeoutRole::Connect,
        TimeoutRole::ApiTranscription,
        TimeoutRole::LocalServer,
        TimeoutRole::PostProcessing,
        TimeoutRole::BuiltinLocal,
    ];

    /// The bounds table of decision #99: (default, min, max) seconds per role.
    pub const fn bounds(self) -> TimeoutBounds {
        let (default, min, max) = match self {
            TimeoutRole::Connect => (5, 1, 60),
            TimeoutRole::ApiTranscription => (30, 5, 600),
            TimeoutRole::LocalServer => (60, 5, 1800),
            TimeoutRole::PostProcessing => (15, 5, 300),
            TimeoutRole::BuiltinLocal => (120, 10, 1800),
        };
        TimeoutBounds { default, min, max }
    }

    /// This role's value in `s`.
    pub fn get(self, s: &TimeoutSettings) -> u32 {
        match self {
            TimeoutRole::Connect => s.connect_s,
            TimeoutRole::ApiTranscription => s.api_transcription_s,
            TimeoutRole::LocalServer => s.local_server_s,
            TimeoutRole::PostProcessing => s.post_processing_s,
            TimeoutRole::BuiltinLocal => s.builtin_local_s,
        }
    }
}

/// The defaults of [`TimeoutRole::bounds`] as settings (read by `settings::defaults`).
pub fn default_settings() -> TimeoutSettings {
    let d = |role: TimeoutRole| role.bounds().default;
    TimeoutSettings {
        connect_s: d(TimeoutRole::Connect),
        api_transcription_s: d(TimeoutRole::ApiTranscription),
        local_server_s: d(TimeoutRole::LocalServer),
        post_processing_s: d(TimeoutRole::PostProcessing),
        builtin_local_s: d(TimeoutRole::BuiltinLocal),
    }
}

/// Durations per role (data-model "Timeouts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// TCP/TLS connect, every HTTP role (FR-24, default 5 s).
    pub connect: Duration,
    /// The whole API transcription request, connect to last body byte (default 30 s).
    pub api_transcription: Duration,
    /// The whole local-server transcription request (default 60 s).
    pub local_server: Duration,
    /// The whole post-processing request (default 15 s).
    pub post_processing: Duration,
    /// Built-in whisper.cpp transcription (default 120 s).
    pub builtin: Duration,
    /// Model download: the longest wait for the next body bytes (or the response
    /// headers), a per-read timeout, never a total one (spec 002 Clarification 5:
    /// 30 s; decision #49). Fixed, not taken from the settings.
    pub download_no_data: Duration,
}

impl Timeouts {
    /// The durations of `s`, each clamped into its [`TimeoutRole::bounds`]: a hand-edited
    /// file is not validated on load, and `0` or `u32::MAX` must not reach a request.
    pub fn from_settings(s: &TimeoutSettings) -> Timeouts {
        let secs =
            |role: TimeoutRole| Duration::from_secs(u64::from(role.bounds().clamp(role.get(s))));
        Timeouts {
            connect: secs(TimeoutRole::Connect),
            api_transcription: secs(TimeoutRole::ApiTranscription),
            local_server: secs(TimeoutRole::LocalServer),
            post_processing: secs(TimeoutRole::PostProcessing),
            builtin: secs(TimeoutRole::BuiltinLocal),
            download_no_data: DOWNLOAD_NO_DATA,
        }
    }
}

impl Default for Timeouts {
    /// [`Timeouts::from_settings`] of the default settings.
    fn default() -> Timeouts {
        Timeouts::from_settings(&default_settings())
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
