//! A real-time [`AudioSource`] for tests (T-051; feature `test-fakes`, never in a
//! release build): frames from an [`AudioBuffer`] or PCM16 WAV bytes, pushed in
//! 10 ms chunks paced by real time and stamped with `Instant::now()`; silence once
//! the data runs out, as a live microphone would. T-006's `dictation_e2e` injects
//! it through the wiring function.
//!
//! The WAV reader is the inverse of [`wav::encode`](crate::audio::wav::encode),
//! plus any channel count and any rate; written by hand (`hound` is not consented,
//! decision #9).
//!
//! SKELETON (T-051 red tests): bodies are placeholders for the developer.

use std::sync::Arc;

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

/// Reads a PCM 16-bit little-endian RIFF WAV (any rate, any channel count).
pub fn read_wav(bytes: &[u8]) -> Result<Frames, WavError> {
    let _ = bytes;
    todo!("T-051: test_support::realtime::read_wav")
}

/// See the module docs.
pub struct RealtimeSource {
    _skeleton: (),
}

impl RealtimeSource {
    pub fn new(frames: Frames) -> RealtimeSource {
        let _ = frames;
        todo!("T-051: RealtimeSource::new")
    }

    /// 16 kHz mono frames from `audio`.
    pub fn from_buffer(audio: &AudioBuffer) -> RealtimeSource {
        let _ = audio;
        todo!("T-051: RealtimeSource::from_buffer")
    }

    pub fn from_wav(bytes: &[u8]) -> Result<RealtimeSource, WavError> {
        let _ = bytes;
        todo!("T-051: RealtimeSource::from_wav")
    }
}

impl AudioSource for RealtimeSource {
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let _ = sink;
        todo!("T-051: RealtimeSource::start")
    }
}
