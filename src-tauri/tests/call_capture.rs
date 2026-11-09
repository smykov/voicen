//! T-127: the shell's call-track capture (`voicen_lib::win::track_capture`, spec 007 R1, R2,
//! FR-002, FR-010, FR-014; T026/T027) and the free-disk-space probe the call side reuses
//! (`local_models::WinDiskSpace`, T-125 Notes 2026-11-05: no `win/disk.rs`). Windows CI only
//! (decision #5): cpal and the Win32 calls exist only there.
//!
//! API pinned for the developer (docs/tasks/T-127.md `## Tests`; the module does not exist yet,
//! so this file fails to compile until it does; that unresolved import is the red):
//!
//! ```text
//! voicen_lib::win::track_capture::
//!   CpalTrackSource                       // Send + Sync; implements core's `TrackSource`
//!     ::new() -> CpalTrackSource
//!     open(track, device, sink) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError>
//!     now() -> CaptureStamp               // QueryPerformanceCounter x 10^7 / frequency, x 100:
//!                                         // non-decreasing, on the 100 ns grid, the clock of
//!                                         // cpal's `Stream::now()` and of the callback stamps
//!   track_loss(cpal::ErrorKind) -> Option<LossCause>
//!                                         // StreamInvalidated -> Invalidated,
//!                                         // DeviceNotAvailable -> NotAvailable,
//!                                         // Xrun / RealtimeDenied / DeviceChanged -> None,
//!                                         // every other kind -> Other
//!   loss_callback(track: Track, sink: Arc<dyn TrackSink>) -> impl FnMut(cpal::Error) + Send + 'static
//!                                         // the stream's error callback: reports
//!                                         // `sink.lost(track, cause, <now()>)` at most once per
//!                                         // stream; a kind with no loss neither reports nor uses
//!                                         // up the one report
//!   stamp_of(cpal::StreamInstant) -> CaptureStamp
//!                                         // exactly `capture.as_nanos()`, saturating at u64::MAX
//!   open_error(&CaptureError) -> OpenError
//!                                         // NoDevice -> NoDevice, AccessDenied -> AccessDenied,
//!                                         // DeviceBusy -> Busy, Other(_) -> Other
//! ```
//!
//! What the runner has is not known (docs/decisions/windows-ci-runner.md: its audio endpoints
//! were never probed). The failure-branch tests therefore branch on what cpal says the host has
//! (`default_output_device()` / `default_input_device()`): no endpoint -> `OpenError::NoDevice`
//! and the sink sees nothing; an endpoint -> `Ok`, stamps on the `now()` timeline, and the fact
//! printed (read it with `-- --nocapture`; it goes into research.md R1/R2 and
//! windows-ci-runner.md). Real loopback, a default-output switch and drift are the owner's manual
//! check (OQ-62 (b), decisions #178).
//!
//! No verdict is a ceiling on a measured time (T-201): the 3 s `open_bounded` budget holds by
//! construction (a timed-out open is `Other`, never `NoDevice`), and every call that could hang
//! runs under `timing_support::within_hang`.
#![cfg(windows)]

mod timing_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cpal::traits::HostTrait;
use cpal::{Error, ErrorKind, StreamInstant};
use timing_support::{eventually, within_hang, HANG};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::{
    CallDeps, CallIndicator, CallSession, CallSettingsPort, CallShell, SubtitleConsumer, Toggled,
};
use voicen_core::call::wav_source::WavTrackSource;
use voicen_core::call::{
    CallNotice, CallSource, CallStore, CaptureStamp, CommunicationsCheck, DeviceEvent,
    DeviceEvents, DeviceEventsFactory, EndpointId, LossCause, OpenError, StopReason, Track,
    TrackDevice, TrackSink, TrackSource,
};
use voicen_core::clock::{FakeClock, LocalOffset, SystemMonotonic};
use voicen_core::diag::{Log, LogConfig};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::platform::{CaptureHandle, DeviceId};
use voicen_core::recording::CaptureError;
use voicen_core::settings::roots::FsCallRoots;
use voicen_core::settings::service::WarnedHostFailed;
use voicen_core::settings::url::ProviderHost;
use voicen_core::settings::{defaults, Settings};
use voicen_core::test_support::TempDir;
use voicen_lib::local_models::WinDiskSpace;
use voicen_lib::win::track_capture::{
    loss_callback, open_error, stamp_of, track_loss, CpalTrackSource,
};

