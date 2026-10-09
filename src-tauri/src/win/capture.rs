//! The microphones (T-006 design 2; T-012): `CpalSource`, an `AudioSource` over
//! cpal 0.18 (WASAPI). `devices` lists the active input endpoints with their
//! endpoint id (`IMMDevice::GetId`, research R-3), display name and the Windows
//! default flagged; `start` opens the device with that id (`device_by_id`, a
//! `Specific` device: no default-device following), never another one. Which
//! device a press uses is core's decision (`microphone::choose`). Both run on an
//! opener thread and wait for it at most a fixed budget, because cpal's
//! enumeration and device activation have no bound of their own; a stream that
//! opens after `start` gave up is dropped at once (NFR-02). cpal's errors map to
//! `CaptureError` kinds with fixed literals ([`capture_error`]): cpal's message is
//! never copied (P-009).
//!
//! Device loss (T-012): every call of a stream's error callback means the capture
//! ended (cpal 0.18.2 `run_input` ends a `Specific` device's stream thread after
//! it), so the first one reports `FrameSink::device_lost` and later ones nothing
//! ([`device_loss_callback`]).
//!
//! Lock contract (core `platform.rs`): `devices` and `start` return within
//! [`OPEN_BUDGET`] each and never call the session; the data callback calls only
//! the sink, and the error callback only the sink's `device_lost`, which never
//! takes the session lock, so dropping a handle (it joins cpal's stream thread)
//! never waits for a thread that waits for the session. No state is shared
//! between calls.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Error, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use voicen_core::platform::{AudioSource, CaptureHandle, DeviceId, FrameSink, InputDevice};
use voicen_core::recording::CaptureError;

/// How long a press waits for the device to open (OQ-12, T-006 Q1 default: 3 s);
/// the session lock is held meanwhile.
pub const OPEN_BUDGET: Duration = Duration::from_secs(3);

/// The fixed texts of `CaptureError::Other` (never OS text).
const OPEN_FAILED: &str = "the microphone could not be opened";
const OPEN_TIMED_OUT: &str = "the microphone did not open in time";
const OPENER_FAILED: &str = "the microphone opener could not be started";
const OPENER_ENDED: &str = "the microphone opener ended without a result";
const UNSUPPORTED_FORMAT: &str = "the microphone's sample format is not supported";
/// The display name of a device whose name cannot be read (never empty).
const UNNAMED: &str = "Microphone";

/// The input devices by WASAPI endpoint id (T-012).
pub struct CpalSource {
    _private: (),
}

impl CpalSource {
    pub fn new() -> CpalSource {
        CpalSource { _private: () }
    }
}

impl Default for CpalSource {
    fn default() -> CpalSource {
        CpalSource::new()
    }
}

/// One open capture: owns the stream. Dropping the stream ends cpal's stream
/// thread (joined in its `Drop`), so no callback runs once `stop` or `drop` returned.
struct CpalCapture {
    _stream: Stream,
}

impl CaptureHandle for CpalCapture {
    fn stop(self: Box<Self>) -> Result<(), CaptureError> {
        drop(self);
        Ok(())
    }
}

/// The input stream for `config` in sample type `T`: each callback converts its
/// samples to `f32`, stamps the instant first and hands them to `sink`.
fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    sink: Arc<dyn FrameSink>,
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
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let at = Instant::now();
            buf.clear();
            buf.extend(data.iter().map(|s| s.to_sample::<f32>()));
            sink.frames(&buf, rate, channels, at);
        },
        device_loss_callback(lost),
        None,
    )
}

/// A stream's error callback (T-012): the first call, whatever the error kind,
/// reports `sink.device_lost` with the instant stamped in the callback; later
/// calls report nothing. It never invents frames.
pub fn device_loss_callback(sink: Arc<dyn FrameSink>) -> impl FnMut(Error) + Send + 'static {
    let mut reported = false;
    move |_err: Error| {
        if !reported {
            reported = true;
            sink.device_lost(Instant::now());
        }
    }
}

/// The active input devices (runs on the opener thread): the endpoint id, the
/// display name and whether it is the Windows default (compared by id). A device
/// whose id cannot be read, or an id already listed, is skipped.
fn list() -> Result<Vec<InputDevice>, CaptureError> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.id().to_owned());
    let mut devices: Vec<InputDevice> = Vec::new();
    for device in host.input_devices().map_err(|e| capture_error(&e))? {
        let Ok(id) = device.id() else {
            continue;
        };
        let id = id.id().to_owned();
        if id.is_empty() || devices.iter().any(|d| d.id.0 == id) {
            continue;
        }
        let name = device
            .description()
            .ok()
            .map(|d| d.name().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| UNNAMED.to_owned());
        devices.push(InputDevice {
            is_default: default_id.as_deref() == Some(id.as_str()),
            id: DeviceId(id),
            name,
        });
    }
    Ok(devices)
}

