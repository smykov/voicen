//! T-128: the launch routes into call recording (spec 007 FR-001, FR-002, FR-043; T028, T029;
//! decisions #197, #277, #278) over the shell's app wiring (`build_app`, mock runtime) with the
//! call part started by `voicen_lib::call::start_calls` over fakes. Windows CI only (decision #5).
//! The API these tests pin is in `call_shell_support/mod.rs`; `voicen_lib::call` does not exist
//! yet, so this file fails to compile until it does (that unresolved import is the red).
//!
//! The invariants (docs/tasks/T-128.md analysis): (I-a) every shell start or stop is one
//! `CallRequest` posted to the one call worker, which alone calls the session; (I-c) an unknown
//! `--call` value writes exactly one `call_switch_refused reason=unknown_value` and starts
//! nothing, a launch without the switch never starts call mode; (I-d) every refusal and every
//! `CallNotice` reaches the one shared notifier through core's mapping, the log lines staying
//! core's; (I-e) while `next_deadline()` is `Some`, the `call-tick` thread calls `tick` at it.
//!
//! No verdict is a ceiling on a measured time (F-019, F-021, T-201): every wait is a positive
//! wait on an outcome; the bounded negatives only watch for something that must not come. The 10 s
//! reminder and the 4 h limit are core's `call_session` tests on a fake clock.
#![cfg(windows)]

mod call_shell_support;
mod timing_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tauri::Manager;

use call_shell_support::{
    argv, eventually, failing_once_opener, holds_for_a_while, plain_opener, CallRig, Refusing,
    WavSource, CWD, EXE,
};
use voicen_core::call::events::refused_notice;
use voicen_core::call::{
    CallNotice, CallSource, OpenError, RefusedPart, RefusedReason, StopReason,
};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::platform::NotifyCall;
use voicen_core::settings::gate::LaunchArgs;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_lib::call::{self, CallRequest};
use voicen_lib::tray;

/// Each binary serialises its tests: the tray icons and their windows are per session.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn offer(id: voicen_core::i18n::MessageId, params: Vec<(&'static str, String)>) -> NotifyCall {
    NotifyCall {
        id,
        params,
        retry: None,
    }
}

#[test]
fn a_second_launch_with_call_record_posts_one_request_and_logs_call_requested_source_switch() {
    // AC1 (T028, T029): the single-instance callback maps `--call record` to one
    // `CallRequest::Record(Switch)`; the call worker enters `toggle_record(On, Switch)`, which
    // logs `call_requested source=switch` once and starts the call. The callback itself posts
    // nothing to the settings opener and returns no receipt. Bite: `on_second_instance` still
    // mapping `StartCallRecord` to `None` (no line, no call); a request posted twice (two
    // lines); `Source::Tray` instead of `Switch` (no `source=switch` line).
    let _serial = serial();
    let (rig, _source) = CallRig::with_wav();

    let posted = tray::on_second_instance(
        rig.handle(),
        argv(&[EXE, "--call", "record"]),
        CWD.to_string(),
    );

    assert!(
        posted.is_none(),
        "a --call record launch posted a settings request"
    );
    assert!(
        eventually(|| rig.recording()),
        "the call never started; log lines: {:?}",
        rig.lines(&["call_requested"])
    );
    assert!(
        holds_for_a_while(|| rig.count(&["call_requested", "source=switch"]) == 1),
        "expected exactly one `call_requested source=switch`, got {:?}",
        rig.lines(&["call_requested"])
    );
    assert_eq!(
        rig.count(&["call_requested"]),
        1,
        "any other source logged a request"
    );
    assert!(
        rig.notifier.calls().is_empty(),
        "a started call offered a notice: {:?}",
        rig.notifier.calls()
    );
    if let Some(session) = rig.session() {
        let _ = session.stop(StopReason::User);
    }
}