/// OS-like text planted as cpal's message: it must never change a verdict.
const CANARY: &str = "PLANTED-OS-TEXT 0x80070005 Zugriff verweigert";

/// Every cpal 0.18.2 `ErrorKind`, by a match that lists each one, so a kind added by a cpal
/// update shows up here (the same list as tests/capture.rs `all_kinds`).
fn all_kinds() -> Vec<ErrorKind> {
    let all = vec![
        ErrorKind::DeviceBusy,
        ErrorKind::DeviceChanged,
        ErrorKind::DeviceNotAvailable,
        ErrorKind::HostUnavailable,
        ErrorKind::InvalidInput,
        ErrorKind::PermissionDenied,
        ErrorKind::RealtimeDenied,
        ErrorKind::ResourceExhausted,
        ErrorKind::StreamInvalidated,
        ErrorKind::UnsupportedConfig,
        ErrorKind::UnsupportedOperation,
        ErrorKind::Xrun,
        ErrorKind::BackendError,
        ErrorKind::Other,
    ];
    for kind in &all {
        match kind {
            ErrorKind::DeviceBusy
            | ErrorKind::DeviceChanged
            | ErrorKind::DeviceNotAvailable
            | ErrorKind::HostUnavailable
            | ErrorKind::InvalidInput
            | ErrorKind::PermissionDenied
            | ErrorKind::RealtimeDenied
            | ErrorKind::ResourceExhausted
            | ErrorKind::StreamInvalidated
            | ErrorKind::UnsupportedConfig
            | ErrorKind::UnsupportedOperation
            | ErrorKind::Xrun
            | ErrorKind::BackendError
            | ErrorKind::Other => {}
            _ => panic!("a cpal ErrorKind this test does not list: {kind:?}"),
        }
    }
    all
}

