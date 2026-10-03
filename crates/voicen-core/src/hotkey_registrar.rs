//! Two-phase global hotkey registration (spec 004 T067; contracts/core-traits.md).
//!
//! Defined here, implemented by the shell (001, T-006). The settings save
//! `prepare`s the new hotkey, then `commit`s it (releasing the old one) or `abort`s
//! it, so a refused save keeps the old hotkey (FR-05).

use crate::settings::hotkey::Hotkey;
use crate::settings::Mode;

/// A registration that is prepared but not yet active. Built by the implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    pub hotkey: Hotkey,
    pub mode: Mode,
    /// Implementation-defined handle (e.g. a Win32 hotkey id).
    pub token: u64,
}

/// The hotkey cannot be registered (taken by another app or reserved by Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

pub trait HotkeyRegistrar: Send + Sync {
    fn prepare(&self, hotkey: Hotkey, mode: Mode) -> Result<Prepared, Unavailable>;
    /// Make `prepared` the active hotkey and release the previous one.
    fn commit(&self, prepared: Prepared);
    /// Drop `prepared`; the previous hotkey stays active.
    fn abort(&self, prepared: Prepared);
}

/// One recorded call on [`FakeHotkeyRegistrar`].
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrarCall {
    Prepare(Hotkey, Mode),
    Commit(Hotkey),
    Abort(Hotkey),
}

#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
struct FakeRegistrarState {
    calls: Vec<RegistrarCall>,
    fail_prepare: bool,
    active: Option<(Hotkey, Mode)>,
    next_token: u64,
}

/// In-memory [`HotkeyRegistrar`] with a call log and an injectable `prepare` failure.
///
/// - Every call is recorded in [`calls`](Self::calls), failed ones included.
/// - [`fail_prepare`](Self::fail_prepare)`(true)`: every later `prepare` returns
///   `Err(Unavailable)` until set back to `false`.
/// - [`active`](Self::active): the committed hotkey and mode (`None` before the
///   first commit); `abort` and a failed `prepare` leave it unchanged.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeHotkeyRegistrar {
    state: std::sync::Mutex<FakeRegistrarState>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeHotkeyRegistrar {
    pub fn new() -> FakeHotkeyRegistrar {
        FakeHotkeyRegistrar::default()
    }

    pub fn fail_prepare(&self, fail: bool) {
        self.lock().fail_prepare = fail;
    }

    pub fn calls(&self) -> Vec<RegistrarCall> {
        self.lock().calls.clone()
    }

    pub fn active(&self) -> Option<(Hotkey, Mode)> {
        self.lock().active
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeRegistrarState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl HotkeyRegistrar for FakeHotkeyRegistrar {
    fn prepare(&self, hotkey: Hotkey, mode: Mode) -> Result<Prepared, Unavailable> {
        let mut state = self.lock();
        state.calls.push(RegistrarCall::Prepare(hotkey, mode));
        if state.fail_prepare {
            return Err(Unavailable);
        }
        state.next_token += 1;
        Ok(Prepared {
            hotkey,
            mode,
            token: state.next_token,
        })
    }

    fn commit(&self, prepared: Prepared) {
        let mut state = self.lock();
        state.calls.push(RegistrarCall::Commit(prepared.hotkey));
        state.active = Some((prepared.hotkey, prepared.mode));
    }

    fn abort(&self, prepared: Prepared) {
        self.lock()
            .calls
            .push(RegistrarCall::Abort(prepared.hotkey));
    }
}
