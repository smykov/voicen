//! Platform ports the pipeline delivers through (spec 001 T010,
//! contracts/core-traits.md "Platform traits", T-001).
//!
//! The Windows implementations live in `src-tauri` (T-006: capture, clipboard and
//! paste; T-007: the file-system audio store; T-052/T-053: the tray and the
//! overlay behind `Indicator`). The capture, indicator and shell-request ports are
//! called only by the dictation session (`crate::dictation`, T-051). Errors carry
//! no OS text (P-009). The public fakes (feature `test-fakes`, decision #23 N4)
//! sit next to the traits.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::AudioBuffer;
use crate::recording::{CaptureError, OverlayState, TrayState};
use crate::settings::gate::SettingsTab;
use crate::settings::FieldId;

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

/// Paste into the start window. The dictation session's worker calls the delivery
/// methods (`wait_modifiers_released`, `is_in_front`, `send_ctrl_v`) with no
/// session lock held.
pub trait Paster: Send + Sync {
    /// The window in front at a press. The session calls it with its lock held
    /// (T-051): it must return promptly, must never call back into the session,
    /// and must not wait for another call on this paster, which the worker may be
    /// running (`wait_modifiers_released` waits up to `delivery::MODIFIER_WAIT`):
    /// no one mutex held across the paster's methods.
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

    /// The capture ended without being asked to (T-012, spec 001 FR-031: the device
    /// was unplugged or failed); `at` is the instant the adapter stamped. The
    /// adapter reports it at most once per capture, from any thread, also from
    /// inside `AudioSource::start` or while its handle is dropped. The session's
    /// sink only records it and hands it off without blocking, and never takes the
    /// session lock, so the call is safe in all of those places.
    fn device_lost(&self, at: Instant);
}

/// An input device's identity: the WASAPI endpoint id in the shell (research R-3),
/// the saved `settings::Microphone::id`. Matched as a whole string only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// One input device of [`AudioSource::devices`] (T-012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub id: DeviceId,
    /// The display name (what `notice.mic_fallback` and the settings show).
    pub name: String,
    /// The Windows default input device; at most one entry is.
    pub is_default: bool,
}

/// One running capture. Once [`stop`](Self::stop) returns, or the handle is
/// dropped, the sink is called no more and the device is closed (NFR-02).
///
/// The session calls `stop` with no session lock held, but it may drop a live
/// handle with its lock held (the session's `Drop` while a recording is on):
/// dropping must return promptly, must never call back into the session and must
/// not wait for a thread that does (an audio callback reporting to the session).
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>) -> Result<(), CaptureError>;
}

/// The microphones (T-051; T-012: the device list and the open by id). The
/// adapter enumerates and opens; which device a press uses is decided only by
/// core (`crate::microphone::choose` over this list).
pub trait AudioSource: Send + Sync {
    /// The input devices present now (unplugged ones are absent), with the Windows
    /// default flagged. The one list for a press and for the settings UI
    /// (`settings_list_microphones`). The session calls it at a press with its lock
    /// held, under the same rules as [`start`](Self::start).
    fn devices(&self) -> Result<Vec<InputDevice>, CaptureError>;

    /// Opens the device `device` (an id of [`devices`](Self::devices); an id no
    /// device has is `Err(NoDevice)`, never another device) and starts delivering
    /// frames to `sink`; an end of the capture it was not asked for is reported
    /// once through [`FrameSink::device_lost`]. The session calls it at a press
    /// with its lock held (T-051): it must return promptly (once the device is open
    /// or has failed), must never call back into the session (an error or
    /// device-loss path inside `start` included), and must not wait for another
    /// call on this source or its handles, such as a `CaptureHandle::stop` running
    /// on another thread.
    fn start(
        &self,
        device: &DeviceId,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}

/// The tray icon and the overlay (contracts/core-traits.md). The session calls it
/// from inside its lock, once per change; an implementation must not block on
/// another thread and must never call back into the session.
pub trait Indicator: Send + Sync {
    fn set_tray(&self, state: TrayState, retry_available: bool);
    fn set_overlay(&self, state: &OverlayState);
}

/// What the session asks the shell to do (T-051: the open-settings action of
/// `settings::gate::blocked_actions`; T-055: the field to focus, for a failed
/// hotkey registration; T-007 adds the notifier). The session calls it outside its
/// lock, so an implementation may call back into the session.
pub trait ShellRequests: Send + Sync {
    /// Open (or raise) the settings window on `tab`, focusing `field` when given.
    fn open_settings(&self, tab: SettingsTab, field: Option<FieldId>);
}

/// The Esc claim (T-009, FR-22): the session asks for Esc while a recording is on
/// and gives it back when the recording ends, once per change of
/// `RecordingController::live_id().is_some()`. The session calls it from inside
/// its lock: an implementation must not block on another thread and must never
/// call back into the session from `set`; it reports the outcome later through
/// `DictationSession::cancel_key_result`, and an Esc press through
/// `DictationSession::esc_pressed`.
pub trait CancelKey: Send + Sync {
    fn set(&self, claimed: bool);
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
        AudioBuffer, AudioSource, CancelKey, CaptureError, CaptureHandle, Clipboard,
        ClipboardError, DeviceId, FieldId, FrameSink, Indicator, InputDevice, OverlayState,
        PasteError, Paster, PendingId, SettingsTab, ShellRequests, StartWindow, TempAudioStore,
        TrayState, WindowRef,
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

