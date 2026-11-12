//! The call mode's audio-device signals (spec 007 R4, FR-010, FR-012; T-150, invariants
//! N1-N4 of docs/tasks/T-150.md): [`NotifyFactory`] / [`AudioNotify`], core's
//! `DeviceEventsFactory` / `DeviceEvents` over an `IMMNotificationClient`, and [`ComCheck`],
//! core's `CommunicationsCheck` over `IMMDeviceEnumerator::GetDefaultAudioEndpoint`.
//!
//! The shell decides nothing about devices (N1): each callback maps through the pure
//! [`event_of`] onto at most one `DeviceEvent` and only posts it to the stream's channel
//! (an unbounded `mpsc` send, its error ignored); whether an id concerns a track is core's
//! route. A callback never blocks, takes no lock, never (un)registers or releases the
//! enumerator and never unwinds across the FFI boundary (N2): `UnregisterEndpointNotificationCallback`
//! waits for a running callback, so a callback that waited on the device thread would
//! deadlock its drop.
//!
//! One thread (N3): [`AudioNotify::open`] initialises COM (MTA; `RPC_E_CHANGED_MODE`
//! tolerated, no `CoUninitialize` then), creates the enumerator and registers the client on
//! the calling thread, which is core's device thread; `Drop` unregisters on the same thread,
//! releases the enumerator, then uninitialises COM. A failed step leaves a stream with
//! `registered() == false` whose `next()` is always `None` and that holds nothing to
//! unregister (the stream-level loss map in `track_capture` still reports a lost track).
//!
//! The communications check (N4) runs on a helper thread bounded by [`OPEN_BUDGET`] (it is
//! called under the session lock); it compares the endpoint ids of the eCommunications and
//! eConsole render defaults through the pure [`differs_of`], opens or activates no device
//! and keeps no COM object past the call. `Err` carries only the OS code (`os_code`: the
//! audio "not found" is `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`, Win32 1168, not the
//! `E_NOTFOUND` constant of the windows crate's HtmlHelp module). Nothing here logs; ids
//! never leave the core ports.

use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;

use voicen_core::call::{CommunicationsCheck, DeviceEvent, DeviceEvents, DeviceEventsFactory};
use voicen_core::platform::DeviceId;
use windows::core::{implement, Result as WinResult, HRESULT, PCWSTR};
use windows::Win32::Foundation::{ERROR_TIMEOUT, E_UNEXPECTED, PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

use super::capture::OPEN_BUDGET;
use super::os_code;

/// Core's [`DeviceEventsFactory`]: each `open` is one [`AudioNotify::open`] on the calling
/// (device) thread.
pub struct NotifyFactory;

impl DeviceEventsFactory for NotifyFactory {
    fn open(&self) -> Box<dyn DeviceEvents> {
        Box::new(AudioNotify::open())
    }
}

/// One capture lifetime's device-event stream (core's [`DeviceEvents`]).
pub struct AudioNotify {
    /// The live registration; `None` when it failed or for a detached client.
    registration: Option<Registration>,
    receiver: Receiver<DeviceEvent>,
    /// Dropped last (field order and `Drop` below): after the unregister and the
    /// enumerator's release.
    _com: Option<ComGuard>,
}

struct Registration {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

// SAFETY: the COM objects and the COM guard are created by `open` and released by `Drop` on
// the same thread: core creates the stream on its device thread, polls it there and drops it
// there (`DeviceEvents` docs, N3); between the two it is only moved, never used from another
// thread. `next` touches only the `Receiver`, which is `Send`.
unsafe impl Send for AudioNotify {}

impl AudioNotify {
    /// Registers a notification client on the calling thread (N3). Any failure (COM init,
    /// the enumerator, the registration) yields a stream with `registered() == false`
    /// whose `next()` is always `None`.
    pub fn open() -> AudioNotify {
        let (sender, receiver) = mpsc::channel();
        let Some(com) = ComGuard::init() else {
            return AudioNotify {
                registration: None,
                receiver,
                _com: None,
            };
        };
        let client: IMMNotificationClient = NotifyClient { sender }.into();
        // SAFETY: COM is initialised on this thread (`com`, alive until after the
        // unregister); the CLSID and the interface type match.
        let enumerator: WinResult<IMMDeviceEnumerator> =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
        let registration = enumerator.ok().and_then(|enumerator| {
            // SAFETY: `client` is a valid IMMNotificationClient that the registration holds
            // a reference to until it is unregistered in `Drop`, on this thread.
            unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }
                .ok()
                .map(|()| Registration { enumerator, client })
        });
        AudioNotify {
            registration,
            receiver,
            _com: Some(com),
        }
    }

    /// Test seam: the client type `open` registers, wired to the returned stream and
    /// registered nowhere (no COM guard, no enumerator; `registered() == false`).
    pub fn detached() -> (IMMNotificationClient, AudioNotify) {
        let (sender, receiver) = mpsc::channel();
        let client: IMMNotificationClient = NotifyClient { sender }.into();
        let notify = AudioNotify {
            registration: None,
            receiver,
            _com: None,
        };
        (client, notify)
    }

    /// True only after a successful `RegisterEndpointNotificationCallback`.
    pub fn registered(&self) -> bool {
        self.registration.is_some()
    }
}

impl DeviceEvents for AudioNotify {
    fn next(&self) -> Option<DeviceEvent> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for AudioNotify {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            // SAFETY: the same thread and COM apartment that registered (N3); synchronous:
            // returns after a running callback, which never waits on this thread (N2).
            let _ = unsafe {
                registration
                    .enumerator
                    .UnregisterEndpointNotificationCallback(&registration.client)
            };
            drop(registration);
        }
        // `_com` drops after this body: COM is uninitialised last.
    }
}