/// Opens and starts the input device with endpoint id `id` (runs on the opener
/// thread); no such input device is `NoDevice`.
fn open(id: &DeviceId, sink: Arc<dyn FrameSink>) -> Result<Stream, CaptureError> {
    let host = cpal::default_host();
    let device = host
        .device_by_id(&cpal::DeviceId::new(host.id(), &id.0))
        .filter(|d| d.supports_input())
        .ok_or(CaptureError::NoDevice)?;
    let supported = device
        .default_input_config()
        .map_err(|e| capture_error(&e))?;
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, sink),
        SampleFormat::F64 => build::<f64>(&device, config, sink),
        SampleFormat::I8 => build::<i8>(&device, config, sink),
        SampleFormat::I16 => build::<i16>(&device, config, sink),
        SampleFormat::I24 => build::<cpal::I24>(&device, config, sink),
        SampleFormat::I32 => build::<i32>(&device, config, sink),
        SampleFormat::I64 => build::<i64>(&device, config, sink),
        SampleFormat::U8 => build::<u8>(&device, config, sink),
        SampleFormat::U16 => build::<u16>(&device, config, sink),
        SampleFormat::U24 => build::<cpal::U24>(&device, config, sink),
        SampleFormat::U32 => build::<u32>(&device, config, sink),
        SampleFormat::U64 => build::<u64>(&device, config, sink),
        _ => return Err(CaptureError::Other(UNSUPPORTED_FORMAT.to_owned())),
    }
    .map_err(|e| capture_error(&e))?;
    stream.play().map_err(|e| capture_error(&e))?;
    Ok(stream)
}

/// Runs `opener` on its own thread and waits for its result at most `budget`
/// (invariants 2 and 3): its error as is; a timeout, a failure to start the thread or
/// an opener that ended without a result (a panic) -> `Other` with a fixed literal; a
/// value produced after the timeout is dropped on the opener thread at once, never
/// handed out. `CpalSource::devices` and `CpalSource::start` use it with
/// `OPEN_BUDGET`.
pub fn open_bounded<T: Send + 'static>(
    budget: Duration,
    opener: impl FnOnce() -> Result<T, CaptureError> + Send + 'static,
) -> Result<T, CaptureError> {
    let (opened, result) = mpsc::channel::<Result<T, CaptureError>>();
    let spawned = thread::Builder::new()
        .name("mic-open".into())
        .spawn(move || {
            // After a timeout the receiver is gone: the send fails and drops the value
            // here, which (for a stream) closes the device at once (NFR-02). A panic
            // drops the sender instead: the caller sees `Disconnected` at once.
            let _ = opened.send(opener());
        });
    if spawned.is_err() {
        return Err(CaptureError::Other(OPENER_FAILED.to_owned()));
    }
    match result.recv_timeout(budget) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Timeout) => Err(CaptureError::Other(OPEN_TIMED_OUT.to_owned())),
        Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Other(OPENER_ENDED.to_owned())),
    }
}

impl AudioSource for CpalSource {
    fn devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        open_bounded(OPEN_BUDGET, list)
    }

    fn start(
        &self,
        device: &DeviceId,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let device = device.clone();
        let stream = open_bounded(OPEN_BUDGET, move || open(&device, sink))?;
        Ok(Box::new(CpalCapture { _stream: stream }))
    }
}

/// The capture error of a cpal error: `DeviceNotAvailable` -> `NoDevice`,
/// `PermissionDenied` -> `AccessDenied`, `DeviceBusy` -> `DeviceBusy`, anything
/// else -> `Other` with a fixed literal (never cpal's message: OS text).
pub fn capture_error(err: &Error) -> CaptureError {
    match err.kind() {
        ErrorKind::DeviceNotAvailable => CaptureError::NoDevice,
        ErrorKind::PermissionDenied => CaptureError::AccessDenied,
        ErrorKind::DeviceBusy => CaptureError::DeviceBusy,
        _ => CaptureError::Other(OPEN_FAILED.to_owned()),
    }
}
