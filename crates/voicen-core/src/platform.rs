//! Platform ports the pipeline delivers through (spec 001 T010,
//! contracts/core-traits.md "Platform traits", T-001).
//!
//! The Windows implementations live in `src-tauri` (T-006: clipboard and paste;
//! T-007: the file-system audio store). Errors carry no OS text (P-009). The
//! public fakes (feature `test-fakes`, decision #23 N4) sit next to the traits.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use crate::audio::AudioBuffer;

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

#[cfg(any(test, feature = "test-fakes"))]
pub use fakes::*;

#[cfg(any(test, feature = "test-fakes"))]
mod fakes {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::Duration;

    use super::{
        AudioBuffer, Clipboard, ClipboardError, PasteError, Paster, PendingId, StartWindow,
        TempAudioStore, WindowRef,
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
}
