//! T-150: the shell's Windows audio-device notifications and communications check
//! (`voicen_lib::win::audio_notify`, spec 007 R4, FR-010, FR-012; T070/T071; invariants N1-N4 of
//! docs/tasks/T-150.md). Windows CI only (decision #5): `IMMDeviceEnumerator` exists only there.
//!
//! API pinned for the developer (docs/tasks/T-150.md `## Tests`; the module does not exist yet,
//! so this file fails to compile until it does; that unresolved import is the red):
//!
//! ```text
//! voicen_lib::win::audio_notify::
//!   NotifyFactory                         // unit struct, Send + Sync; implements core's
//!                                         // `DeviceEventsFactory` (open() boxes `AudioNotify::open()`)
//!   AudioNotify                           // implements core's `DeviceEvents`
//!     ::open() -> AudioNotify             // N3: COM guard (MTA, RPC_E_CHANGED_MODE tolerated),
//!                                         // enumerator, client, RegisterEndpointNotificationCallback,
//!                                         // all on the calling thread; any failure -> an AudioNotify
//!                                         // with registered() == false whose next() is always None
//!     ::detached() -> (IMMNotificationClient, AudioNotify)
//!                                         // test seam: the same client type `open()` registers,
//!                                         // wired to the returned receiver, registered NOWHERE
//!                                         // (no COM guard, no enumerator); registered() == false;
//!                                         // dropping the AudioNotify drops the receiver only
//!     ::registered(&self) -> bool         // true only after a successful registration
//!     DeviceEvents::next(&self) -> Option<DeviceEvent>   // non-blocking pull (mpsc try_recv)
//!   ComCheck                              // unit struct, Send + Sync; implements core's
//!                                         // `CommunicationsCheck` (N4)
//!   differs_of(comms: Result<String, i32>, console: Result<String, i32>) -> Result<bool, i32>
//!                                         // pure: both Ok -> Ok(comms != console); comms Err(c)
//!                                         // -> Err(c) (also when console is Err); console Err(c)
//!                                         // with comms Ok -> Err(c)
//! ```
//!
//! The client's N1 table (every callback posts at most one event and returns `S_OK`):
//!
//! ```text
//! OnDefaultDeviceChanged(eRender,  eConsole, _) -> DefaultOutputChanged
//! OnDefaultDeviceChanged(eCapture, eConsole, _) -> DefaultInputChanged
//! OnDefaultDeviceChanged(any other flow or role, out-of-range values included) -> none
//! OnDeviceRemoved(id)                           -> DeviceRemoved(id)
//! OnDeviceStateChanged(id, DEVICE_STATE_ACTIVE) -> none
//! OnDeviceStateChanged(id, DISABLED | NOTPRESENT | UNPLUGGED) -> DeviceRemoved(id)
//! OnDeviceAdded, OnPropertyValueChanged         -> none
//! ```
//!
//! What the runner has is not known (docs/decisions/windows-ci-runner.md has no fact on the audio
//! services). The two OS-facing tests therefore accept both answers the runner can give and print
//! the fact (read it with `-- --nocapture`; it goes into windows-ci-runner.md). Real device
//! switches, per-role callback order and Bluetooth are the owner's manual check (quickstart 3).
//!
//! No verdict is a ceiling on a measured time (T-201): a call that could hang runs under
//! `timing_support::within_hang`, the "no event" window is `holds_for`.
#![cfg(windows)]

mod timing_support;

use std::thread;
use std::time::Duration;

use cpal::traits::HostTrait;
use timing_support::{holds_for, within_hang, HANG};
use voicen_core::call::{CommunicationsCheck, DeviceEvent, DeviceEvents, DeviceEventsFactory};
use voicen_core::platform::DeviceId;
use voicen_lib::win::audio_notify::{differs_of, AudioNotify, ComCheck, NotifyFactory};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eAll, eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole,
    DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT,
    DEVICE_STATE_UNPLUGGED,
};

