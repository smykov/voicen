//! Start with Windows (req FR-19; spec 004 FR-019, research R-3 step 3, R-5;
//! contracts/core-traits.md#autostart; T-014).
//!
//! Defined here, implemented by the shell (`src-tauri/src/autostart.rs`, the HKCU
//! Run value). Only `SettingsService` calls it: the save step (applied only when
//! `start_with_windows` changes, undone on a later failure) and
//! `reconcile_autostart` at start.

/// An autostart failure: an OS error code. Never carries the exe path (it holds the
/// Windows user name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartError {
    pub os_code: i32,
}

/// The per-user logon start entry.
pub trait Autostart: Send + Sync {
    /// The entry exists, whatever its data.
    fn is_enabled(&self) -> Result<bool, AutostartError>;
    /// `true`: (re)write the entry with the current command (idempotent, fixes a
    /// stale path). `false`: remove it; an absent entry is `Ok`.
    fn set(&self, enabled: bool) -> Result<(), AutostartError>;
}

/// What `SettingsService::reconcile_autostart` did (R-11; T-008 logs `as_str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    None,
    Written,
    Removed,
    Failed,
}

impl ReconcileAction {
    /// `none` | `written` | `removed` | `failed`.
    pub fn as_str(self) -> &'static str {
        // RED STUB (T-014 test-writer): the developer replaces this.
        ""
    }
}

/// One recorded call on [`FakeAutostart`].
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartCall {
    IsEnabled,
    Set(bool),
}

#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
struct FakeAutostartState {
    enabled: bool,
    calls: Vec<AutostartCall>,
    fail_set_true: Option<AutostartError>,
    fail_set_false: Option<AutostartError>,
    fail_is_enabled: Option<AutostartError>,
}

/// In-memory [`Autostart`] with a call log and injectable failures.
///
/// - [`new`](Self::new) starts off, [`enabled`](Self::enabled) starts on.
/// - Every trait call is recorded in [`calls`](Self::calls), failed ones included.
/// - [`fail_set`](Self::fail_set)`(value, err)` makes every later `set(value)` fail
///   (the other value still succeeds), [`fail_is_enabled`](Self::fail_is_enabled)
///   every later `is_enabled`, until [`clear_failures`](Self::clear_failures). A
///   failed `set` leaves the state unchanged.
/// - [`is_on`](Self::is_on) inspects the state without being recorded.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeAutostart {
    state: std::sync::Mutex<FakeAutostartState>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeAutostart {
    /// No entry.
    pub fn new() -> FakeAutostart {
        FakeAutostart::default()
    }

    /// An entry already present.
    pub fn enabled() -> FakeAutostart {
        let fake = FakeAutostart::default();
        fake.lock().enabled = true;
        fake
    }

    /// The entry exists now (not recorded as a call).
    pub fn is_on(&self) -> bool {
        self.lock().enabled
    }

    /// Every trait call so far, in order.
    pub fn calls(&self) -> Vec<AutostartCall> {
        self.lock().calls.clone()
    }

    /// Every later `set(value)` fails with `error`.
    pub fn fail_set(&self, value: bool, error: AutostartError) {
        let mut state = self.lock();
        if value {
            state.fail_set_true = Some(error);
        } else {
            state.fail_set_false = Some(error);
        }
    }

    /// Every later `is_enabled` fails with `error`.
    pub fn fail_is_enabled(&self, error: AutostartError) {
        self.lock().fail_is_enabled = Some(error);
    }

    pub fn clear_failures(&self) {
        let mut state = self.lock();
        state.fail_set_true = None;
        state.fail_set_false = None;
        state.fail_is_enabled = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeAutostartState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl Autostart for FakeAutostart {
    fn is_enabled(&self) -> Result<bool, AutostartError> {
        let mut state = self.lock();
        state.calls.push(AutostartCall::IsEnabled);
        match state.fail_is_enabled {
            Some(error) => Err(error),
            None => Ok(state.enabled),
        }
    }

    fn set(&self, enabled: bool) -> Result<(), AutostartError> {
        let mut state = self.lock();
        state.calls.push(AutostartCall::Set(enabled));
        let failure = if enabled {
            state.fail_set_true
        } else {
            state.fail_set_false
        };
        match failure {
            Some(error) => Err(error),
            None => {
                state.enabled = enabled;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_action_as_str() {
        // The R-11 log words. Bite: any arm of as_str changed or two sharing a word.
        let table = [
            (ReconcileAction::None, "none"),
            (ReconcileAction::Written, "written"),
            (ReconcileAction::Removed, "removed"),
            (ReconcileAction::Failed, "failed"),
        ];
        for (action, word) in table {
            assert_eq!(action.as_str(), word, "{action:?}");
        }
    }

    #[test]
    fn autostart_error_carries_only_the_os_code() {
        // Bite: a field added to AutostartError (e.g. the exe path, which holds the
        // Windows user name).
        let err = AutostartError { os_code: 5 };
        assert_eq!(format!("{err:?}"), "AutostartError { os_code: 5 }");
    }
}