/// COM on the current thread (MTA): `CoUninitialize` on drop only after a successful
/// `CoInitializeEx`; `RPC_E_CHANGED_MODE` (the thread is already an STA) is usable as is.
/// Not `Send`: it must drop on the thread that made it.
struct ComGuard {
    owns: bool,
    _thread_bound: PhantomData<*mut ()>,
}

impl ComGuard {
    fn init() -> Option<ComGuard> {
        // SAFETY: no reserved pointer; balanced by `Drop` when it succeeded.
        let result: HRESULT = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() || result == RPC_E_CHANGED_MODE {
            Some(ComGuard {
                owns: result.is_ok(),
                _thread_bound: PhantomData,
            })
        } else {
            None
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: balances this thread's successful `CoInitializeEx`.
            unsafe { CoUninitialize() };
        }
    }
}

/// One `IMMNotificationClient` callback, reduced to what [`event_of`] decides on.
enum Notice {
    DefaultChanged(EDataFlow, ERole),
    Removed(String),
    StateChanged(String, DEVICE_STATE),
    Ignored,
}

/// The N1 table: the eConsole default of render / capture changed -> `DefaultOutputChanged`
/// / `DefaultInputChanged`; a removed device, or one whose new state is not
/// `DEVICE_STATE_ACTIVE` -> `DeviceRemoved(id)`; everything else -> none.
fn event_of(notice: Notice) -> Option<DeviceEvent> {
    match notice {
        Notice::DefaultChanged(flow, role) if role == eConsole && flow == eRender => {
            Some(DeviceEvent::DefaultOutputChanged)
        }
        Notice::DefaultChanged(flow, role) if role == eConsole && flow == eCapture => {
            Some(DeviceEvent::DefaultInputChanged)
        }
        Notice::DefaultChanged(..) | Notice::Ignored => None,
        Notice::Removed(id) => Some(DeviceEvent::DeviceRemoved(DeviceId(id))),
        Notice::StateChanged(_, state) if state == DEVICE_STATE_ACTIVE => None,
        Notice::StateChanged(id, _) => Some(DeviceEvent::DeviceRemoved(DeviceId(id))),
    }
}

/// The registered client: holds only the stream's `Sender` (N1, N2).
#[implement(IMMNotificationClient)]
struct NotifyClient {
    sender: Sender<DeviceEvent>,
}