/// A fake endpoint id (the real ones look like `{0.0.0.00000000}.{GUID}`), once with a non-ASCII
/// character so the UTF-16 to `String` step is exercised.
const IDS: [&str; 2] = [
    "{0.0.1.00000000}.{11111111-2222-3333-4444-555555555555}",
    "{0.0.0.00000000}.{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}-\u{fc}",
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Every event pending now (the pull is non-blocking: the callback posted before it returned).
fn drain(notify: &AudioNotify) -> Vec<DeviceEvent> {
    let mut out = Vec::new();
    while let Some(event) = notify.next() {
        out.push(event);
        assert!(out.len() < 100, "next() never runs dry");
    }
    out
}

fn removed(id: &str) -> DeviceEvent {
    DeviceEvent::DeviceRemoved(DeviceId(id.to_string()))
}

// ---- 1. register and unregister on one thread, no event on the runner (Acceptance 2) ------

#[test]
fn register_and_unregister_on_the_device_thread_with_no_event_on_the_runner() {
    // Acceptance 2 (T070, T071; N3): on a spawned thread (core's device thread) `open()`
    // registers, nothing arrives from a runner where no device changes, and the drop on the
    // same thread unregisters and returns (an unregister waits for a running callback, so a
    // hang here is a callback that blocked; `within_hang` reports it). The factory path (what
    // core calls) gives the same no-event stream. A runner whose Audio services refuse the
    // registration is the printed `registered() == false` branch: then the assertion on
    // `registered()` is red for a runner reason and the orchestrator decides (T-150 approach).
    // Bite: a registration that never happens (registered() false), an event posted on
    // registration (a synthetic DefaultOutputChanged), a Drop that unregisters on another
    // thread or not at all (the drop hangs or the callback outlives its sender), a COM guard
    // dropped before the unregister.
    let Some(((registered, quiet, factory_quiet), _)) = within_hang(|| {
        let thread_id = thread::current().id();
        let notify = AudioNotify::open();
        let registered = notify.registered();
        let quiet = holds_for(Duration::from_millis(300), || notify.next().is_none());
        drop(notify);
        assert_eq!(thread::current().id(), thread_id);

        let boxed: Box<dyn DeviceEvents> = NotifyFactory.open();
        let factory_quiet = holds_for(Duration::from_millis(100), || boxed.next().is_none());
        drop(boxed);
        (registered, quiet, factory_quiet)
    }) else {
        panic!("open / next / drop did not return within {HANG:?}");
    };
    println!(
        "T-150 fact: IMMNotificationClient registration on the runner: registered() = {registered}; \
         no event in 300 ms = {quiet}; no event through the factory in 100 ms = {factory_quiet}"
    );
    assert!(
        registered,
        "the registration did not succeed on this runner"
    );
    assert!(quiet, "an event arrived on a runner with no device change");
    assert!(factory_quiet, "an event arrived through the factory");
}

// ---- 2. the callback map (Acceptance 4) ----------------------------------------------------

#[test]
fn every_callback_maps_to_at_most_one_typed_event() {
    // Acceptance 4 (T071; N1, N2): the client the module registers is called through its COM
    // interface for every flow x role (the named ones plus an out-of-range value of each) and
    // the four DEVICE_STATE values; `next()` yields exactly the N1 table, each callback
    // returns success at once and posts at most one event; after the receiving side is gone a
    // callback still returns success (send error ignored: no panic across the FFI boundary).
    // Bite: eCommunications or eMultimedia mapped to DefaultOutputChanged / DefaultInputChanged
    // (role rows), flow swapped (render <-> capture rows), DEVICE_STATE_ACTIVE mapped to
    // DeviceRemoved or a non-active state mapped to none (state rows), OnDeviceAdded or
    // OnPropertyValueChanged posting (none rows), two events from one callback (the `drain`
    // length), a `send(..).unwrap()` (the after-drop rows panic), a callback that returns an
    // error HRESULT, an id not carried through (DeviceRemoved payload).
    let (client, notify) = AudioNotify::detached();
    let mut wrong = Vec::new();

    let flows = [eRender, eCapture, eAll, EDataFlow(3)];
    let roles = [eConsole, eMultimedia, eCommunications, ERole(3)];
    for id in IDS {
        let w = wide(id);
        let pw = PCWSTR(w.as_ptr());
        for flow in flows {
            for role in roles {
                // SAFETY: `pw` points into `w`, NUL-terminated and alive for the call.
                let got = unsafe { client.OnDefaultDeviceChanged(flow, role, pw) };
                let want: Vec<DeviceEvent> = if flow == eRender && role == eConsole {
                    vec![DeviceEvent::DefaultOutputChanged]
                } else if flow == eCapture && role == eConsole {
                    vec![DeviceEvent::DefaultInputChanged]
                } else {
                    Vec::new()
                };
                let events = drain(&notify);
                if got.is_err() || events != want {
                    wrong.push(format!(
                        "OnDefaultDeviceChanged({flow:?}, {role:?}): {got:?}, events {events:?}, \
                         expected Ok and {want:?}"
                    ));
                }
            }
        }

        // SAFETY: as above.
        let got = unsafe { client.OnDeviceRemoved(pw) };
        let events = drain(&notify);
        if got.is_err() || events != vec![removed(id)] {
            wrong.push(format!(
                "OnDeviceRemoved({id}): {got:?}, events {events:?}, expected Ok and [{:?}]",
                removed(id)
            ));
        }

        let states: [(DEVICE_STATE, bool); 4] = [
            (DEVICE_STATE_ACTIVE, false),
            (DEVICE_STATE_DISABLED, true),
            (DEVICE_STATE_NOTPRESENT, true),
            (DEVICE_STATE_UNPLUGGED, true),
        ];
        for (state, gone) in states {
            // SAFETY: as above.
            let got = unsafe { client.OnDeviceStateChanged(pw, state) };
            let want = if gone { vec![removed(id)] } else { Vec::new() };
            let events = drain(&notify);
            if got.is_err() || events != want {
                wrong.push(format!(
                    "OnDeviceStateChanged({id}, {state:?}): {got:?}, events {events:?}, \
                     expected Ok and {want:?}"
                ));
            }
        }

        // SAFETY: as above.
        let got = unsafe { client.OnDeviceAdded(pw) };
        let events = drain(&notify);
        if got.is_err() || !events.is_empty() {
            wrong.push(format!("OnDeviceAdded({id}): {got:?}, events {events:?}"));
        }
        let key = PROPERTYKEY {
            fmtid: GUID::zeroed(),
            pid: 0,
        };
        // SAFETY: as above; `key` is passed by value.
        let got = unsafe { client.OnPropertyValueChanged(pw, key) };
        let events = drain(&notify);
        if got.is_err() || !events.is_empty() {
            wrong.push(format!(
                "OnPropertyValueChanged({id}): {got:?}, events {events:?}"
            ));
        }
    }

    // The receiving side gone (the stream dropped while the registration is still live): every
    // callback must still return success and must not panic.
    drop(notify);
    let w = wide(IDS[0]);
    let pw = PCWSTR(w.as_ptr());
    // SAFETY: as above.
    unsafe {
        let results = [
            (
                "OnDefaultDeviceChanged",
                client.OnDefaultDeviceChanged(eRender, eConsole, pw),
            ),
            ("OnDeviceRemoved", client.OnDeviceRemoved(pw)),
            (
                "OnDeviceStateChanged",
                client.OnDeviceStateChanged(pw, DEVICE_STATE_UNPLUGGED),
            ),
        ];
        for (name, got) in results {
            if got.is_err() {
                wrong.push(format!("{name} after the receiver was dropped: {got:?}"));
            }
        }
    }

    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// ---- 3. the communications check on the runner (Acceptance 3, the failure branch) ----------

#[test]
fn the_communications_check_returns_a_value_or_none_without_error() {
    // Acceptance 3 (T070; N4), the failure branch: on a runner with no render endpoint the
    // check answers `Err(code)` (core: none), with endpoints `Ok(bool)`; either way it returns
    // (no panic, no hang: `within_hang` reports one), and an `Err` carries a real, non-zero OS
    // code. The branch taken is printed. Bite: an `unwrap` / `expect` on GetDefaultAudioEndpoint
    // (the panic fails the test on a device-less runner), an `Err(0)` for a failed call, a
    // check that blocks on a device activation. A lookup error swallowed at its origin
    // (`GetDefaultAudioEndpoint` failing turned into an empty id, so `Ok(false)`) is caught by
    // the oracle below on a runner without a render endpoint: cpal's own lookup of the eConsole
    // render default (`GetDefaultAudioEndpoint(eRender, eConsole)`, the same call the check
    // makes) finds none, so the check must answer `Err`. With a render endpoint that branch is
    // not reached; a lost error in `differs_of` is pinned by the decision table below.
    let Some((answer, _)) = within_hang(|| ComCheck.differs()) else {
        panic!("the communications check did not return within {HANG:?}");
    };
    let Some((has_render_default, _)) =
        within_hang(|| cpal::default_host().default_output_device().is_some())
    else {
        panic!("cpal's default output lookup did not return within {HANG:?}");
    };
    println!("T-150 fact: eConsole render default present (cpal) = {has_render_default}");
    if !has_render_default {
        assert!(
            answer.is_err(),
            "no eConsole render default on this runner, yet the check answered {answer:?}: \
             a failed lookup was turned into a verdict"
        );
    }
    match answer {
        Ok(differs) => {
            println!("T-150 fact: communications check: Ok({differs}) (the runner has a render endpoint)");
        }
        Err(code) => {
            assert_ne!(code, 0, "a failed check carries no OS code");
            println!("T-150 fact: communications check: Err({code} = {code:#x}) (core: none)");
        }
    }
}

// ---- 4. the decision table (Acceptance 3) --------------------------------------------------

#[test]
fn the_communications_decision_table() {
    // Acceptance 3 (N4): the pure decision over the two device ids: same -> Ok(false),
    // different -> Ok(true), the communications lookup failing with ERROR_NOT_FOUND (1168) ->
    // Err(1168), the console lookup failing -> Err, both failing -> the communications code.
    // No error is turned into a verdict (core treats Err as none, but the shell reports it).
    // Bite: `Ok(false)` on any Err (the swallowed error), `Ok(true)` on an Err, the two ids
    // compared by length or prefix, the equal case inverted, the console code reported when
    // both fail.
    let a = || Ok::<String, i32>(IDS[0].to_string());
    let b = || Ok::<String, i32>(IDS[1].to_string());
    assert_eq!(differs_of(a(), a()), Ok(false), "same device");
    assert_eq!(differs_of(a(), b()), Ok(true), "different devices");
    assert_eq!(differs_of(b(), a()), Ok(true), "different devices, swapped");
    assert_eq!(
        differs_of(Err(1168), a()),
        Err(1168),
        "no communications device"
    );
    assert_eq!(differs_of(a(), Err(5)), Err(5), "no console device");
    assert_eq!(
        differs_of(Err(1168), Err(5)),
        Err(1168),
        "both lookups failed"
    );
    assert_eq!(
        differs_of(Err(-2_147_023_728), Err(-2_147_024_891)),
        Err(-2_147_023_728),
        "an HRESULT code passes through unchanged"
    );
}