#[test]
fn a_second_launch_refused_on_the_runner_keeps_the_app_running() {
    // AC2 failure branch (T028): with the real `CpalTrackSource` the start is refused at the
    // first part, the system loopback, when the host has no render endpoint, and the app runs
    // on: `call_refused part=system reason=no_device` is logged once, the session is not
    // recording, a later request is still served and the tray still answers. The premise (the
    // runner has no render endpoint, docs/decisions/windows-ci-runner.md) was never observed:
    // this test asserts it, so a runner that has one fails here with the premise named (P-016,
    // run on wip/T-128 first). Bite: a refusal that is swallowed (no `call_refused` line in
    // the app's log, or none at the worker), a refusal that exits or panics the worker (the
    // second request is never served), the request routed to a source other than the real one.
    let _serial = serial();
    let has_output = timing_support::within_hang(|| {
        use cpal::traits::HostTrait;
        cpal::default_host().default_output_device().is_some()
    })
    .map(|(has, _)| has)
    .expect("the default-device query did not return");
    assert!(
        !has_output,
        "runner premise (docs/decisions/windows-ci-runner.md:204): the host has a default render \
         endpoint, so the system loopback opens; AC2 and the FR-043 smoke premise need an owner decision"
    );
    let rig = CallRig::started(
        Arc::new(voicen_lib::win::track_capture::CpalTrackSource::new()),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );

    let posted = tray::on_second_instance(
        rig.handle(),
        argv(&[EXE, "--call", "record"]),
        CWD.to_string(),
    );

    assert!(posted.is_none());
    assert!(
        eventually(|| rig.count(&["call_refused", "part=system", "reason=no_device"]) == 1),
        "no `call_refused part=system reason=no_device`; call lines: {:?}",
        rig.lines(&["call_requested"])
    );
    assert_eq!(rig.count(&["call_requested", "source=switch"]), 1);
    assert!(!rig.recording(), "a refused start left a call recording");
    // The app keeps running and the worker still serves requests.
    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));
    assert!(
        eventually(|| rig.count(&["call_requested", "source=tray"]) == 1),
        "the worker did not serve a request after a refusal"
    );
    assert!(tray::applied(rig.handle()).is_some(), "the tray is gone");
}

#[test]
fn a_first_launch_with_the_switch_applies_it_once_after_the_tray_exists() {
    // AC3 (T028): `apply_launch` with `--call record` (run() calls it after `start_calls`,
    // which is after the tray is built) posts one `Record(Switch)`: one
    // `call_requested source=switch` line, the call records, and the tray, which exists, shows
    // it. Bite: `apply_launch` never posting (no line), posting twice (two lines), applying
    // before the call part exists (a no-op: here the part is started first, so the line is
    // the proof).
    let _serial = serial();
    let (rig, _source) = CallRig::with_wav();
    assert!(
        tray::applied(rig.handle()).is_some(),
        "premise: the tray is built"
    );

    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE, "--call", "record"]));

    assert!(
        eventually(|| rig.recording()),
        "the switch did not start a call"
    );
    assert!(
        holds_for_a_while(|| rig.count(&["call_requested", "source=switch"]) == 1),
        "expected exactly one `call_requested source=switch`, got {:?}",
        rig.lines(&["call_requested"])
    );
    assert!(
        eventually(|| tray::applied(rig.handle())
            .is_some_and(|a| { a.state == voicen_core::recording::TrayState::CallRecording })),
        "the tray never showed the call"
    );
    if let Some(session) = rig.session() {
        let _ = session.stop(StopReason::User);
    }
}

#[test]
fn a_launch_without_the_switch_never_starts_call_mode() {
    // AC3 (T028): no `--call` (plain and `--autostart`, first launch, and a second launch that
    // fronts settings) starts nothing: no request line, no call, no track opened, no
    // `call_switch_refused`. Bite: `apply_launch` starting a call whenever it is called, or
    // for `autostart`, or `start_calls` starting one itself.
    let _serial = serial();
    let (rig, source) = CallRig::with_wav();

    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE]));
    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE, "--autostart"]));

    assert!(
        holds_for_a_while(|| !rig.recording()
            && rig.count(&["call_requested"]) == 0
            && rig.count(&["call_switch_refused"]) == 0
            && source.opens() == 0),
        "call mode started or a switch line was written: {:?} {:?}",
        rig.lines(&["call_requested"]),
        rig.lines(&["call_switch_refused"])
    );
}

