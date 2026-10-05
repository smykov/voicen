//! The microphone (T-006 design 2): `CpalSource`, an `AudioSource` over the Windows
//! default input device (cpal 0.18, WASAPI). The open runs on an opener thread and
//! `start` waits for it at most a fixed budget, because cpal's device activation
//! has no bound of its own; a stream that opens after `start` gave up is dropped at
//! once (NFR-02). cpal's errors map to `CaptureError` kinds with fixed literals
//! ([`capture_error`]): cpal's message is never copied (P-009).
//!
//! Lock contract (core `platform.rs`): `start` returns within [`OPEN_BUDGET`] and
//! never calls the session; the data callback calls only the sink, and the error
//! callback records nothing, so dropping a handle (it joins cpal's stream thread)
//! never waits for a thread that calls the session. No state is shared between
//! calls.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Error, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use voicen_core::platform::{AudioSource, CaptureHandle, FrameSink};
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

/// The Windows default input device (T-012 adds the device choice).
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
        // A device lost mid-recording just ends the frames (T-012 owns its notice).
        |_err: Error| {},
        None,
    )
}

/// Opens and starts the default input device (runs on the opener thread).
fn open(sink: Arc<dyn FrameSink>) -> Result<Stream, CaptureError> {
    let device = cpal::default_host()
        .default_input_device()
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
/// handed out. `CpalSource::start` uses it with `OPEN_BUDGET` and `open(sink)`.
pub fn open_bounded<T: Send + 'static>(
    budget: Duration,
    opener: impl FnOnce() -> Result<T, CaptureError> + Send + 'static,
) -> Result<T, CaptureError> {
    let _ = (budget, opener);
    todo!("T-006 review 1")
}

impl AudioSource for CpalSource {
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let (opened, result) = mpsc::channel::<Result<Stream, CaptureError>>();
        let spawned = thread::Builder::new()
            .name("mic-open".into())
            .spawn(move || {
                // After a timeout the receiver is gone: the send fails and drops the
                // stream here, which closes the device at once (NFR-02).
                let _ = opened.send(open(sink));
            });
        if spawned.is_err() {
            return Err(CaptureError::Other(OPENER_FAILED.to_owned()));
        }
        match result.recv_timeout(OPEN_BUDGET) {
            Ok(Ok(stream)) => Ok(Box::new(CpalCapture { _stream: stream })),
            Ok(Err(err)) => Err(err),
            Err(RecvTimeoutError::Timeout) => Err(CaptureError::Other(OPEN_TIMED_OUT.to_owned())),
            Err(RecvTimeoutError::Disconnected) => {
                Err(CaptureError::Other(OPENER_ENDED.to_owned()))
            }
        }
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