    struct SourceScript {
        start_error: Option<CaptureError>,
        stop_error: Option<CaptureError>,
        chunks: Vec<FrameChunk>,
        devices: Vec<InputDevice>,
        devices_error: Option<CaptureError>,
        device_loss: Option<Instant>,
    }

    impl Default for SourceScript {
        fn default() -> SourceScript {
            SourceScript {
                start_error: None,
                stop_error: None,
                chunks: Vec::new(),
                devices: vec![FakeAudioSource::default_device()],
                devices_error: None,
                device_loss: None,
            }
        }
    }

    #[derive(Default)]
    struct SourceState {
        script: Mutex<SourceScript>,
        started_with: Mutex<Vec<DeviceId>>,
        open: Mutex<usize>,
    }

    /// Scripted [`AudioSource`] with counters. Each successful `start` opens one
    /// handle and delivers the scripted chunks, in order, from its own capture
    /// thread (as a real adapter's callback would); `stop`, or dropping the handle,
    /// joins that thread and then closes the handle, so every chunk has reached the
    /// sink before `stop` returns and none arrives after. Defaults: one device,
    /// [`default_device`](Self::default_device), flagged default; `start` succeeds
    /// for an id in the list (any other id is `Err(NoDevice)`, counted, nothing
    /// opened), no chunks, no device loss, `stop` succeeds.
    #[derive(Default)]
    pub struct FakeAudioSource {
        state: Arc<SourceState>,
    }

    impl FakeAudioSource {
        pub fn new() -> FakeAudioSource {
            FakeAudioSource::default()
        }

        /// The one device of the default list: a fake endpoint, the Windows
        /// default.
        pub fn default_device() -> InputDevice {
            InputDevice {
                id: DeviceId("{0.0.1.00000000}.{fake-default-mic}".to_string()),
                name: "Default Microphone (fake)".to_string(),
                is_default: true,
            }
        }

        /// What `devices()` returns from now on (and which ids `start` opens).
        pub fn set_devices(&self, devices: Vec<InputDevice>) {
            lock(&self.state.script).devices = devices;
        }

        /// While `Some`, `devices()` returns this error.
        pub fn set_devices_error(&self, err: Option<CaptureError>) {
            lock(&self.state.script).devices_error = err;
        }

        /// While `Some(at)`, each later capture, after its chunks, reports
        /// `sink.device_lost(at)` once from its capture thread.
        pub fn set_device_loss(&self, at: Option<Instant>) {
            lock(&self.state.script).device_loss = at;
        }

        /// The id of every `start` call so far, failed ones included.
        pub fn started_with(&self) -> Vec<DeviceId> {
            lock(&self.state.started_with).clone()
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
            lock(&self.state.started_with).len()
        }

        /// Handles started and not yet stopped or dropped: the device is open.
        pub fn open_handles(&self) -> usize {
            *lock(&self.state.open)
        }
    }

    impl AudioSource for FakeAudioSource {
        fn devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
            let script = lock(&self.state.script);
            match script.devices_error.clone() {
                Some(err) => Err(err),
                None => Ok(script.devices.clone()),
            }
        }

        fn start(
            &self,
            device: &DeviceId,
            sink: Arc<dyn FrameSink>,
        ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
            lock(&self.state.started_with).push(device.clone());
            let (chunks, loss) = {
                let script = lock(&self.state.script);
                if let Some(err) = script.start_error.clone() {
                    return Err(err);
                }
                if !script.devices.iter().any(|d| &d.id == device) {
                    return Err(CaptureError::NoDevice);
                }
                (script.chunks.clone(), script.device_loss)
            };
            *lock(&self.state.open) += 1;
            let deliver = move || {
                for c in &chunks {
                    sink.frames(&c.samples, c.rate, c.channels, c.at);
                }
                if let Some(at) = loss {
                    sink.device_lost(at);
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
        OpenSettings(SettingsTab, Option<FieldId>),
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
        fn open_settings(&self, tab: SettingsTab, field: Option<FieldId>) {
            lock(&self.calls).push(ShellRequestCall::OpenSettings(tab, field));
        }
    }

    /// [`CancelKey`] that records every `set` in order.
    #[derive(Default)]
    pub struct FakeCancelKey {
        calls: Mutex<Vec<bool>>,
    }

    impl FakeCancelKey {
        pub fn new() -> FakeCancelKey {
            FakeCancelKey::default()
        }

        /// Every `set(claimed)` so far, in order.
        pub fn calls(&self) -> Vec<bool> {
            lock(&self.calls).clone()
        }
    }

    impl CancelKey for FakeCancelKey {
        fn set(&self, claimed: bool) {
            lock(&self.calls).push(claimed);
        }
    }
}