#[test]
fn the_call_part_is_started_by_call_start_calls_not_by_build_app() {
    // AC5 (T028): `build_app` alone manages no `CallSession` and no `CallStore`; a request on
    // such an app is a no-op (no panic, no line); `call::start_calls` manages both. Bite:
    // `build_app` / `wire` building the call part (the first assertion), `request` panicking
    // or logging without a part, `start_calls` not managing the session.
    let _serial = serial();
    let rig = CallRig::bare();
    assert!(
        rig.session().is_none(),
        "build_app alone manages a CallSession"
    );
    assert!(
        rig.handle()
            .try_state::<Arc<voicen_core::call::CallStore>>()
            .is_none(),
        "build_app alone manages a CallStore"
    );

    call::request(rig.handle(), CallRequest::Record(CallSource::Switch));
    call::request(rig.handle(), CallRequest::RefuseSwitch);
    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE, "--call", "record"]));

    assert!(
        holds_for_a_while(
            || rig.count(&["call_requested"]) == 0 && rig.count(&["call_switch_refused"]) == 0
        ),
        "a request on an app without a call part wrote a line"
    );

    rig.start(
        Arc::new(WavSource::default()),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );
    assert!(
        rig.session().is_some(),
        "start_calls did not manage a CallSession"
    );
    assert!(
        rig.handle()
            .try_state::<Arc<voicen_core::call::CallStore>>()
            .is_some(),
        "start_calls did not manage a CallStore"
    );
}

#[test]
fn a_refusal_reaches_the_notifier_once() {
    // AC6 (T029; I-d): a refused start (the system track has no device) offers the one shared
    // notifier exactly one message, the id of core's `refused_notice(System, NoDevice)`, no
    // params, `retry: None`; the log keeps core's two lines (`call_requested`, `call_refused`)
    // once each. Bite: the refusal never mapped (no offer: `Toggled::Refused` dropped by the
    // worker), a shell-side mapping with another id, a second offer, `retry: Some`.
    let _serial = serial();
    let rig = CallRig::started(
        Arc::new(Refusing(OpenError::NoDevice)),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );

    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));

    let want = offer(
        refused_notice(RefusedPart::System, RefusedReason::NoDevice),
        Vec::new(),
    );
    assert!(
        eventually(|| !rig.notifier.calls().is_empty()),
        "no offer reached the notifier"
    );
    assert!(
        holds_for_a_while(|| rig.notifier.calls() == vec![want.clone()]),
        "expected exactly {want:?}, got {:?}",
        rig.notifier.calls()
    );
    assert_eq!(rig.count(&["call_requested", "source=tray"]), 1);
    assert_eq!(
        rig.count(&["call_refused", "part=system", "reason=no_device"]),
        1
    );
}

#[test]
fn a_stop_reaches_the_notifier_once() {
    // AC6 (T029; I-d): a user stop offers exactly one message, `CallNotice::CallSavedTranscribing`
    // through core's `message()`, `retry: None`; the log has one `call_stopped reason=user`.
    // Bite: the stop notice never routed to the shared notifier (`ShellCallShell::notice`
    // dropping it), routed twice, an id other than core's mapping.
    let _serial = serial();
    let (rig, source) = CallRig::with_wav();
    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));
    assert!(eventually(|| rig.recording()), "the call did not start");
    source.pump();

    call::request(rig.handle(), CallRequest::Stop(CallSource::Tray));

    assert!(eventually(|| !rig.recording()), "the call did not stop");
    let (id, params) = CallNotice::CallSavedTranscribing.message();
    let want = offer(id, params);
    assert!(
        eventually(|| !rig.notifier.calls().is_empty()),
        "no offer reached the notifier"
    );
    assert!(
        holds_for_a_while(|| rig.notifier.calls() == vec![want.clone()]),
        "expected exactly {want:?}, got {:?}",
        rig.notifier.calls()
    );
    assert_eq!(rig.count(&["call_stopped", "reason=user"]), 1);
}

#[test]
fn an_unknown_call_value_writes_call_switch_refused_on_first_and_second_launch() {
    // AC10 (#197, T-121 r1; I-c): `--call bogus` (and a `--call` with no value) writes exactly one
    // `call_switch_refused reason=unknown_value` per launch, on the second launch (the callback)
    // and on the first (`apply_launch`), and starts nothing: no request line, no call, no track
    // opened, nothing for the notifier. Bite: `RefuseSwitch` never posted or its line never
    // written (count short), written twice, written by the callback thread and again by the
    // worker, `Unknown` mapped to a start (`call_requested`), `apply_launch` ignoring `Unknown`.
    let _serial = serial();
    let (rig, source) = CallRig::with_wav();

    let posted = tray::on_second_instance(
        rig.handle(),
        argv(&[EXE, "--call", "bogus"]),
        CWD.to_string(),
    );
    assert!(posted.is_none());
    assert!(
        eventually(|| rig.count(&["call_switch_refused", "reason=unknown_value"]) == 1),
        "second launch: no `call_switch_refused reason=unknown_value`"
    );

    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE, "--call", "bogus"]));
    assert!(
        eventually(|| rig.count(&["call_switch_refused", "reason=unknown_value"]) == 2),
        "first launch: no second `call_switch_refused` line"
    );

    call::apply_launch(rig.handle(), &LaunchArgs::parse([EXE, "--call"]));
    assert!(
        eventually(|| rig.count(&["call_switch_refused", "reason=unknown_value"]) == 3),
        "first launch with no value: no third line"
    );

    assert!(
        holds_for_a_while(|| rig.count(&["call_switch_refused"]) == 3
            && rig.count(&["call_requested"]) == 0
            && !rig.recording()
            && source.opens() == 0
            && rig.notifier.calls().is_empty()),
        "an unknown value started a call or wrote more than one line per launch: {:?}",
        rig.lines(&["call_switch_refused"])
    );
}