impl NotifyClient {
    /// Maps and posts; never blocks (unbounded channel), ignores a gone receiver and
    /// never lets a panic unwind into COM. Always success.
    fn post(&self, notice: impl FnOnce() -> Notice) -> WinResult<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(event) = event_of(notice()) {
                let _ = self.sender.send(event);
            }
        }));
        Ok(())
    }
}

/// The endpoint id a callback passes (a null pointer -> empty; invalid UTF-16 replaced).
fn id_of(id: &PCWSTR) -> String {
    if id.is_null() {
        return String::new();
    }
    // SAFETY: a non-null id from COM (or the caller) is a NUL-terminated UTF-16 string,
    // valid for the duration of the callback.
    String::from_utf16_lossy(unsafe { id.as_wide() })
}

impl IMMNotificationClient_Impl for NotifyClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        pwstrdeviceid: &PCWSTR,
        dwnewstate: DEVICE_STATE,
    ) -> WinResult<()> {
        self.post(|| Notice::StateChanged(id_of(pwstrdeviceid), dwnewstate))
    }

    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> WinResult<()> {
        self.post(|| Notice::Ignored)
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &PCWSTR) -> WinResult<()> {
        self.post(|| Notice::Removed(id_of(pwstrdeviceid)))
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _pwstrdefaultdeviceid: &PCWSTR,
    ) -> WinResult<()> {
        self.post(|| Notice::DefaultChanged(flow, role))
    }

    fn OnPropertyValueChanged(&self, _pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        self.post(|| Notice::Ignored)
    }
}

/// Core's [`CommunicationsCheck`] (N4).
pub struct ComCheck;

impl CommunicationsCheck for ComCheck {
    /// Bounded by [`OPEN_BUDGET`] on a helper thread: a timeout -> `Err(ERROR_TIMEOUT)`, a
    /// helper that could not start or ended without an answer -> `Err(E_UNEXPECTED)`.
    fn differs(&self) -> Result<bool, i32> {
        let (answer, result) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("comms-check".into())
            .spawn(move || {
                let _ = answer.send(check());
            });
        if spawned.is_err() {
            return Err(E_UNEXPECTED.0);
        }
        match result.recv_timeout(OPEN_BUDGET) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => Err(ERROR_TIMEOUT.0 as i32),
            Err(RecvTimeoutError::Disconnected) => Err(E_UNEXPECTED.0),
        }
    }
}

/// The check on the helper thread: COM, the enumerator, the two render defaults' ids.
fn check() -> Result<bool, i32> {
    let Some(_com) = ComGuard::init() else {
        return Err(E_UNEXPECTED.0);
    };
    // SAFETY: COM is initialised on this thread for the whole call (`_com`, dropped last).
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|err| os_code(&err))?;
    let comms = default_id(&enumerator, eCommunications);
    let console = default_id(&enumerator, eConsole);
    drop(enumerator);
    differs_of(comms, console)
}

/// The endpoint id of the render default for `role`; no device is opened or activated.
fn default_id(enumerator: &IMMDeviceEnumerator, role: ERole) -> Result<String, i32> {
    // SAFETY: a live enumerator on a COM-initialised thread.
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, role) }
        .map_err(|err| os_code(&err))?;
    // SAFETY: as above; the returned string is ours to free with CoTaskMemFree.
    let id = unsafe { device.GetId() }.map_err(|err| os_code(&err))?;
    if id.is_null() {
        return Err(E_UNEXPECTED.0);
    }
    // SAFETY: a non-null id from GetId is NUL-terminated UTF-16; freed exactly once, after
    // the copy.
    let text = String::from_utf16_lossy(unsafe { id.as_wide() });
    unsafe { CoTaskMemFree(Some(id.0 as *const core::ffi::c_void)) };
    Ok(text)
}

/// The N4 decision: both ids known -> whether they differ; the communications lookup failed
/// -> its code (also when the console lookup failed too); only the console one -> its code.
pub fn differs_of(comms: Result<String, i32>, console: Result<String, i32>) -> Result<bool, i32> {
    let comms = comms?;
    let console = console?;
    Ok(comms != console)
}