/// The loss each kind must give, written out here and not taken from the code under test: a
/// default-device change (`StreamInvalidated`, `DeviceNotAvailable`) keeps the old stream
/// running but the track no longer follows the device; the three kinds after which cpal 0.18.2
/// keeps delivering the device's audio (`Xrun`, `RealtimeDenied`, `DeviceChanged`) are no loss;
/// every other kind ended the stream thread.
fn expected_loss(kind: ErrorKind) -> Option<LossCause> {
    match kind {
        ErrorKind::StreamInvalidated => Some(LossCause::Invalidated),
        ErrorKind::DeviceNotAvailable => Some(LossCause::NotAvailable),
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => None,
        _ => Some(LossCause::Other),
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A sink that records what reached it.
#[derive(Default)]
struct RecordingSink {
    blocks: Mutex<Vec<(Track, CaptureStamp, u32, u16)>>,
    losses: Mutex<Vec<(Track, LossCause, CaptureStamp)>>,
}

impl RecordingSink {
    fn blocks(&self) -> Vec<(Track, CaptureStamp, u32, u16)> {
        lock(&self.blocks).clone()
    }
    fn losses(&self) -> Vec<(Track, LossCause, CaptureStamp)> {
        lock(&self.losses).clone()
    }
}

impl TrackSink for RecordingSink {
    fn block(&self, track: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp) {
        assert!(rate > 0 && channels > 0, "a block without rate or channels");
        assert!(
            !interleaved.is_empty() && interleaved.len().is_multiple_of(usize::from(channels)),
            "a ragged or empty block"
        );
        lock(&self.blocks).push((track, at, rate, channels));
    }
    fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp) {
        lock(&self.losses).push((track, cause, at));
    }
}

type Opened = Result<(Box<dyn CaptureHandle>, EndpointId), OpenError>;

/// `CpalTrackSource::open` under the hang bound (the open budget is 3 s inside; a test that
/// waits for it is a hang the bound reports, not a slow pass).
fn open_hung(track: Track, device: TrackDevice, sink: Arc<RecordingSink>) -> Opened {
    let Some((opened, _)) = within_hang(move || CpalTrackSource::new().open(track, device, sink))
    else {
        panic!("{track:?}: open did not return within {HANG:?}")
    };
    opened
}

/// Whether cpal's WASAPI host has a default render (`output`) or capture endpoint.
fn host_has_default(output: bool) -> bool {
    let Some((has, _)) = within_hang(move || {
        let host = cpal::default_host();
        if output {
            host.default_output_device().is_some()
        } else {
            host.default_input_device().is_some()
        }
    }) else {
        panic!("the default-device query did not return within {HANG:?}")
    };
    has
}

/// The device-present branch: an `Ok` open, then a short capture whose stamps are on the
/// `now()` timeline, then a clean stop.
fn assert_opens_on_the_now_timeline(track: Track, device: TrackDevice, label: &str) {
    let source = CpalTrackSource::new();
    let sink = Arc::new(RecordingSink::default());
    let before = source.now();
    let (handle, endpoint) = match open_hung(track, device, sink.clone()) {
        Ok(opened) => opened,
        Err(err) => panic!("{label}: the host has a default endpoint but open gave {err:?}"),
    };
    // Loopback delivers nothing while nothing plays (R1): wait a moment, assert what came.
    thread::sleep(Duration::from_millis(300));
    let after = source.now();
    let blocks = sink.blocks();
    assert!(handle.stop().is_ok(), "{label}: stop failed");
    let stamps: Vec<u64> = blocks.iter().map(|b| b.1.ns()).collect();
    assert!(
        stamps.windows(2).all(|w| w[0] <= w[1]),
        "{label}: stamps not monotonic: {stamps:?}"
    );
    assert!(
        stamps.iter().all(|s| *s >= before.ns() && *s <= after.ns()),
        "{label}: stamps outside [{}, {}] of now(): {stamps:?}",
        before.ns(),
        after.ns()
    );
    assert!(
        blocks.iter().all(|b| b.0 == track),
        "{label}: blocks tagged with another track"
    );
    assert!(
        sink.losses().is_empty(),
        "{label}: a loss with no error: {:?}",
        sink.losses()
    );
    println!(
        "T-127 fact: {label} opened (endpoint {endpoint:?}); {} blocks in 300 ms, first {:?} last {:?}, \
         rate/channels {:?}, now() window [{}, {}] ns",
        blocks.len(),
        stamps.first(),
        stamps.last(),
        blocks.first().map(|b| (b.2, b.3)),
        before.ns(),
        after.ns()
    );
}

/// The no-endpoint branch: the open is `NoDevice` and nothing reaches the sink.
fn assert_no_device(track: Track, device: TrackDevice, label: &str) {
    let sink = Arc::new(RecordingSink::default());
    let got = open_hung(track, device, sink.clone());
    match got {
        Err(OpenError::NoDevice) => {}
        Err(other) => panic!("{label}: expected NoDevice, got {other:?}"),
        Ok(_) => panic!("{label}: a capture opened with no endpoint"),
    }
    thread::sleep(Duration::from_millis(100));
    assert!(sink.blocks().is_empty(), "{label}: blocks reached the sink");
    assert!(sink.losses().is_empty(), "{label}: a loss reached the sink");
    println!("T-127 fact: {label}: the host has no such endpoint, open = NoDevice");
}

// ---- 1. the failure branch (Acceptance 1, 5) ---------------------------------------------

#[test]
fn opening_the_system_loopback_without_a_render_endpoint_is_no_device_and_with_one_is_on_the_now_timeline(
) {
    // Acceptance 1 (T026) and the first acceptance (R1, R2): with no default render endpoint
    // (the expected runner) `open(System, DefaultOutputLoopback)` is `Err(NoDevice)` and the
    // sink sees nothing; with one it opens, and its stamps are monotonic, inside the
    // `now()` window and on the call's timeline. Bite: loopback opened on
    // `default_input_device()` (a runner with a mic but no speakers opens Ok, a runner with
    // speakers but no mic gives NoDevice), a missing device mapped to Other or to Ok with a
    // silent handle, a stamp taken from `Instant` at callback entry instead of
    // `capture.as_nanos()` (outside the `now()` window), blocks tagged with the wrong track.
    if host_has_default(true) {
        assert_opens_on_the_now_timeline(
            Track::System,
            TrackDevice::DefaultOutputLoopback,
            "System/DefaultOutputLoopback",
        );
    } else {
        assert_no_device(
            Track::System,
            TrackDevice::DefaultOutputLoopback,
            "System/DefaultOutputLoopback",
        );
    }
}

#[test]
fn opening_the_default_input_without_a_capture_endpoint_is_no_device_and_with_one_is_on_the_now_timeline(
) {
    // Acceptance 5 (T027: the mic opens the default input): the same two branches for
    // `open(Mic, DefaultInput)`. Bite: the mic opened on `default_output_device()` (the
    // branches swap on a runner with only one kind of endpoint), NoDevice mapped to Other,
    // a fallback to loopback.
    if host_has_default(false) {
        assert_opens_on_the_now_timeline(Track::Mic, TrackDevice::DefaultInput, "Mic/DefaultInput");
    } else {
        assert_no_device(Track::Mic, TrackDevice::DefaultInput, "Mic/DefaultInput");
    }
}

#[test]
fn a_selected_endpoint_no_machine_has_is_no_device_and_reports_nothing() {
    // Acceptance 5 (T027: "opens the selected mic"): a well-formed endpoint id of no device
    // and a malformed one both give `NoDevice` (never Ok, never Other), and the sink gets
    // nothing. Bite: falling back to the default input when the id is missing (Ok or blocks on
    // a runner with a mic), an unparsable id mapped to Other, `Selected` ignored.
    for raw in [
        "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000000}",
        "not-an-endpoint-id",
    ] {
        assert_no_device(
            Track::Mic,
            TrackDevice::Selected(DeviceId(raw.to_string())),
            raw,
        );
    }
}

#[test]
fn a_capture_error_maps_one_to_one_onto_an_open_error_whatever_its_text() {
    // Acceptance 1 (the refusal reason core shows): NoDevice -> NoDevice, AccessDenied ->
    // AccessDenied, DeviceBusy -> Busy, Other(_) -> Other, with and without OS-like text
    // inside (an `OpenError` carries none). Bite: DeviceBusy or AccessDenied falling to Other,
    // NoDevice mapped to Other, a map by message text.
    let cases = [
        (CaptureError::NoDevice, OpenError::NoDevice),
        (CaptureError::AccessDenied, OpenError::AccessDenied),
        (CaptureError::DeviceBusy, OpenError::Busy),
        (CaptureError::Other(String::new()), OpenError::Other),
        (CaptureError::Other(CANARY.to_owned()), OpenError::Other),
        (
            CaptureError::Other("the microphone did not open in time".to_owned()),
            OpenError::Other,
        ),
    ];
    for (err, want) in cases {
        assert_eq!(open_error(&err), want, "{err:?}");
    }
}

// ---- 2. the loss map (Acceptance 5) --------------------------------------------------------

#[test]
fn every_cpal_error_kind_maps_to_the_documented_track_loss() {
    // Acceptance 5 (`StreamInvalidated` / `DeviceNotAvailable` count as track losses; T-104's
    // lesson: an Xrun is not one). Over every cpal 0.18.2 kind, with and without a planted
    // message (it never changes the verdict). Bite: `Xrun` (or `DeviceChanged`,
    // `RealtimeDenied`) mapped to a loss, `StreamInvalidated` / `DeviceNotAvailable` mapped to
    // Other or swapped, the loss of the other kinds dropped (None), a map by message.
    let mut wrong = Vec::new();
    for kind in all_kinds() {
        let want = expected_loss(kind);
        let got = track_loss(kind);
        if got != want {
            wrong.push(format!("{kind:?}: {got:?}, expected {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn the_error_callback_reports_one_loss_per_stream_stamped_when_it_ran() {
    // Acceptance 5, invariant: for each kind that is a loss, on both tracks, the first error
    // reports `sink.lost(track, cause, at)` with `at` read from the `now()` clock inside the
    // callback (between two reads around the call); later errors of any kind report nothing;
    // no block is invented. Bite: a report per call (no once flag: 2+ losses), a stamp taken
    // at build time (outside the window), the track swapped, the cause taken from the last
    // error instead of the first.
    let source = CpalTrackSource::new();
    let mut wrong = Vec::new();
    for track in [Track::Mic, Track::System] {
        for kind in all_kinds() {
            let Some(cause) = expected_loss(kind) else {
                continue;
            };
            let sink = Arc::new(RecordingSink::default());
            let mut callback = loss_callback(track, sink.clone());
            let before = source.now();
            callback(Error::with_message(kind, CANARY));
            let after = source.now();
            callback(Error::new(ErrorKind::DeviceNotAvailable));
            callback(Error::new(ErrorKind::StreamInvalidated));
            callback(Error::new(ErrorKind::BackendError));
            match sink.losses().as_slice() {
                [(t, c, at)] if *t == track && *c == cause && *at >= before && *at <= after => {}
                other => wrong.push(format!(
                    "{track:?} {kind:?}: losses {other:?}, expected one ({track:?}, {cause:?}) \
                     between {before:?} and {after:?}"
                )),
            }
            if !sink.blocks().is_empty() {
                wrong.push(format!(
                    "{track:?} {kind:?}: blocks from the error callback"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn an_error_that_keeps_the_track_delivering_is_no_loss_and_does_not_use_up_the_report() {
    // Acceptance 5 (T-104 / F-016 carried over): `Xrun`, `RealtimeDenied` and `DeviceChanged`
    // report nothing, also repeated; they do not use up the stream's one report, so a later
    // `DeviceNotAvailable` still reports exactly one `NotAvailable`, and later errors nothing.
    // Bite: the dictation-style "any kind" callback (the Xrun reports a loss), a filter that
    // lets one of the three through, a once flag set by an ignored kind (the unplug is lost).
    let source = CpalTrackSource::new();
    let mut wrong = Vec::new();
    for kind in all_kinds()
        .into_iter()
        .filter(|k| expected_loss(*k).is_none())
    {
        let sink = Arc::new(RecordingSink::default());
        let mut callback = loss_callback(Track::System, sink.clone());
        callback(Error::with_message(kind, CANARY));
        callback(Error::new(kind));
        if !sink.losses().is_empty() {
            wrong.push(format!("{kind:?}: reported as a loss"));
            continue;
        }
        let before = source.now();
        callback(Error::new(ErrorKind::DeviceNotAvailable));
        let after = source.now();
        callback(Error::new(kind));
        callback(Error::new(ErrorKind::StreamInvalidated));
        match sink.losses().as_slice() {
            [(Track::System, LossCause::NotAvailable, at)] if *at >= before && *at <= after => {}
            other => wrong.push(format!(
                "{kind:?} then DeviceNotAvailable: losses {other:?}, expected one NotAvailable \
                 between {before:?} and {after:?}"
            )),
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// ---- 3. the stamp conversion (first acceptance, R2) ---------------------------------------

#[test]
fn a_cpal_capture_instant_converts_to_its_exact_nanoseconds_and_keeps_its_order() {
    // First acceptance (T026): the callback's stamp is cpal's QPC capture instant in exact
    // nanoseconds, never a rounded, scaled or re-based value, and later instants give later
    // stamps. Bite: milliseconds or microseconds (a coarser grid), `secs` dropped or `nanos`
    // dropped, an offset against the first block, an `Instant` read at callback entry, an
    // unsaturated `as u64` truncation of a huge instant.
    let cases = [
        (0u64, 0u32),
        (0, 100),
        (0, 999_999_900),
        (1, 0),
        (1, 500),
        (7, 123_456_700),
        (86_400, 0),
        (3_600_000, 999_999_900),
    ];
    let mut previous: Option<CaptureStamp> = None;
    for (secs, nanos) in cases {
        let stamp = stamp_of(StreamInstant::new(secs, nanos));
        assert_eq!(stamp.ns(), secs * 1_000_000_000 + u64::from(nanos));
        if let Some(prev) = previous {
            assert!(prev < stamp, "order lost: {prev:?} then {stamp:?}");
        }
        previous = Some(stamp);
    }
    assert_eq!(stamp_of(StreamInstant::from_nanos(42)).ns(), 42);
    assert_eq!(
        stamp_of(StreamInstant::from_nanos(u64::MAX)).ns(),
        u64::MAX,
        "the largest representable instant"
    );
    assert_eq!(
        stamp_of(StreamInstant::new(u64::MAX, 999_999_999)).ns(),
        u64::MAX,
        "an instant past u64 nanoseconds saturates, it does not wrap to a small stamp"
    );
}

// ---- 4. now() (first acceptance, R2) -------------------------------------------------------

#[test]
fn now_never_decreases_moves_on_and_lies_on_the_100_ns_grid() {
    // First acceptance (T026): `CallSession::open_tracks` reads `now()` before any stream
    // exists, so it reads QPC itself with cpal's formula: successive reads never decrease,
    // every read is a multiple of 100 ns (the grid of `Stream::now()` and of the callback
    // stamps), and the clock moves on. A second source reads the same clock (one timeline, not
    // a per-source origin). The type is `Send + Sync` for `CallDeps.source`. Bite: `now()`
    // not rounded to 100 ns (`ns % 100 != 0`), a constant or a per-instance origin (the clock
    // never moves / two sources disagree), a counter read without the frequency division
    // (non-monotonic once scaled wrongly or off the grid), a wall clock that steps back.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CpalTrackSource>();

    let source = CpalTrackSource::new();
    let other = CpalTrackSource::new();
    let first = source.now();
    let mut last = first;
    for i in 0..10_000 {
        let at = source.now();
        assert!(at >= last, "read {i}: {at:?} after {last:?}");
        assert_eq!(at.ns() % 100, 0, "read {i}: {at:?} is off the 100 ns grid");
        last = at;
    }
    assert!(
        eventually(|| source.now() > first),
        "now() did not move within {HANG:?}"
    );
    let a = source.now();
    let b = other.now();
    let c = source.now();
    assert!(
        a <= b && b <= c,
        "two sources on different timelines: {a:?} {b:?} {c:?}"
    );
}

// ---- 5. free disk space (Acceptance 3) -----------------------------------------------------

#[test]
fn the_free_disk_space_of_the_temp_dir_is_a_positive_number() {
    // Acceptance 3 (T026, T027): `GetDiskFreeSpaceExW` through the one existing
    // `DiskSpace` implementation returns a value for `%TEMP%`; a real volume has bytes left,
    // so the value is more than 0, and a not yet existing child of it (the next call folder)
    // answers like its nearest existing ancestor. Bite: the probe returning `Ok(0)` or an
    // error for an existing dir, the call side wired to a stub, an error for a missing child.
    let temp = std::env::var_os("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    assert!(temp.is_dir(), "%TEMP% is not a directory: {temp:?}");
    let available = WinDiskSpace
        .available_bytes(&temp)
        .unwrap_or_else(|e| panic!("available_bytes({temp:?}) failed: {:?}", e.kind()));
    assert!(available > 0, "no free bytes reported for {temp:?}");
    let child = temp.join("voicen-t127-not-yet-created").join("call");
    let for_child = WinDiskSpace
        .available_bytes(&child)
        .unwrap_or_else(|e| panic!("available_bytes({child:?}) failed: {:?}", e.kind()));
    assert!(for_child > 0, "no free bytes reported for {child:?}");
    println!("T-127 fact: {available} bytes free on the volume of {temp:?}");
}

// ---- 6. a WAV source through the session (Acceptance 2) ------------------------------------

/// Two WAV sources behind core's one `TrackSource` port: the session opens each track through
/// `CallDeps.source`, and `WavTrackSource` keeps one sink.
struct DualWav {
    mic: WavTrackSource,
    system: WavTrackSource,
}

impl DualWav {
    fn pump(&self, until: CaptureStamp) {
        self.mic.pump(until);
        self.system.pump(until);
    }
}

impl TrackSource for DualWav {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        match track {
            Track::Mic => self.mic.open(track, device, sink),
            Track::System => self.system.open(track, device, sink),
        }
    }

    fn now(&self) -> CaptureStamp {
        self.system.now()
    }
}

/// A 2 s 16-bit mono 16 kHz WAV of a 440 Hz tone (silence would encode to almost nothing).
fn tone_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..32_000u32)
        .map(|i| {
            let phase = f64::from(i) * 440.0 * std::f64::consts::TAU / 16_000.0;
            (phase.sin() * 12_000.0) as i16
        })
        .collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&16_000u32.to_le_bytes());
    v.extend_from_slice(&32_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// The session's three ports, minimal: the indicator keeps the recording flag, the shell and
/// the subtitle consumer ignore everything, the settings port serves one fixed snapshot.
#[derive(Default)]
struct Indicator {
    recording: Mutex<Vec<bool>>,
}

impl CallIndicator for Indicator {
    fn set_state(&self, _state: &IndicatorState) {}
    fn set_tick(&self, _tick: &IndicatorTick) {}
    fn set_call_recording(&self, recording: bool) {
        lock(&self.recording).push(recording);
    }
}

struct Shell;

impl CallShell for Shell {
    fn notice(&self, _notice: CallNotice) {}
    fn open_calls_tab(&self) {}
}

struct Port(Arc<Settings>);

impl CallSettingsPort for Port {
    fn snapshot(&self) -> Arc<Settings> {
        self.0.clone()
    }
    fn remember_warned_host(&self, _host: &ProviderHost) -> Result<(), WarnedHostFailed> {
        Ok(())
    }
}

struct Subtitles;

impl SubtitleConsumer for Subtitles {
    fn audio(&self, _track: Track, _samples: &[f32]) {}
}

/// No device changes: the WAV-backed captures never change endpoint (T-149 ports).
struct NoDeviceEvents;

impl DeviceEvents for NoDeviceEvents {
    fn next(&self) -> Option<DeviceEvent> {
        None
    }
}

impl DeviceEventsFactory for NoDeviceEvents {
    fn open(&self) -> Box<dyn DeviceEvents> {
        Box::new(NoDeviceEvents)
    }
}

/// The communications device is the default one.
struct SameComms;

impl CommunicationsCheck for SameComms {
    fn differs(&self) -> Result<bool, i32> {
        Ok(false)
    }
}

struct Utc;

impl LocalOffset for Utc {
    fn seconds_east(&self, _t: SystemTime) -> i32 {
        0
    }
}

/// The call folders under `root`.
fn call_folders(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// What an Ogg Opus file must show: Ogg pages, the Opus head and tags, and audio pages.
fn assert_ogg_opus_with_audio(path: &Path) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{path:?}: {:?}", e.kind()));
    assert!(bytes.starts_with(b"OggS"), "{path:?} is not an Ogg file");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"OpusHead"), "{path:?} has no OpusHead");
    assert!(has(b"OpusTags"), "{path:?} has no OpusTags");
    let pages = bytes.windows(4).filter(|w| *w == b"OggS").count();
    assert!(pages >= 3, "{path:?}: {pages} Ogg pages, no audio page");
    assert!(
        bytes.len() > 1_500,
        "{path:?}: {} bytes for about 2 s of tone",
        bytes.len()
    );
}

#[test]
fn a_wav_source_through_the_session_writes_both_track_files_as_ogg_opus() {
    // Acceptance 2 (T026, T027): core's `WavTrackSource` as `CallDeps.source`, with the
    // session's own sink and writer (the path the shell's `CpalTrackSource` feeds):
    // record on, the due blocks of both tracks pumped, record off; the call folder holds
    // `mic.opus` and `system.opus`, both Ogg Opus with an audio page (the codec linked in the
    // shell exe, T-117), and the indicator saw record on then off. Bite: a session that never
    // reads the source's sink (empty or header-only files, fewer than 3 pages / under 1.5 kB),
    // one track routed to the other's file (one file header-only), the codec not linked in the
    // shell (the writer fails: not `Started` or no files), the stop not flushing the writer
    // (the files end before the audio pages).
    let tmp = TempDir::new();
    let root = tmp.path().join("calls");
    let logs = tmp.path().join("logs");
    fs::create_dir_all(&root).expect("the calls root");
    fs::create_dir_all(&logs).expect("the logs dir");

    let mut settings = defaults(Some("en-US"));
    settings.api.base_url = "https://api.example.com/v1".to_string();
    settings.calls.storage_root = Some(root.clone());
    settings.calls.reminder = false;
    settings.calls.warned_hosts = Vec::new();
    settings.microphone = None;

    let start = CaptureStamp::from_ms(5_000);
    let wav = tone_wav();
    let dual = Arc::new(DualWav {
        mic: WavTrackSource::from_bytes(&wav)
            .expect("the tone wav")
            .with_block_ms(20)
            .with_start(start),
        system: WavTrackSource::from_bytes(&wav)
            .expect("the tone wav")
            .with_block_ms(20)
            .with_start(start),
    });

    let mono = Arc::new(SystemMonotonic);
    let roots = Arc::new(FsCallRoots::new(tmp.path().join("default-calls")));
    let store = Arc::new(CallStore::new(roots, mono.clone()));
    let clock = Arc::new(FakeClock::at(
        UNIX_EPOCH + Duration::from_secs(1_791_115_200),
    ));
    let log = Arc::new(Log::open(
        logs,
        clock.clone(),
        Arc::new(Utc),
        LogConfig::default(),
        Box::new(|_| {}),
    ));
    let indicator = Arc::new(Indicator::default());
    let session = CallSession::new(CallDeps {
        source: dual.clone(),
        store,
        settings: Arc::new(Port(Arc::new(settings))),
        indicator: indicator.clone(),
        shell: Arc::new(Shell),
        log,
        clock,
        offset: Arc::new(Utc),
        mono,
        app_version: "0.0.0-test".to_string(),
        open_track: Arc::new(|path| fs::File::create(path)),
        channel_capacity: 256,
        subtitles: Arc::new(Subtitles),
        devices: Arc::new(NoDeviceEvents),
        comms: Arc::new(SameComms),
    });

    assert_eq!(
        session.toggle_record(true, CallSource::Tray),
        Toggled::Started
    );
    dual.pump(CaptureStamp::from_ms(5_000 + 2_000));
    assert!(session.stop(StopReason::User), "no call was recording");

    let folders = call_folders(&root);
    assert_eq!(folders.len(), 1, "exactly one call folder: {folders:?}");
    assert_ogg_opus_with_audio(&folders[0].join("mic.opus"));
    assert_ogg_opus_with_audio(&folders[0].join("system.opus"));
    assert_eq!(
        *lock(&indicator.recording),
        vec![true, false],
        "the indicator's recording flag"
    );
}
