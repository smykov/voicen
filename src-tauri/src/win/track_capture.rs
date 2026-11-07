//! The call tracks (spec 007 R1, R2; T-127): `CpalTrackSource`, core's `TrackSource` over
//! cpal 0.18 (WASAPI). `open` resolves the track's device on an opener thread bounded by
//! [`OPEN_BUDGET`] ([`open_bounded`], shared with dictation): `Selected` -> the input endpoint
//! with that id (`device_by_id`, an input device only), else `NoDevice`; `DefaultInput` -> the
//! default capture endpoint; `DefaultOutputLoopback` -> the default render endpoint, opened as an
//! input stream with its `default_output_config()` (cpal adds `AUDCLNT_STREAMFLAGS_LOOPBACK` for
//! an input stream on a render device, and follows default-device changes for a default handle).
//! A stream that opens after the budget is dropped on the opener thread.
//!
//! One timeline (invariant): every block is stamped with cpal's capture instant
//! ([`stamp_of`]: the `GetBuffer` QPC position, exact nanoseconds), and [`CpalTrackSource::now`]
//! reads the same QPC clock with cpal's own `Stream::now()` formula (100 ns units x 100), so the
//! call start and every stamp share one clock and one 100 ns grid. `now()` does not need a
//! stream: the session reads it before any track is open.
//!
//! Track loss ([`track_loss`], [`loss_callback`]): `StreamInvalidated` -> `Invalidated` and
//! `DeviceNotAvailable` -> `NotAvailable` (a default-device change: the old stream keeps running
//! but no longer follows the device); `Xrun`, `RealtimeDenied` and `DeviceChanged` leave the track
//! delivering and are no loss (F-016 / T-104); every other kind ended the stream thread ->
//! `Other`. Each stream reports at most one loss.
//!
//! Lock contract: the data callback calls only `TrackSink::block`, the error callback only
//! `TrackSink::lost`; neither calls the session, so dropping a handle (it joins cpal's stream
//! thread) never waits for a thread that waits for the session. No cpal or OS text reaches an
//! `OpenError`; endpoint ids pass only through `EndpointId::new`. Nothing here logs.

use std::sync::{Arc, OnceLock};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Error, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig, StreamInstant,
};
use voicen_core::call::{
    CaptureStamp, EndpointId, LossCause, OpenError, Track, TrackDevice, TrackSink, TrackSource,
};
use voicen_core::platform::CaptureHandle;
use voicen_core::recording::CaptureError;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use super::capture::{capture_error, open_bounded, OPEN_BUDGET};

/// The fixed text of an unsupported sample format (never OS text; it maps to `Other`).
const UNSUPPORTED_FORMAT: &str = "the call track's sample format is not supported";

/// The call tracks' device adapter (mic and system loopback). Holds no state: every `open`
/// resolves its device anew.
pub struct CpalTrackSource {
    _private: (),
}

impl CpalTrackSource {
    pub fn new() -> CpalTrackSource {
        CpalTrackSource { _private: () }
    }
}

impl Default for CpalTrackSource {
    fn default() -> CpalTrackSource {
        CpalTrackSource::new()
    }
}

/// One open call track: owns the stream. Dropping the stream ends cpal's stream thread
/// (joined in its `Drop`), so no callback runs once `stop` or `drop` returned.
struct TrackCapture {
    _stream: Stream,
}

impl CaptureHandle for TrackCapture {
    fn stop(self: Box<Self>) -> Result<(), CaptureError> {
        drop(self);
        Ok(())
    }
}

impl TrackSource for CpalTrackSource {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        let (stream, endpoint) =
            open_bounded(OPEN_BUDGET, move || open_track(track, &device, sink))
                .map_err(|e| open_error(&e))?;
        Ok((Box::new(TrackCapture { _stream: stream }), endpoint))
    }

    fn now(&self) -> CaptureStamp {
        qpc_now()
    }
}

/// Resolves, builds and starts the track's stream (runs on the opener thread).
fn open_track(
    track: Track,
    device: &TrackDevice,
    sink: Arc<dyn TrackSink>,
) -> Result<(Stream, EndpointId), CaptureError> {
    let host = cpal::default_host();
    let (device, supported) = match device {
        TrackDevice::Selected(id) => {
            let device = host
                .device_by_id(&cpal::DeviceId::new(host.id(), &id.0))
                .filter(|d| d.supports_input())
                .ok_or(CaptureError::NoDevice)?;
            let config = device
                .default_input_config()
                .map_err(|e| capture_error(&e))?;
            (device, config)
        }
        TrackDevice::DefaultInput => {
            let device = host.default_input_device().ok_or(CaptureError::NoDevice)?;
            let config = device
                .default_input_config()
                .map_err(|e| capture_error(&e))?;
            (device, config)
        }
        TrackDevice::DefaultOutputLoopback => {
            let device = host.default_output_device().ok_or(CaptureError::NoDevice)?;
            let config = device
                .default_output_config()
                .map_err(|e| capture_error(&e))?;
            (device, config)
        }
    };
    let endpoint = device
        .id()
        .map(|id| EndpointId::new(id.id()))
        .unwrap_or_else(|_| EndpointId::new(""));
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, track, sink),
        SampleFormat::F64 => build::<f64>(&device, config, track, sink),
        SampleFormat::I8 => build::<i8>(&device, config, track, sink),
        SampleFormat::I16 => build::<i16>(&device, config, track, sink),
        SampleFormat::I24 => build::<cpal::I24>(&device, config, track, sink),
        SampleFormat::I32 => build::<i32>(&device, config, track, sink),
        SampleFormat::I64 => build::<i64>(&device, config, track, sink),
        SampleFormat::U8 => build::<u8>(&device, config, track, sink),
        SampleFormat::U16 => build::<u16>(&device, config, track, sink),
        SampleFormat::U24 => build::<cpal::U24>(&device, config, track, sink),
        SampleFormat::U32 => build::<u32>(&device, config, track, sink),
        SampleFormat::U64 => build::<u64>(&device, config, track, sink),
        _ => return Err(CaptureError::Other(UNSUPPORTED_FORMAT.to_owned())),
    }
    .map_err(|e| capture_error(&e))?;
    stream.play().map_err(|e| capture_error(&e))?;
    Ok((stream, endpoint))
}

