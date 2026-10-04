//! Platform ports the pipeline delivers through (spec 001 T010,
//! contracts/core-traits.md "Platform traits", T-001).
//!
//! The Windows implementations live in `src-tauri` (T-006: clipboard and paste;
//! T-007: the file-system audio store). Errors carry no OS text (P-009). The
//! public fakes (feature `test-fakes`, decision #23 N4) sit next to the traits.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::AudioBuffer;
use crate::recording::{CaptureError, OverlayState, TrayState};
use crate::settings::gate::SettingsTab;

/// An opaque window handle (the HWND in the shell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowRef(pub u64);

/// The window that was in front when the recording started (data-model
/// "StartWindow").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartWindow {
    /// Root owner of the foreground window at start.
    pub handle: WindowRef,
    pub process_id: u32,
    /// Target integrity level above ours, or unknown.
    pub elevated: bool,
}

/// The id of the one pending (failed, retryable) recording; monotonic per pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PendingId(pub(crate) u64);

/// The clipboard could not be written (no OS text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardError;

/// Ctrl+V could not be sent (no OS text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteError;

/// The system clipboard.
pub trait Clipboard: Send + Sync {
    /// Writes text with the history/cloud exclusion formats (FR-022). Retries
    /// internally.
    fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError>;
}

/// Paste into the start window.
pub trait Paster: Send + Sync {
    fn capture_start_window(&self) -> Option<StartWindow>;
    /// Waits at most `max_wait` for Shift/Ctrl/Alt/Win to be released; false if
    /// still held.
    fn wait_modifiers_released(&self, max_wait: Duration) -> bool;
    fn is_in_front(&self, w: &StartWindow) -> bool;
    fn send_ctrl_v(&self) -> Result<(), PasteError>;
}

/// Where the pending recording's audio is kept (R-18; T-007 builds the file store).
pub trait TempAudioStore: Send + Sync {
    fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> std::io::Result<()>;
    fn get_pending(&self, id: PendingId) -> std::io::Result<AudioBuffer>;
    fn delete_pending(&self, id: PendingId) -> std::io::Result<()>;
    /// At start and on exit (FR-032).
    fn delete_all(&self) -> std::io::Result<()>;
}

/// Receives the frames of one capture (T-051). Called on the adapter's audio
/// thread; it must not block.
pub trait FrameSink: Send + Sync {
    /// Interleaved `f32` samples in `[-1.0, 1.0]` at `rate` Hz with `channels`
    /// channels. `at` is the instant the adapter stamped in its callback; the first
    /// one of a capture gives `RecordingStarted.hotkey_to_first_frame_ms`.
    fn frames(&self, interleaved: &[f32], rate: u32, channels: u16, at: Instant);
}

/// One running capture. Once [`stop`](Self::stop) returns, or the handle is
/// dropped, the sink is called no more and the device is closed (NFR-02).
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>) -> Result<(), CaptureError>;
}

/// The microphone (T-051: the default input device; T-012 adds the device choice
/// and device loss).
pub trait AudioSource: Send + Sync {
    /// Opens the device and starts delivering frames to `sink`.
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}

/// The tray icon and the overlay (contracts/core-traits.md). The session calls it
/// from inside its lock, once per change; an implementation must not block on
/// another thread and must never call back into the session.
pub trait Indicator: Send + Sync {
    fn set_tray(&self, state: TrayState, retry_available: bool);
    fn set_overlay(&self, state: &OverlayState);
}

/// What the session asks the shell to do (T-051: the open-settings action of
/// `settings::gate::blocked_actions`; T-055 adds the field focus, T-007 the
/// notifier).
pub trait ShellRequests: Send + Sync {
    fn open_settings(&self, tab: SettingsTab);
}

#[cfg(any(test, feature = "test-fakes"))]
pub use fakes::*;

#[cfg(any(test, feature = "test-fakes"))]
mod fakes {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use super::{
        AudioBuffer, AudioSource, CaptureError, CaptureHandle, Clipboard, ClipboardError,
        FrameSink, Indicator, OverlayState, PasteError, Paster, PendingId, SettingsTab,
        ShellRequests, StartWindow, TempAudioStore, TrayState, WindowRef,
    };

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// In-memory [`Clipboard`]: records every text it is asked to write (failed
    /// attempts included) and fails while [`set_fail`](Self::set_fail) is on.
    #[derive(Default)]
    pub struct FakeClipboard {
        texts: Mutex<Vec<String>>,
        fail: Mutex<bool>,
    }

