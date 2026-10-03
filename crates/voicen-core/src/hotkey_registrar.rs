//! Two-phase global hotkey registration (spec 004 T067; contracts/core-traits.md).
//!
//! Defined here, implemented by the shell (001, T-006). The settings save
//! `prepare`s the new hotkey, then `commit`s it (releasing the old one) or `abort`s
//! it, so a refused save keeps the old hotkey (FR-05).
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

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
    // STUB: the developer chooses the state.
    _state: (),
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl FakeHotkeyRegistrar {
    pub fn new() -> FakeHotkeyRegistrar {
        todo!("T-003: FakeHotkeyRegistrar::new")
    }

    pub fn fail_prepare(&self, fail: bool) {
        todo!("T-003: FakeHotkeyRegistrar::fail_prepare")
    }

    pub fn calls(&self) -> Vec<RegistrarCall> {
        todo!("T-003: FakeHotkeyRegistrar::calls")
    }

    pub fn active(&self) -> Option<(Hotkey, Mode)> {
        todo!("T-003: FakeHotkeyRegistrar::active")
    }
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl HotkeyRegistrar for FakeHotkeyRegistrar {
    fn prepare(&self, hotkey: Hotkey, mode: Mode) -> Result<Prepared, Unavailable> {
        todo!("T-003: FakeHotkeyRegistrar::prepare")
    }

    fn commit(&self, prepared: Prepared) {
        todo!("T-003: FakeHotkeyRegistrar::commit")
    }

    fn abort(&self, prepared: Prepared) {
        todo!("T-003: FakeHotkeyRegistrar::abort")
    }
}