/// The input stream for `config` in sample type `T`: each callback converts its samples to
/// `f32` into one reused buffer and hands them to `sink` with cpal's capture instant.
fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    track: Track,
    sink: Arc<dyn TrackSink>,
) -> Result<Stream, Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let lost = Arc::clone(&sink);
    let rate = config.sample_rate;
    let channels = config.channels;
    let mut buf: Vec<f32> = Vec::new();
    device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            if data.is_empty() {
                return;
            }
            buf.clear();
            buf.extend(data.iter().map(|s| s.to_sample::<f32>()));
            sink.block(
                track,
                &buf,
                rate,
                channels,
                stamp_of(info.timestamp().capture),
            );
        },
        loss_callback(track, lost),
        None,
    )
}

/// The track loss of a cpal stream error: `StreamInvalidated` -> `Invalidated`,
/// `DeviceNotAvailable` -> `NotAvailable`, `Xrun` / `RealtimeDenied` / `DeviceChanged` -> none
/// (the track keeps delivering its device's audio), every other kind -> `Other` (the stream
/// thread has ended).
pub fn track_loss(kind: ErrorKind) -> Option<LossCause> {
    match kind {
        ErrorKind::StreamInvalidated => Some(LossCause::Invalidated),
        ErrorKind::DeviceNotAvailable => Some(LossCause::NotAvailable),
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => None,
        _ => Some(LossCause::Other),
    }
}

/// A call stream's error callback: the first error that is a loss ([`track_loss`]) reports
/// `sink.lost(track, cause, at)` with `at` read from the [`CpalTrackSource::now`] clock in the
/// callback; later errors report nothing. An error that is no loss neither reports nor uses up
/// the one report. It never invents blocks.
pub fn loss_callback(track: Track, sink: Arc<dyn TrackSink>) -> impl FnMut(Error) + Send + 'static {
    let mut reported = false;
    move |err: Error| {
        if reported {
            return;
        }
        if let Some(cause) = track_loss(err.kind()) {
            reported = true;
            sink.lost(track, cause, qpc_now());
        }
    }
}

/// The capture stamp of a cpal instant: exactly its nanoseconds, saturating at `u64::MAX`.
pub fn stamp_of(instant: StreamInstant) -> CaptureStamp {
    CaptureStamp(u64::try_from(instant.as_nanos()).unwrap_or(u64::MAX))
}

/// The open error of a capture error, one-to-one: `NoDevice` -> `NoDevice`, `AccessDenied` ->
/// `AccessDenied`, `DeviceBusy` -> `Busy`, `Other(_)` -> `Other` (its text is dropped).
pub fn open_error(err: &CaptureError) -> OpenError {
    match err {
        CaptureError::NoDevice => OpenError::NoDevice,
        CaptureError::AccessDenied => OpenError::AccessDenied,
        CaptureError::DeviceBusy => OpenError::Busy,
        CaptureError::Other(_) => OpenError::Other,
    }
}

/// `QueryPerformanceFrequency`, read once (constant for the system's lifetime); at least 1.
fn qpc_frequency() -> u128 {
    static FREQUENCY: OnceLock<u128> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency: i64 = 0;
        // SAFETY: the pointer is to a live local i64 for the duration of the call.
        let read = unsafe { QueryPerformanceFrequency(&mut frequency) };
        match read {
            Ok(()) => u128::try_from(frequency).unwrap_or(0).max(1),
            Err(_) => 1,
        }
    })
}

/// The QPC clock in nanoseconds with cpal's `Stream::now()` formula: the counter in 100 ns
/// units (counter x 10^7 / frequency, floored), x 100, saturating at `u64::MAX`.
fn qpc_now() -> CaptureStamp {
    let mut counter: i64 = 0;
    // SAFETY: the pointer is to a live local i64 for the duration of the call.
    // QPC cannot fail on Windows XP and later; a failed read counts as 0.
    if unsafe { QueryPerformanceCounter(&mut counter) }.is_err() {
        counter = 0;
    }
    let units_100ns = u128::try_from(counter).unwrap_or(0) * 10_000_000 / qpc_frequency();
    CaptureStamp(u64::try_from(units_100ns * 100).unwrap_or(u64::MAX))
}