    impl FakeClipboard {
        pub fn new() -> FakeClipboard {
            FakeClipboard::default()
        }

        /// Every `set_text_excluded_from_history` argument so far, in order.
        pub fn texts(&self) -> Vec<String> {
            lock(&self.texts).clone()
        }

        /// While on, every write returns `Err(ClipboardError)` (still recorded).
        pub fn set_fail(&self, fail: bool) {
            *lock(&self.fail) = fail;
        }
    }

    impl Clipboard for FakeClipboard {
        fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError> {
            lock(&self.texts).push(text.to_string());
            if *lock(&self.fail) {
                Err(ClipboardError)
            } else {
                Ok(())
            }
        }
    }

    /// One recorded call on [`FakePaster`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PasterCall {
        CaptureStartWindow,
        WaitModifiersReleased(Duration),
        IsInFront(WindowRef),
        SendCtrlV,
    }

    struct PasterScript {
        start_window: Option<StartWindow>,
        modifiers_released: bool,
        in_front: bool,
        send_ok: bool,
    }

    /// Scripted [`Paster`] with a call log. Defaults: no start window captured,
    /// modifiers released, the window in front, Ctrl+V sent. `wait_modifiers_released`
    /// returns at once (it never sleeps).
    pub struct FakePaster {
        script: Mutex<PasterScript>,
        calls: Mutex<Vec<PasterCall>>,
    }

    impl Default for FakePaster {
        fn default() -> FakePaster {
            FakePaster {
                script: Mutex::new(PasterScript {
                    start_window: None,
                    modifiers_released: true,
                    in_front: true,
                    send_ok: true,
                }),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl FakePaster {
        pub fn new() -> FakePaster {
            FakePaster::default()
        }

        /// What `capture_start_window` returns.
        pub fn with_start_window(self, w: Option<StartWindow>) -> FakePaster {
            lock(&self.script).start_window = w;
            self
        }

        /// What `capture_start_window` returns from now on (the foreground window
        /// changed; T-051).
        pub fn set_start_window(&self, w: Option<StartWindow>) {
            lock(&self.script).start_window = w;
        }

        /// What `wait_modifiers_released` returns.
        pub fn with_modifiers_released(self, released: bool) -> FakePaster {
            lock(&self.script).modifiers_released = released;
            self
        }

        /// What `is_in_front` returns.
        pub fn with_in_front(self, in_front: bool) -> FakePaster {
            lock(&self.script).in_front = in_front;
            self
        }

        /// `send_ctrl_v` returns `Err(PasteError)`.
        pub fn with_send_error(self) -> FakePaster {
            lock(&self.script).send_ok = false;
            self
        }

        /// Every trait call so far, in order.
        pub fn calls(&self) -> Vec<PasterCall> {
            lock(&self.calls).clone()
        }

        fn record(&self, call: PasterCall) {
            lock(&self.calls).push(call);
        }
    }

    impl Paster for FakePaster {
        fn capture_start_window(&self) -> Option<StartWindow> {
            self.record(PasterCall::CaptureStartWindow);
            lock(&self.script).start_window.clone()
        }

        fn wait_modifiers_released(&self, max_wait: Duration) -> bool {
            self.record(PasterCall::WaitModifiersReleased(max_wait));
            lock(&self.script).modifiers_released
        }

        fn is_in_front(&self, w: &StartWindow) -> bool {
            self.record(PasterCall::IsInFront(w.handle));
            lock(&self.script).in_front
        }

        fn send_ctrl_v(&self) -> Result<(), PasteError> {
            self.record(PasterCall::SendCtrlV);
            if lock(&self.script).send_ok {
                Ok(())
            } else {
                Err(PasteError)
            }
        }
    }

    /// One recorded call on [`FakeTempAudioStore`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum StoreCall {
        Put(PendingId),
        Get(PendingId),
        Delete(PendingId),
        DeleteAll,
    }

    /// In-memory [`TempAudioStore`] with a call log and a put-fail switch.
    #[derive(Default)]
    pub struct FakeTempAudioStore {
        audio: Mutex<BTreeMap<PendingId, AudioBuffer>>,
        calls: Mutex<Vec<StoreCall>>,
        fail_put: Mutex<bool>,
    }

    fn not_found() -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::NotFound)
    }

    impl FakeTempAudioStore {
        pub fn new() -> FakeTempAudioStore {
            FakeTempAudioStore::default()
        }

        /// While on, `put_pending` stores nothing and returns an error (still
        /// recorded).
        pub fn set_fail_put(&self, fail: bool) {
            *lock(&self.fail_put) = fail;
        }

        /// Every trait call so far, in order.
        pub fn calls(&self) -> Vec<StoreCall> {
            lock(&self.calls).clone()
        }

        /// The ids currently stored, ascending (not recorded as a call).
        pub fn stored_ids(&self) -> Vec<PendingId> {
            lock(&self.audio).keys().copied().collect()
        }

        fn record(&self, call: StoreCall) {
            lock(&self.calls).push(call);
        }
    }

    impl TempAudioStore for FakeTempAudioStore {
        fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> std::io::Result<()> {
            self.record(StoreCall::Put(id));
            if *lock(&self.fail_put) {
                return Err(std::io::Error::other("fake put failure"));
            }
            lock(&self.audio).insert(id, audio.clone());
            Ok(())
        }

        fn get_pending(&self, id: PendingId) -> std::io::Result<AudioBuffer> {
            self.record(StoreCall::Get(id));
            lock(&self.audio).get(&id).cloned().ok_or_else(not_found)
        }

        fn delete_pending(&self, id: PendingId) -> std::io::Result<()> {
            self.record(StoreCall::Delete(id));
            lock(&self.audio).remove(&id);
            Ok(())
        }

        fn delete_all(&self) -> std::io::Result<()> {
            self.record(StoreCall::DeleteAll);
            lock(&self.audio).clear();
            Ok(())
        }
    }

    /// One block of frames a [`FakeAudioSource`] capture delivers, with the instant
    /// the adapter would have stamped in its callback.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FrameChunk {
        pub samples: Vec<f32>,
        pub rate: u32,
        pub channels: u16,
        pub at: Instant,
    }

    impl FrameChunk {
        /// `audio` (16 kHz mono) as `f32` frames (`sample / 32768`), stamped `at`.
        pub fn from_buffer(audio: &AudioBuffer, at: Instant) -> FrameChunk {
            FrameChunk {
                samples: audio
                    .samples()
                    .iter()
                    .map(|&s| f32::from(s) / 32768.0)
                    .collect(),
                rate: crate::audio::SAMPLE_RATE,
                channels: 1,
                at,
            }
        }
    }

    #[derive(Default)]
    struct SourceScript {
        start_error: Option<CaptureError>,
        stop_error: Option<CaptureError>,
        chunks: Vec<FrameChunk>,
    }

    #[derive(Default)]
    struct SourceState {
        script: Mutex<SourceScript>,
        start_calls: Mutex<usize>,
        open: Mutex<usize>,
    }

    /// Scripted [`AudioSource`] with counters. Each successful `start` opens one
    /// handle and delivers the scripted chunks, in order, from its own capture
    /// thread (as a real adapter's callback would); `stop`, or dropping the handle,
    /// joins that thread and then closes the handle, so every chunk has reached the
    /// sink before `stop` returns and none arrives after. Defaults: `start`
    /// succeeds, no chunks, `stop` succeeds.
    #[derive(Default)]
    pub struct FakeAudioSource {
        state: Arc<SourceState>,
    }

    impl FakeAudioSource {
        pub fn new() -> FakeAudioSource {
            FakeAudioSource::default()
        }

        /// While `Some`, `start` returns this error (the call is still counted) and
        /// opens nothing.
        pub fn set_start_error(&self, err: Option<CaptureError>) {
            lock(&self.state.script).start_error = err;
        }

        /// While `Some`, `stop` returns this error; the handle still closes.
        pub fn set_stop_error(&self, err: Option<CaptureError>) {
            lock(&self.state.script).stop_error = err;
        }

        /// The chunks every later capture delivers.
        pub fn set_chunks(&self, chunks: Vec<FrameChunk>) {
            lock(&self.state.script).chunks = chunks;
        }

        /// Every `start` call so far, failed ones included.
        pub fn start_calls(&self) -> usize {
            *lock(&self.state.start_calls)
        }

        /// Handles started and not yet stopped or dropped: the device is open.
        pub fn open_handles(&self) -> usize {
            *lock(&self.state.open)
        }
    }

    impl AudioSource for FakeAudioSource {
        fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
            *lock(&self.state.start_calls) += 1;
            let chunks = {
                let script = lock(&self.state.script);
                if let Some(err) = script.start_error.clone() {
                    return Err(err);
                }
                script.chunks.clone()
            };
            *lock(&self.state.open) += 1;
            let deliver = move || {
                for c in &chunks {
                    sink.frames(&c.samples, c.rate, c.channels, c.at);
                }
            };
            let thread = match std::thread::Builder::new()
                .name("fake-capture".to_string())
                .spawn(deliver)
            {
                Ok(t) => t,
                Err(e) => panic!("cannot spawn the fake capture thread: {e}"),
            };
            Ok(Box::new(FakeCapture {
                state: Arc::clone(&self.state),
                thread: Some(thread),
                closed: false,
            }))
        }
    }

    struct FakeCapture {
        state: Arc<SourceState>,
        thread: Option<JoinHandle<()>>,
        closed: bool,
    }

    impl FakeCapture {
        fn close(&mut self) {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            if !self.closed {
                self.closed = true;
                let mut open = lock(&self.state.open);
                *open = open.saturating_sub(1);
            }
        }
    }

    impl CaptureHandle for FakeCapture {
        fn stop(mut self: Box<Self>) -> Result<(), CaptureError> {
            self.close();
            match lock(&self.state.script).stop_error.clone() {
                Some(err) => Err(err),
                None => Ok(()),
            }
        }
    }

    impl Drop for FakeCapture {
        fn drop(&mut self) {
            self.close();
        }
    }

    /// One recorded call on [`FakeIndicator`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum IndicatorCall {
        Tray(TrayState, bool),
        Overlay(OverlayState),
    }

    /// [`Indicator`] that records every call in order, with the instant it came.
    #[derive(Default)]
    pub struct FakeIndicator {
        calls: Mutex<Vec<(Instant, IndicatorCall)>>,
    }

    impl FakeIndicator {
        pub fn new() -> FakeIndicator {
            FakeIndicator::default()
        }

        /// Every call so far, in order.
        pub fn calls(&self) -> Vec<IndicatorCall> {
            lock(&self.calls).iter().map(|(_, c)| c.clone()).collect()
        }

        /// Every call so far with the instant it was made.
        pub fn timed_calls(&self) -> Vec<(Instant, IndicatorCall)> {
            lock(&self.calls).clone()
        }

        /// The `set_tray` arguments so far, in order.
        pub fn trays(&self) -> Vec<(TrayState, bool)> {
            lock(&self.calls)
                .iter()
                .filter_map(|(_, c)| match c {
                    IndicatorCall::Tray(state, retry) => Some((*state, *retry)),
                    IndicatorCall::Overlay(_) => None,
                })
                .collect()
        }

        /// The `set_overlay` arguments so far, in order.
        pub fn overlays(&self) -> Vec<OverlayState> {
            lock(&self.calls)
                .iter()
                .filter_map(|(_, c)| match c {
                    IndicatorCall::Overlay(state) => Some(state.clone()),
                    IndicatorCall::Tray(..) => None,
                })
                .collect()
        }
    }

    impl Indicator for FakeIndicator {
        fn set_tray(&self, state: TrayState, retry_available: bool) {
            lock(&self.calls).push((Instant::now(), IndicatorCall::Tray(state, retry_available)));
        }

        fn set_overlay(&self, state: &OverlayState) {
            lock(&self.calls).push((Instant::now(), IndicatorCall::Overlay(state.clone())));
        }
    }

    /// One recorded call on [`FakeShellRequests`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ShellRequestCall {
        OpenSettings(SettingsTab),
    }

    /// [`ShellRequests`] that records every request in order.
    #[derive(Default)]
    pub struct FakeShellRequests {
        calls: Mutex<Vec<ShellRequestCall>>,
    }

    impl FakeShellRequests {
        pub fn new() -> FakeShellRequests {
            FakeShellRequests::default()
        }

        /// Every request so far, in order.
        pub fn calls(&self) -> Vec<ShellRequestCall> {
            lock(&self.calls).clone()
        }
    }

    impl ShellRequests for FakeShellRequests {
        fn open_settings(&self, tab: SettingsTab) {
            lock(&self.calls).push(ShellRequestCall::OpenSettings(tab));
        }
    }
}