#[test]
fn the_call_tick_thread_stops_a_call_whose_track_write_failed() {
    // AC11 (FR-017, FR-018; #278; I-e): the first write of a track file fails once while the
    // call records (T-144: the writer flags it and `next_deadline()` is now). Nothing but the
    // `call-tick` thread calls `tick`, so within the hang bound the call stops by itself:
    // `call_stopped reason=storage_error` once and one `notice.stopped.storage` offer
    // (`retry: None`). Without the thread the call keeps recording. The error fake fails one
    // write (F-024), so a later retry cannot hide a swallow. Bite: no tick thread (the call
    // keeps recording), a thread that waits for the 10 s poll instead of `next_deadline`,
    // a thread that ticks once and never re-reads the deadline, a stop whose notice is dropped.
    let _serial = serial();
    let armed = Arc::new(AtomicBool::new(false));
    let source = WavSource::new();
    let rig = CallRig::started(
        source.clone(),
        FakeDisk::with_available(100_000_000_000),
        failing_once_opener(Arc::clone(&armed)),
    );
    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));
    assert!(eventually(|| rig.recording()), "the call did not start");

    armed.store(true, Ordering::SeqCst);
    source.pump();

    assert!(
        eventually(|| rig.count(&["call_stopped", "reason=storage_error"]) == 1),
        "the call kept recording after a failed track write; recording = {}",
        rig.recording()
    );
    assert!(!rig.recording());
    let (id, params) = CallNotice::Stopped {
        reason: StopReason::StorageError,
    }
    .message();
    assert!(
        eventually(|| !rig.notifier.calls().is_empty()),
        "no stop notice reached the notifier"
    );
    assert!(
        holds_for_a_while(|| rig.notifier.calls() == vec![offer(id, params.clone())]),
        "expected one `notice.stopped.storage`, got {:?}",
        rig.notifier.calls()
    );
}

#[test]
fn the_call_tick_thread_stops_below_200_mb_at_the_first_poll() {
    // AC11 (FR-017; #278; I-e): the free space of the calls folder is under 200 MB; the first
    // poll is due at the start (T-144), so the `call-tick` thread ticks at once: the call logs
    // `call_storage_failed kind=low_disk` and then stops with `call_stopped reason=low_disk`,
    // one `notice.stopped.low_disk` offer. Nothing else calls `tick`, so without the thread
    // the call keeps recording. Bite: no tick thread; a thread that sleeps the whole 10 s
    // poll period before its first tick (the wait is a positive one: it only fails at the
    // hang bound); `tick` wired to the wrong session method.
    let _serial = serial();
    let disk: Arc<dyn DiskSpace> = FakeDisk::with_available(100_000_000);
    let source = WavSource::new();
    let rig = CallRig::started(source, disk, plain_opener());

    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));

    assert!(
        eventually(|| rig.count(&["call_stopped", "reason=low_disk"]) == 1),
        "the call kept recording with under 200 MB free; recording = {}",
        rig.recording()
    );
    assert_eq!(
        rig.count(&["call_storage_failed", "kind=low_disk"]),
        1,
        "the low_disk storage line"
    );
    assert!(!rig.recording());
    let (id, params) = CallNotice::Stopped {
        reason: StopReason::LowDisk,
    }
    .message();
    assert!(
        eventually(|| !rig.notifier.calls().is_empty()),
        "no stop notice reached the notifier"
    );
    assert!(
        holds_for_a_while(|| rig.notifier.calls() == vec![offer(id, params.clone())]),
        "expected one `notice.stopped.low_disk`, got {:?}",
        rig.notifier.calls()
    );
}
