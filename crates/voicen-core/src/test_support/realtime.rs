//! A real-time [`AudioSource`] for tests (T-051; feature `test-fakes`, never in a
//! release build): frames from an [`AudioBuffer`] or PCM16 WAV bytes, pushed in
//! 10 ms chunks paced by real time and stamped with `Instant::now()`; silence once
//! the data runs out, as a live microphone would. T-006's `dictation_e2e` injects
//! it through the wiring function.
//!
//! The WAV reader is the inverse of [`wav::encode`](crate::audio::wav::encode),
//! plus any channel count and any rate; written by hand (`hound` is not consented,
//! decision #9).

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::AudioBuffer;
use crate::platform::{AudioSource, CaptureHandle, FrameSink};
use crate::recording::CaptureError;

/// Interleaved `f32` frames in `[-1.0, 1.0]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frames {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u16,
}

/// The bytes are not a PCM 16-bit WAV this reader understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavError;

/// `WAVE_FORMAT_PCM`.
const FORMAT_PCM: u16 = 1;

/// The little-endian `u16` at `at`, if the bytes reach that far.
fn u16_at(bytes: &[u8], at: usize) -> Result<u16, WavError> {
    let b = bytes
        .get(at..at.checked_add(2).ok_or(WavError)?)
        .ok_or(WavError)?;
    Ok(u16::from_le_bytes([
        *b.first().ok_or(WavError)?,
        *b.get(1).ok_or(WavError)?,
    ]))
}

/// The little-endian `u32` at `at`, if the bytes reach that far.
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, WavError> {
    let lo = u16_at(bytes, at)?;
    let hi = u16_at(bytes, at.checked_add(2).ok_or(WavError)?)?;
    Ok(u32::from(lo) | (u32::from(hi) << 16))
}

/// Reads a PCM 16-bit little-endian RIFF WAV (any rate, any channel count).
///
/// The `fmt ` chunk must say PCM, 16 bits, at least one channel and a rate above
/// zero; the `data` chunk must lie inside the bytes and hold whole frames. Other
/// chunks are skipped. Anything else is a [`WavError`], never a panic.
pub fn read_wav(bytes: &[u8]) -> Result<Frames, WavError> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(WavError);
    }
    let mut format: Option<(u32, u16)> = None;
    let mut at = 12usize;
    loop {
        let id = bytes
            .get(at..at.checked_add(4).ok_or(WavError)?)
            .ok_or(WavError)?;
        let len = usize::try_from(u32_at(bytes, at.checked_add(4).ok_or(WavError)?)?)
            .map_err(|_| WavError)?;
        let body = at.checked_add(8).ok_or(WavError)?;
        let end = body.checked_add(len).ok_or(WavError)?;
        if id == b"fmt " {
            if len < 16 || end > bytes.len() {
                return Err(WavError);
            }
            let tag = u16_at(bytes, body)?;
            let channels = u16_at(bytes, body + 2)?;
            let rate = u32_at(bytes, body + 4)?;
            let bits = u16_at(bytes, body + 14)?;
            if tag != FORMAT_PCM || bits != 16 || channels == 0 || rate == 0 {
                return Err(WavError);
            }
            format = Some((rate, channels));
        } else if id == b"data" {
            let (rate, channels) = format.ok_or(WavError)?;
            let data = bytes.get(body..end).ok_or(WavError)?;
            let frame_bytes = usize::from(channels).checked_mul(2).ok_or(WavError)?;
            if !data.len().is_multiple_of(frame_bytes) {
                return Err(WavError);
            }
            let (pairs, _) = data.as_chunks::<2>();
            let samples = pairs
                .iter()
                .map(|&pair| f32::from(i16::from_le_bytes(pair)) / 32768.0)
                .collect();
            return Ok(Frames {
                samples,
                rate,
                channels,
            });
        }
        // Chunks are padded to an even length.
        at = end.checked_add(len % 2).ok_or(WavError)?;
    }
}

/// How often the source pushes a chunk.
const CHUNK: Duration = Duration::from_millis(10);

/// See the module docs.
pub struct RealtimeSource {
    frames: Arc<Frames>,
}

impl RealtimeSource {
    pub fn new(frames: Frames) -> RealtimeSource {
        RealtimeSource {
            frames: Arc::new(frames),
        }
    }

    /// 16 kHz mono frames from `audio`.
    pub fn from_buffer(audio: &AudioBuffer) -> RealtimeSource {
        RealtimeSource::new(Frames {
            samples: audio
                .samples()
                .iter()
                .map(|&s| f32::from(s) / 32768.0)
                .collect(),
            rate: crate::audio::SAMPLE_RATE,
            channels: 1,
        })
    }

    pub fn from_wav(bytes: &[u8]) -> Result<RealtimeSource, WavError> {
        read_wav(bytes).map(RealtimeSource::new)
    }
}

impl AudioSource for RealtimeSource {
    /// Starts the push thread: chunk `k` (10 ms of frames, the data first and then
    /// silence) is pushed once `k × 10 ms` have passed since the start, so a late
    /// wake-up catches up but never runs ahead of real time.
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let frames = Arc::clone(&self.frames);
        if frames.rate == 0 || frames.channels == 0 {
            return Err(CaptureError::Other(
                "test source: no rate or channels".into(),
            ));
        }
        let channels = usize::from(frames.channels);
        let per_chunk = usize::try_from(frames.rate / 100).unwrap_or(1).max(1) * channels;
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let push = move || {
            let started = Instant::now();
            let mut pos = 0usize;
            let mut chunk = Vec::with_capacity(per_chunk);
            for k in 0u32.. {
                let due = started + CHUNK * k;
                let now = Instant::now();
                if due > now {
                    match stop_rx.recv_timeout(due - now) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                } else if !matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                    return;
                }
                chunk.clear();
                let end = pos.saturating_add(per_chunk).min(frames.samples.len());
                chunk.extend_from_slice(frames.samples.get(pos..end).unwrap_or(&[]));
                pos = end;
                chunk.resize(per_chunk, 0.0);
                sink.frames(&chunk, frames.rate, frames.channels, Instant::now());
            }
        };
        let thread = std::thread::Builder::new()
            .name("realtime-source".to_string())
            .spawn(push)
            .map_err(|_| CaptureError::Other("test source: cannot spawn".into()))?;
        Ok(Box::new(RealtimeCapture {
            stop: Some(stop_tx),
            thread: Some(thread),
        }))
    }
}

/// One running [`RealtimeSource`] capture; stopping or dropping it joins the push
/// thread, so no frame reaches the sink afterwards.
struct RealtimeCapture {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl RealtimeCapture {
    fn close(&mut self) {
        // Dropping the sender wakes the push thread (`Disconnected`).
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl CaptureHandle for RealtimeCapture {
    fn stop(mut self: Box<Self>) -> Result<(), CaptureError> {
        self.close();
        Ok(())
    }
}

impl Drop for RealtimeCapture {
    fn drop(&mut self) {
        self.close();
    }
}
