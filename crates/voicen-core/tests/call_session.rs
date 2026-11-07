//! T-126 (spec 007 T024): `CallSession` through its public API, over a scripted
//! `TrackSource`, scripted `CallRoots`, the real `CallStore` on temp dirs, a real
//! `Log` and a fake monotonic clock. The pinned API is documented in
//! `tests/call_support/mod.rs`.
//!
//! Rules that bind these tests: F-018 (nothing here races a background thread: the
//! writer thread is held with `pause_writer` or joined by `stop` before an assertion,
//! and waits poll state), F-021 (no wall-clock ceiling: the 10 s reminder runs on the
//! fake monotonic clock), F-024 (every error fake fails exactly one call).
//!
//! "Subtitles-only" (FR-019) is the session state subtitles on, record off. 1a's UI never
//! turns subtitles on, so the session itself is what is tested: a fake subtitle consumer
//! (`call_support::FakeSubtitles`) is fed while nothing reaches the calls root.

mod call_support;
mod common;
mod diag_support;

use std::fs::File;
use std::io;
use std::sync::{Arc, Barrier};
use std::time::{Duration, UNIX_EPOCH};

use call_support::{CallRig, IndicatorCall, ShellCall, BUDGET, OS};
use serde_json::{json, Value};
use voicen_core::autostart::FakeAutostart;
use voicen_core::call::codec::{scan, ReadEnd};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick, WarningKind};
use voicen_core::call::session::{CallSettingsPort, Toggled};
use voicen_core::call::{
    CallNotice, CallSource, OpenError, RefusedPart, RefusedReason, StopReason, Track, TrackDevice,
};
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::platform::DeviceId;
use voicen_core::secrets::FakeCredentialStore;
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::roots::{FakeCallRoots, UnwritableReason};
use voicen_core::settings::service::{
    IndicatorPositionFailed, SettingsDeps, SettingsService, WarnedHostFailed,
};
use voicen_core::settings::url::{check_base_url, provider_host};
use voicen_core::settings::{defaults, IndicatorPosition, Microphone};

fn wait(cond: impl FnMut() -> bool) -> bool {
    common::timing::eventually(BUDGET, cond)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[track_caller]
fn started(rig: &CallRig, source: CallSource) {
    assert_eq!(
        rig.session.toggle_record(true, source),
        Toggled::Started,
        "premise: the start succeeds"
    );
}

/// Feeds both tracks `from..to` and waits for nothing: `stop` drains.
fn feed(rig: &CallRig, from: u64, to: u64) {
    rig.source.feed_both(from, to, 0.3, 0.1);
}

fn warnings_of(rig: &CallRig) -> Vec<WarningKind> {
    rig.indicator
        .states()
        .last()
        .map(|s| s.warnings.iter().map(|w| w.kind).collect())
        .unwrap_or_default()
}

// ---- off at construction ---------------------------------------------------------------

#[test]
fn off_at_construction() {
    // Acceptance 1 (T024, T025): a new session is off; asking it to stop does nothing
    // and touches no port. Bite: a session constructed recording; a constructor that
    // opens a track, probes the root or pushes an indicator state; a stop that logs or
    // notifies while off.
    let rig = CallRig::new();
    assert!(!rig.session.is_recording());
    assert_eq!(rig.session.try_is_recording(), Some(false));
    assert_eq!(rig.session.next_deadline(), None);
    assert_eq!(
        rig.session.toggle_record(false, CallSource::Tray),
        Toggled::AlreadyOff
    );
    assert!(!rig.session.stop(StopReason::Exit));
    rig.session.tick();
    assert!(rig.source.opens().is_empty());
    assert_eq!(rig.roots.probes(), 0);
    assert!(rig.folders().is_empty());
    assert!(
        rig.indicator.calls().is_empty(),
        "{:?}",
        rig.indicator.calls()
    );
    assert!(rig.shell.calls().is_empty());
    assert!(rig.call_lines().is_empty(), "{:?}", rig.call_lines());
}

// ---- one session under one lock ---------------------------------------------------------

#[test]
fn one_session_for_repeated_on_from_every_source() {
    // Acceptance 2: tray, hotkey and switch all enter toggle_record(on, source); a
    // repeated on, from any source, changes nothing: one capture per track, one
    // folder, one call_started, the indicator and the tray's recording flag set once.
    // The start order is system, mic, root. Bite: a second start (second folder, four
    // opens), a repeated on that re-probes or re-publishes, a start path per source,
    // the root probed before the tracks are open.
    for first in [CallSource::Tray, CallSource::Hotkey, CallSource::Switch] {
        let rig = CallRig::new();
        started(&rig, first);
        for again in [CallSource::Tray, CallSource::Hotkey, CallSource::Switch] {
            assert_eq!(
                rig.session.toggle_record(true, again),
                Toggled::AlreadyOn,
                "{first:?} then {again:?}"
            );
        }
        assert!(rig.session.is_recording());
        assert_eq!(
            rig.source.opens(),
            vec![
                (Track::System, TrackDevice::DefaultOutputLoopback),
                (Track::Mic, TrackDevice::DefaultInput),
            ],
            "{first:?}: one capture per track"
        );
        assert_eq!(rig.source.open_handles(), 2);
        assert_eq!(rig.folders().len(), 1, "{first:?}: one folder");
        assert_eq!(
            *rig.order.lock().expect("order"),
            vec!["open system", "open mic", "probe"],
            "{first:?}: system, then mic, then root"
        );
        assert_eq!(rig.roots.probes(), 1);
        assert_eq!(rig.lines_named("call_started").len(), 1);
        assert!(rig.lines_named("call_refused").is_empty());
        assert_eq!(rig.indicator.recording_flags(), vec![true], "{first:?}");
        assert_eq!(
            rig.indicator.states().len(),
            1,
            "{first:?}: the state is published once per change"
        );
        let requested = rig.lines_named("call_requested");
        let source = match first {
            CallSource::Tray => "tray",
            CallSource::Hotkey => "hotkey",
            CallSource::Switch => "switch",
        };
        assert_eq!(requested[0].get("source"), Some(source));
        assert!(rig.session.stop(StopReason::Exit));
    }
}

#[test]
fn concurrent_starts_give_one_session() {
    // Acceptance 2 "under one lock": eight threads turn record on at once; exactly one
    // gets Started, the others AlreadyOn; one folder, two captures. Bite: a check
    // outside the lock (two folders, four opens), a lock per source.
    let rig = CallRig::new();
    let barrier = Barrier::new(8);
    let results: Vec<Toggled> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let (rig, barrier) = (&rig, &barrier);
                scope.spawn(move || {
                    let source = [CallSource::Tray, CallSource::Hotkey, CallSource::Switch][i % 3];
                    barrier.wait();
                    rig.session.toggle_record(true, source)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("joins"))
            .collect()
    });
    assert_eq!(
        results.iter().filter(|t| **t == Toggled::Started).count(),
        1
    );
    assert_eq!(
        results.iter().filter(|t| **t == Toggled::AlreadyOn).count(),
        7,
        "{results:?}"
    );
    assert_eq!(rig.folders().len(), 1);
    assert_eq!(rig.source.opens().len(), 2);
    assert_eq!(rig.source.open_handles(), 2);
    assert_eq!(rig.lines_named("call_started").len(), 1);
}

#[test]
fn off_then_on_gives_a_second_folder() {
    // Acceptance 2: off closes the call; the next on is a new call in a new folder
    // (same second, so the id carries a suffix). Bite: the first folder reused, a
    // handle left open by the off, the second start refused as AlreadyOn.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 400);
    assert_eq!(
        rig.session.toggle_record(false, CallSource::Tray),
        Toggled::Stopped
    );
    assert!(!rig.session.is_recording());
    assert_eq!(rig.source.open_handles(), 0);
    assert_eq!(rig.source.stops(), 2);
    let first = rig.folders();
    assert_eq!(first.len(), 1);
    started(&rig, CallSource::Hotkey);
    let both = rig.folders();
    assert_eq!(both.len(), 2, "{both:?}");
    assert_ne!(both[0], both[1]);
    assert!(both.contains(&first[0]), "the first call is kept");
    assert_eq!(rig.source.open_handles(), 2);
    assert_eq!(rig.source.opens().len(), 4);
    assert_eq!(rig.lines_named("call_started").len(), 2);
    assert_eq!(rig.indicator.recording_flags(), vec![true, false, true]);
    assert!(rig.session.stop(StopReason::Exit));
    assert_eq!(rig.source.open_handles(), 0);
}

#[test]
fn the_selected_microphone_is_opened_by_id() {
    // Acceptance 2 / invariant "mic: Selected(id)": a saved microphone is opened by its
    // id, the loopback is the system track. Bite: the default input opened regardless.
    let rig = CallRig::with(|s| {
        s.calls.warned_hosts = vec!["api.example.com".to_string()];
        s.microphone = Some(Microphone {
            id: "mic-1".to_string(),
            name: "Desk mic".to_string(),
        });
    });
    started(&rig, CallSource::Tray);
    assert_eq!(
        rig.source.opens(),
        vec![
            (Track::System, TrackDevice::DefaultOutputLoopback),
            (
                Track::Mic,
                TrackDevice::Selected(DeviceId("mic-1".to_string()))
            ),
        ]
    );
    assert!(warnings_of(&rig).is_empty(), "{:?}", warnings_of(&rig));
    rig.session.stop(StopReason::Exit);
}

// ---- refusals ---------------------------------------------------------------------------

const OPEN_ERRORS: [(OpenError, RefusedReason, &str); 4] = [
    (OpenError::NoDevice, RefusedReason::NoDevice, "no_device"),
    (
        OpenError::AccessDenied,
        RefusedReason::AccessDenied,
        "access_denied",
    ),
    (OpenError::Busy, RefusedReason::Busy, "busy"),
    (OpenError::Other, RefusedReason::Other, "other"),
];

#[test]
fn a_track_that_fails_to_open_refuses_the_start_and_leaves_nothing() {
    // Acceptance 3 (failure branch): system and mic x no_device / access_denied / busy /
    // other. The start is refused with `call_refused part= reason=`; no handle stays
    // open (what was opened is stopped), no folder exists, the root was never probed
    // when a track failed, the Calls tab is NOT asked for, the indicator never turned on.
    // The fake fails exactly one open (F-024), so the next start succeeds: nothing is
    // left wedged. Bite: a mic failure that leaves the system capture open, a folder
    // made before the tracks, a refusal without a log line or with two, a track refusal
    // that opens the tab, the reason mapped to its neighbour, the part swapped.
    for (track, part, name) in [
        (Track::System, RefusedPart::System, "system"),
        (Track::Mic, RefusedPart::Mic, "mic"),
    ] {
        for (error, reason, reason_name) in OPEN_ERRORS {
            let rig = CallRig::new();
            rig.source.fail_next(track, error);
            let what = format!("{name} {reason_name}");
            assert_eq!(
                rig.session.toggle_record(true, CallSource::Tray),
                Toggled::Refused { part, reason },
                "{what}"
            );
            assert!(!rig.session.is_recording(), "{what}");
            assert_eq!(rig.source.open_handles(), 0, "{what}: nothing left open");
            assert_eq!(
                rig.source.stops(),
                rig.source.opens().len() - 1,
                "{what}: what was opened is stopped"
            );
            assert!(rig.folders().is_empty(), "{what}: no folder");
            assert!(rig.root_files().is_empty(), "{what}: no file");
            assert_eq!(
                rig.roots.probes(),
                0,
                "{what}: the root comes after the tracks"
            );
            let expected_order: &[&str] = match track {
                Track::System => &["open system"],
                Track::Mic => &["open system", "open mic"],
            };
            assert_eq!(*rig.order.lock().expect("order"), expected_order, "{what}");
            let refused = rig.lines_named("call_refused");
            assert_eq!(refused.len(), 1, "{what}: {refused:?}");
            assert_eq!(refused[0].get("part"), Some(name), "{what}");
            assert_eq!(refused[0].get("reason"), Some(reason_name), "{what}");
            assert!(rig.lines_named("call_started").is_empty(), "{what}");
            assert_eq!(
                rig.shell.tab_opens(),
                0,
                "{what}: a track refusal never opens the tab"
            );
            assert!(
                !rig.indicator.recording_flags().contains(&true),
                "{what}: the indicator never turned on"
            );
            assert_eq!(
                rig.session.toggle_record(true, CallSource::Tray),
                Toggled::Started,
                "{what}: the next start works"
            );
            rig.session.stop(StopReason::Exit);
        }
    }
}

#[test]
fn no_devices_gives_system_no_device_and_the_mic_is_not_tried() {
    // Acceptance 3: with no devices at all the first failure is the system track:
    // `part=system reason=no_device`, once, and the mic is never opened (the order is
    // system first). Bite: the mic opened first (part=mic), both failures logged.
    let rig = CallRig::new();
    rig.source.fail_next(Track::System, OpenError::NoDevice);
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    assert_eq!(
        rig.session.toggle_record(true, CallSource::Hotkey),
        Toggled::Refused {
            part: RefusedPart::System,
            reason: RefusedReason::NoDevice
        }
    );
    assert_eq!(rig.source.opens().len(), 1);
    assert_eq!(*rig.order.lock().expect("order"), vec!["open system"]);
    let refused = rig.lines_named("call_refused");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].get("part"), Some("system"));
    assert_eq!(refused[0].get("reason"), Some("no_device"));
    assert_eq!(rig.shell.tab_opens(), 0);
}

#[test]
fn a_root_refusal_leaves_nothing_and_asks_for_the_calls_tab() {
    // Acceptance 3 + decision #243 D1: every UnwritableReason of the probe refuses the
    // start as part=root, mapped missing -> missing, access_denied -> access_denied,
    // not_a_folder / relative / disk_full -> unwritable, other -> other. The tracks were
    // opened first (system, mic) and are stopped again; no folder; one call_refused
    // line; the Calls tab is asked for exactly once, `missing` included, after the lock.
    // Bite: `missing` not opening the tab, the tab opened twice (per layer), the tab
    // asked for inside the lock, a refusal that keeps the handles, a mapping row
    // swapped, a call_storage_failed line on a probe refusal.
    let table = [
        (UnwritableReason::Missing, RefusedReason::Missing, "missing"),
        (
            UnwritableReason::AccessDenied,
            RefusedReason::AccessDenied,
            "access_denied",
        ),
        (
            UnwritableReason::NotAFolder,
            RefusedReason::Unwritable,
            "unwritable",
        ),
        (
            UnwritableReason::Relative,
            RefusedReason::Unwritable,
            "unwritable",
        ),
        (
            UnwritableReason::DiskFull,
            RefusedReason::Unwritable,
            "unwritable",
        ),
        (UnwritableReason::Other, RefusedReason::Other, "other"),
    ];
    for (probe, reason, name) in table {
        let rig = CallRig::new();
        rig.roots.refuse_next(probe);
        assert_eq!(
            rig.session.toggle_record(true, CallSource::Switch),
            Toggled::Refused {
                part: RefusedPart::Root,
                reason
            },
            "{probe:?}"
        );
        assert!(!rig.session.is_recording(), "{probe:?}");
        assert_eq!(
            *rig.order.lock().expect("order"),
            vec!["open system", "open mic", "probe"],
            "{probe:?}: system, mic, then root"
        );
        assert_eq!(rig.source.open_handles(), 0, "{probe:?}");
        assert_eq!(rig.source.stops(), 2, "{probe:?}");
        assert!(rig.folders().is_empty(), "{probe:?}: no folder");
        assert!(rig.root_files().is_empty(), "{probe:?}: no file");
        let refused = rig.lines_named("call_refused");
        assert_eq!(refused.len(), 1, "{probe:?}: {refused:?}");
        assert_eq!(refused[0].get("part"), Some("root"), "{probe:?}");
        assert_eq!(refused[0].get("reason"), Some(name), "{probe:?}");
        assert!(
            rig.lines_named("call_storage_failed").is_empty(),
            "{probe:?}"
        );
        assert_eq!(rig.shell.tab_opens(), 1, "{probe:?}: the Calls tab, once");
        assert_eq!(
            rig.shell.held_flags(),
            vec![false],
            "{probe:?}: after the lock"
        );
        assert!(
            !rig.indicator.recording_flags().contains(&true),
            "{probe:?}"
        );
        assert_eq!(
            rig.session.toggle_record(true, CallSource::Switch),
            Toggled::Started,
            "{probe:?}: the probe refused exactly once"
        );
        assert_eq!(
            rig.shell.tab_opens(),
            1,
            "{probe:?}: a start does not ask again"
        );
        rig.session.stop(StopReason::Exit);
    }
}

#[test]
fn a_storage_failure_after_the_probe_logs_its_kind_then_refuses_root_other() {
    // Decision #243 D2: CreateError::Storage(kind) logs `call_storage_failed kind=write`
    // (no os code) and then `call_refused part=root reason=other`, in that order, with the
    // Calls tab asked for once. The opener fails exactly one call (F-024): the first
    // track file (n = 1) and the second (n = 2, the first file already made and removed).
    // No folder, no handle. Bite: the storage kind dropped, the order reversed, the
    // refusal mapped to `unwritable`, the tab not opened, the failure swallowed so the
    // call starts without a file (`?` replaced by a default), the folder left behind.
    for n in [1, 2] {
        let rig = CallRig::new();
        rig.opener.fail_nth(n, io::ErrorKind::PermissionDenied);
        assert_eq!(
            rig.session.toggle_record(true, CallSource::Tray),
            Toggled::Refused {
                part: RefusedPart::Root,
                reason: RefusedReason::Other
            },
            "file {n}"
        );
        let lines: Vec<_> = rig
            .call_lines()
            .into_iter()
            .filter(|l| l.head == "call_storage_failed" || l.head == "call_refused")
            .collect();
        let heads: Vec<&str> = lines.iter().map(|l| l.head.as_str()).collect();
        assert_eq!(
            heads,
            vec!["call_storage_failed", "call_refused"],
            "file {n}"
        );
        assert_eq!(lines[0].get("kind"), Some("write"), "file {n}");
        assert_eq!(lines[0].get("os"), None, "file {n}: no os code is known");
        assert_eq!(lines[1].get("part"), Some("root"), "file {n}");
        assert_eq!(lines[1].get("reason"), Some("other"), "file {n}");
        assert!(rig.folders().is_empty(), "file {n}: no folder");
        assert!(rig.root_files().is_empty(), "file {n}: no file");
        assert_eq!(rig.source.open_handles(), 0, "file {n}");
        assert_eq!(rig.shell.tab_opens(), 1, "file {n}: the tab, once");
        assert!(!rig.session.is_recording(), "file {n}");
        assert!(rig.lines_named("call_started").is_empty(), "file {n}");
        assert_eq!(
            rig.session.toggle_record(true, CallSource::Tray),
            Toggled::Started,
            "file {n}: the opener failed exactly once"
        );
        assert_eq!(rig.folders().len(), 1);
        rig.session.stop(StopReason::Exit);
    }
}

// ---- the microphone fallback ------------------------------------------------------------

#[test]
fn a_missing_selected_mic_starts_on_the_default_input_with_mic_changed() {
    // Acceptance 4: the selected mic is gone (NoDevice on Selected), the default input is
    // there: the call starts on DefaultInput and the indicator shows `micChanged` with its
    // catalog text. Bite: no retry (refused), a retry without the warning, the retry
    // on the system track, the warning only in the log.
    let rig = CallRig::with(|s| {
        s.calls.warned_hosts = vec!["api.example.com".to_string()];
        s.microphone = Some(Microphone {
            id: "gone-mic".to_string(),
            name: "Unplugged".to_string(),
        });
    });
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    started(&rig, CallSource::Tray);
    assert_eq!(
        rig.source.opens(),
        vec![
            (Track::System, TrackDevice::DefaultOutputLoopback),
            (
                Track::Mic,
                TrackDevice::Selected(DeviceId("gone-mic".to_string()))
            ),
            (Track::Mic, TrackDevice::DefaultInput),
        ]
    );
    assert_eq!(rig.source.open_handles(), 2);
    let state = rig.indicator.states().last().cloned().expect("a state");
    assert!(state.recording);
    assert_eq!(state.warnings.len(), 1, "{:?}", state.warnings);
    assert_eq!(state.warnings[0].kind, WarningKind::MicChanged);
    assert_eq!(
        state.warnings[0].text,
        "Microphone changed to the Windows default"
    );
    assert_eq!(rig.lines_named("call_started").len(), 1);
    assert!(rig.lines_named("call_refused").is_empty());
    rig.session.stop(StopReason::Exit);
}

#[test]
fn the_fallback_is_for_a_missing_mic_only() {
    // Acceptance 4 boundary: busy / access_denied on the selected mic is a refusal, not
    // a retry on the default input; and a default input that is missing too refuses with
    // part=mic reason=no_device and no warning. Bite: a retry on every error, a retry
    // whose failure is swallowed into a start with one track.
    for error in [OpenError::Busy, OpenError::AccessDenied] {
        let rig = CallRig::with(|s| {
            s.microphone = Some(Microphone {
                id: "busy-mic".to_string(),
                name: "Busy".to_string(),
            });
        });
        rig.source.fail_next(Track::Mic, error);
        assert!(matches!(
            rig.session.toggle_record(true, CallSource::Tray),
            Toggled::Refused {
                part: RefusedPart::Mic,
                ..
            }
        ));
        assert_eq!(rig.source.opens().len(), 2, "{error:?}: no second try");
        assert_eq!(rig.source.open_handles(), 0, "{error:?}");
    }
    let rig = CallRig::with(|s| {
        s.microphone = Some(Microphone {
            id: "gone-mic".to_string(),
            name: "Unplugged".to_string(),
        });
    });
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    assert_eq!(
        rig.session.toggle_record(true, CallSource::Tray),
        Toggled::Refused {
            part: RefusedPart::Mic,
            reason: RefusedReason::NoDevice
        }
    );
    assert_eq!(rig.source.open_handles(), 0);
    assert!(rig.folders().is_empty());
    assert!(rig.indicator.states().iter().all(|s| s.warnings.is_empty()));
    let refused = rig.lines_named("call_refused");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].get("part"), Some("mic"));
}

// ---- subtitles-only ---------------------------------------------------------------------

/// The 16 kHz samples `feed_both(from, to, ..)` delivers per track (20 ms = 320 samples).
fn fed(from: u64, to: u64) -> usize {
    usize::try_from((to - from) / 20 * 320).expect("fits")
}

fn consumer_has(rig: &CallRig, samples: usize) -> bool {
    wait(|| {
        rig.subtitles.samples(Track::Mic) == samples
            && rig.subtitles.samples(Track::System) == samples
    })
}

/// Every file under the root with its bytes, sorted.
fn root_snapshot(rig: &CallRig) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut all: Vec<_> = rig
        .root_files()
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p).expect("readable");
            (p, bytes)
        })
        .collect();
    all.sort();
    all
}

#[test]
fn subtitles_only_writes_nothing() {
    // Acceptance 5 (FR-019, I3): subtitles on, record off. The captures are open and the
    // fake consumer receives the audio of both tracks, while nothing reaches the root:
    // no probe, no folder, no file, no call_* line, the recording indicator never on.
    // A tick and a clock move change nothing. Turning subtitles off closes the captures
    // and the consumer gets no more. Bite: a subtitles-on path that calls CallStore or
    // the probe, a tick or an off that writes meta, a consumer that is never fed, a
    // second open on the repeated on, captures left open by the off.
    let rig = CallRig::new();
    assert_eq!(
        rig.session.toggle_subtitles(true, CallSource::Switch),
        Toggled::Started
    );
    assert!(rig.session.is_subtitles());
    assert!(!rig.session.is_recording());
    assert_eq!(rig.source.open_handles(), 2);
    assert_eq!(
        *rig.order.lock().expect("order"),
        ["open system", "open mic"],
        "two captures, no probe"
    );
    rig.source.feed_both(0, 400, 0.3, 0.1);
    assert!(
        consumer_has(&rig, fed(0, 400)),
        "the consumer gets both tracks"
    );
    assert_eq!(
        rig.session.toggle_subtitles(true, CallSource::Hotkey),
        Toggled::AlreadyOn
    );
    assert_eq!(rig.source.opens().len(), 2, "a repeated on opens nothing");
    rig.mono.advance(ms(30_000));
    rig.session.tick();

    let no_disk = |what: &str| {
        assert!(
            rig.root_files().is_empty(),
            "{what}: {:?}",
            rig.root_files()
        );
        assert!(rig.folders().is_empty(), "{what}");
        assert_eq!(rig.roots.probes(), 0, "{what}");
        assert!(
            !rig.root
                .parent()
                .expect("parent")
                .join("default-calls")
                .exists(),
            "{what}"
        );
        assert!(
            rig.call_lines().is_empty(),
            "{what}: {:?}",
            rig.call_lines()
        );
        assert!(
            !rig.indicator.recording_flags().contains(&true),
            "{what}: the indicator shows no recording"
        );
        assert_eq!(rig.shell.tab_opens(), 0, "{what}");
    };
    no_disk("while subtitles run");

    assert_eq!(
        rig.session.toggle_subtitles(false, CallSource::Switch),
        Toggled::Stopped
    );
    assert!(!rig.session.is_subtitles());
    assert_eq!(rig.source.open_handles(), 0);
    assert_eq!(rig.source.stops(), 2);
    assert_eq!(
        rig.session.toggle_subtitles(false, CallSource::Switch),
        Toggled::AlreadyOff
    );
    assert_eq!(rig.subtitles.samples(Track::Mic), fed(0, 400));
    assert_eq!(rig.subtitles.samples(Track::System), fed(0, 400));
    no_disk("after subtitles are off");
}

#[test]
fn a_subtitles_start_that_cannot_open_a_track_is_refused_and_leaves_nothing() {
    // Acceptance 5 failure branch: the system capture, then the mic capture, fails to
    // open once (F-024). The toggle is Refused at that part, subtitles stay off, what
    // was opened is stopped, nothing is written; the next toggle works. Bite: a refusal
    // swallowed into Started, the system capture left open after a mic failure, the flag
    // set before the opens.
    for (track, part, name) in [
        (Track::System, RefusedPart::System, "system"),
        (Track::Mic, RefusedPart::Mic, "mic"),
    ] {
        let rig = CallRig::new();
        rig.source.fail_next(track, OpenError::Busy);
        assert_eq!(
            rig.session.toggle_subtitles(true, CallSource::Switch),
            Toggled::Refused {
                part,
                reason: RefusedReason::Busy
            },
            "{name}"
        );
        assert!(!rig.session.is_subtitles(), "{name}");
        assert_eq!(rig.source.open_handles(), 0, "{name}");
        assert!(rig.root_files().is_empty(), "{name}");
        assert_eq!(rig.roots.probes(), 0, "{name}");
        assert_eq!(
            rig.session.toggle_subtitles(true, CallSource::Switch),
            Toggled::Started,
            "{name}: the next toggle works"
        );
        assert_eq!(
            rig.session.toggle_subtitles(false, CallSource::Switch),
            Toggled::Stopped
        );
    }
}

#[test]
fn record_shares_the_captures_of_running_subtitles_and_survives_their_record_off() {
    // FR-019 / FR-37: subtitles on, then record on, then record off, then subtitles off.
    // Record on opens nothing new (one capture per track) and makes one folder; record
    // off finishes the folder (readable, ready-to-transcribe) while the captures stay
    // open and keep feeding the consumer; nothing is written after the record off; the
    // consumer never misses a block across the transitions. Bite: a second open on
    // record on, captures closed by the record off, the consumer cut at record on or
    // off, blocks written after record off, a record-off that leaves the folder open.
    let rig = CallRig::new();
    rig.session.toggle_subtitles(true, CallSource::Switch);
    rig.source.feed_both(0, 200, 0.3, 0.1);
    assert!(consumer_has(&rig, fed(0, 200)));

    started(&rig, CallSource::Tray);
    assert_eq!(rig.source.opens().len(), 2, "no second open");
    assert_eq!(rig.source.open_handles(), 2);
    assert_eq!(rig.folders().len(), 1);
    assert_eq!(rig.roots.probes(), 1);
    assert!(rig.session.is_recording() && rig.session.is_subtitles());
    rig.source.feed_both(200, 400, 0.3, 0.1);
    assert!(consumer_has(&rig, fed(0, 400)));

    rig.mono.advance(ms(400));
    assert_eq!(
        rig.session.toggle_record(false, CallSource::Tray),
        Toggled::Stopped
    );
    assert!(!rig.session.is_recording());
    assert!(rig.session.is_subtitles(), "record off leaves subtitles on");
    assert_eq!(rig.source.open_handles(), 2, "the captures stay open");
    assert_eq!(rig.source.stops(), 0);
    assert_eq!(rig.only_meta()["state"], json!("ready-to-transcribe"));
    for file in ["mic.opus", "system.opus"] {
        let path = rig.folders()[0].join(file);
        let scanned = scan(File::open(&path).expect(file)).expect("a readable track");
        assert_eq!(scanned.end, ReadEnd::Complete, "{file} is finished");
    }
    assert_eq!(rig.lines_named("call_stopped").len(), 1);
    let after_stop = root_snapshot(&rig);

    rig.source.feed_both(400, 600, 0.3, 0.1);
    assert!(
        consumer_has(&rig, fed(0, 600)),
        "the consumer keeps getting audio"
    );
    assert_eq!(
        rig.session.toggle_subtitles(false, CallSource::Switch),
        Toggled::Stopped
    );
    assert_eq!(rig.source.open_handles(), 0);
    assert_eq!(rig.source.stops(), 2);
    assert_eq!(
        root_snapshot(&rig),
        after_stop,
        "nothing is written after the record off"
    );
    assert_eq!(rig.folders().len(), 1);
    assert_eq!(rig.subtitles.samples(Track::Mic), fed(0, 600));
    assert_eq!(rig.subtitles.samples(Track::System), fed(0, 600));
    // The contract fixes `call_started ... subtitles=0` for 1a (core-call.md); whether a
    // start with subtitles on says 1 is not decided there, so only the count is pinned.
    assert_eq!(rig.lines_named("call_started").len(), 1);
}

#[test]
fn subtitles_off_while_recording_keeps_the_call_and_the_consumer_stops() {
    // Record on, subtitles on then off: the call goes on, one folder, the captures stay
    // open, the consumer receives nothing after the off, and the call is still written
    // in full. A record-only call never feeds the consumer. Bite: captures closed by the
    // subtitles off, the consumer fed while subtitles is off (also from a record-only
    // start), the call stopped by the subtitles off.
    let plain = CallRig::new();
    started(&plain, CallSource::Tray);
    feed(&plain, 0, 400);
    plain.session.stop(StopReason::User);
    assert_eq!(
        plain.subtitles.calls(),
        0,
        "a record-only call feeds nobody"
    );

    let rig = CallRig::new();
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    rig.source.feed_both(0, 200, 0.3, 0.1);
    assert!(consumer_has(&rig, fed(0, 200)), "fed while subtitles is on");
    assert_eq!(
        rig.session.toggle_subtitles(false, CallSource::Switch),
        Toggled::Stopped
    );
    assert!(rig.session.is_recording());
    assert_eq!(rig.source.opens().len(), 2);
    assert_eq!(rig.source.open_handles(), 2);
    rig.source.feed_both(200, 600, 0.3, 0.1);
    rig.mono.advance(ms(600));
    assert!(rig.session.stop(StopReason::User));
    assert_eq!(rig.subtitles.samples(Track::Mic), fed(0, 200));
    assert_eq!(rig.subtitles.samples(Track::System), fed(0, 200));
    let scanned = scan(File::open(rig.folders()[0].join("mic.opus")).expect("mic")).expect("scan");
    assert_eq!(scanned.end, ReadEnd::Complete);
    assert!(
        scanned.duration_48k >= 48_000 * 3 / 5,
        "the call kept all 600 ms: {}",
        scanned.duration_48k
    );
}

#[test]
fn subtitles_and_record_toggle_independently_under_one_lock() {
    // The two toggles never change each other's state, and raced against each other
    // they still give one capture per track and one folder. Bite: record off that turns
    // subtitles off (or the reverse), a lock per toggle (two opens per track, two
    // folders), a subtitles toggle that ignores the record state.
    let rig = CallRig::new();
    let states = |r: bool, s: bool, what: &str| {
        assert_eq!(rig.session.is_recording(), r, "{what}: record");
        assert_eq!(rig.session.is_subtitles(), s, "{what}: subtitles");
    };
    states(false, false, "start");
    started(&rig, CallSource::Tray);
    states(true, false, "record on");
    rig.session.toggle_subtitles(true, CallSource::Switch);
    states(true, true, "subtitles on");
    // Subtitles turned on over a running recording feed the consumer too (bite: the
    // running writer never told to start the feed; mutant 50).
    rig.source.feed_both(0, 200, 0.3, 0.1);
    assert!(
        consumer_has(&rig, fed(0, 200)),
        "subtitles on during a recording feed the consumer"
    );
    rig.session.toggle_subtitles(false, CallSource::Switch);
    states(true, false, "subtitles off");
    rig.session.toggle_subtitles(true, CallSource::Switch);
    rig.session.toggle_record(false, CallSource::Tray);
    states(false, true, "record off");
    rig.session.toggle_subtitles(false, CallSource::Switch);
    states(false, false, "both off");
    assert_eq!(rig.source.open_handles(), 0);

    let rig = CallRig::new();
    let barrier = Barrier::new(8);
    let results: Vec<(bool, Toggled)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let (rig, barrier) = (&rig, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    if i % 2 == 0 {
                        (true, rig.session.toggle_record(true, CallSource::Tray))
                    } else {
                        (
                            false,
                            rig.session.toggle_subtitles(true, CallSource::Switch),
                        )
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("joins"))
            .collect()
    });
    for record in [true, false] {
        let of: Vec<Toggled> = results
            .iter()
            .filter(|(r, _)| *r == record)
            .map(|(_, t)| *t)
            .collect();
        assert_eq!(
            of.iter().filter(|t| **t == Toggled::Started).count()
                + of.iter().filter(|t| **t == Toggled::AlreadyOn).count(),
            4,
            "record={record}: {of:?}"
        );
        assert!(
            of.iter().filter(|t| **t == Toggled::Started).count() <= 1,
            "record={record}: {of:?}"
        );
    }
    assert!(rig.session.is_recording() && rig.session.is_subtitles());
    assert_eq!(rig.source.opens().len(), 2, "one capture per track");
    assert_eq!(rig.source.open_handles(), 2);
    assert_eq!(rig.folders().len(), 1);
    assert_eq!(rig.lines_named("call_started").len(), 1);
}

#[test]
fn a_session_that_is_off_writes_nothing() {
    // Acceptance 5 (FR-019, I3): with record off nothing reaches the disk, whatever else
    // the session is asked: a stop, a tick, a clock move, a settings change, a turn-off.
    // The calls root stays empty, the default root is never created, the probe is never
    // run, no track is opened. Bite: a folder or meta created at construction or at
    // the first tick, a probe on every call.
    let rig = CallRig::new();
    rig.mono.advance(ms(30_000));
    rig.session.tick();
    rig.port.edit(|s| s.calls.reminder = false);
    assert_eq!(
        rig.session.toggle_record(false, CallSource::Hotkey),
        Toggled::AlreadyOff
    );
    rig.session.stop(StopReason::User);
    assert!(rig.root_files().is_empty(), "{:?}", rig.root_files());
    assert!(rig.folders().is_empty());
    assert!(!rig
        .root
        .parent()
        .expect("parent")
        .join("default-calls")
        .exists());
    assert_eq!(rig.roots.probes(), 0);
    assert!(rig.source.opens().is_empty());
    assert!(rig.call_lines().is_empty());
    assert!(rig.indicator.calls().is_empty());
}

// ---- stop -------------------------------------------------------------------------------

#[test]
fn stop_closes_both_files_readable_and_reports_it() {
    // Acceptance 6: stop closes both files readable (the Ogg scan ends Complete, with
    // audio), writes meta.json `ready-to-transcribe` with the stop reason, ended_at and
    // the duration, emits `call_saved_transcribing` after the lock, and logs `call_stopped`
    // with the byte counts of the two files. Bite: a file not finished (Truncated), meta
    // left `recording`, byte counts 0 or swapped, no notice, the notice before the
    // indicator is off, the capture left open.
    for (how, reason, name) in [
        (Some(CallSource::Tray), StopReason::User, "user"),
        (Some(CallSource::Hotkey), StopReason::Hotkey, "hotkey"),
        (None, StopReason::Exit, "exit"),
        (None, StopReason::Sleep, "sleep"),
    ] {
        let rig = CallRig::new();
        started(&rig, CallSource::Tray);
        feed(&rig, 0, 2000);
        rig.mono.advance(ms(2000));
        match how {
            Some(source) => assert_eq!(
                rig.session.toggle_record(false, source),
                Toggled::Stopped,
                "{name}"
            ),
            None => assert!(rig.session.stop(reason), "{name}"),
        }
        assert!(!rig.session.is_recording(), "{name}");
        assert_eq!(rig.source.open_handles(), 0, "{name}");
        assert_eq!(rig.source.stops(), 2, "{name}");

        let folders = rig.folders();
        assert_eq!(folders.len(), 1, "{name}");
        let dir = &folders[0];
        let mut sizes = Vec::new();
        for file in ["mic.opus", "system.opus"] {
            let path = dir.join(file);
            let scanned = scan(File::open(&path).expect(file)).expect("a readable track");
            assert_eq!(scanned.end, ReadEnd::Complete, "{name}: {file} is finished");
            assert!(
                scanned.duration_48k > 48_000 * 3 / 2,
                "{name}: {file} holds the audio: {}",
                scanned.duration_48k
            );
            sizes.push(std::fs::metadata(&path).expect("size").len());
        }
        assert!(sizes.iter().all(|s| *s > 0));
        let meta: Value =
            serde_json::from_slice(&std::fs::read(dir.join("meta.json")).expect("meta.json"))
                .expect("json");
        assert_eq!(meta["state"], json!("ready-to-transcribe"), "{name}");
        assert_eq!(meta["stop_reason"], json!(name), "{name}");
        assert!(meta["ended_at"].is_string(), "{name}: {meta}");
        let duration = meta["duration_ms"].as_u64().expect("a duration");
        assert!((1_500..=2_500).contains(&duration), "{name}: {duration}");
        assert!(!dir.join("meta.json.tmp").exists(), "{name}");

        let stopped = rig.lines_named("call_stopped");
        assert_eq!(stopped.len(), 1, "{name}");
        assert_eq!(stopped[0].get("reason"), Some(name), "{name}");
        assert_eq!(
            stopped[0].get("duration_ms"),
            Some(duration.to_string().as_str())
        );
        assert_eq!(
            stopped[0].get("mic_bytes"),
            Some(sizes[0].to_string().as_str()),
            "{name}"
        );
        assert_eq!(
            stopped[0].get("system_bytes"),
            Some(sizes[1].to_string().as_str()),
            "{name}"
        );
        let heads: Vec<String> = rig.call_lines().into_iter().map(|l| l.head).collect();
        let started_at = heads
            .iter()
            .position(|h| h == "call_started")
            .expect("started");
        let stopped_at = heads
            .iter()
            .position(|h| h == "call_stopped")
            .expect("stopped");
        assert!(started_at < stopped_at, "{name}: {heads:?}");

        assert_eq!(
            rig.shell.calls(),
            vec![ShellCall::Notice(CallNotice::CallSavedTranscribing)],
            "{name}"
        );
        assert_eq!(
            rig.shell.held_flags(),
            vec![false],
            "{name}: after the lock"
        );
        assert_eq!(rig.indicator.recording_flags(), vec![true, false], "{name}");
        assert_eq!(
            rig.indicator.states().last(),
            Some(&IndicatorState::idle()),
            "{name}"
        );
    }
}

// ---- the indicator inside the lock ------------------------------------------------------

#[test]
fn indicator_changes_inside_the_lock_and_shell_requests_after_it() {
    // Acceptance 7 (I4): every set_state / set_tick / set_call_recording runs while the
    // session lock is held (the fake asks `try_is_recording`, which answers None under
    // the lock), on the start, the tick (writer thread) and the stop; the shell's notice
    // runs with the lock released. A tick carries the levels of the audio: the mic loud,
    // the system quiet. Bite: an indicator call after unlock (the held flag false), a
    // shell call inside the lock, ticks published from the writer without the lock, a
    // tick that swaps the tracks' levels, no tick at all.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    assert!(
        rig.session.try_is_recording().is_some(),
        "the lock is free between calls"
    );
    // F-018: each pass moves the mono clock, feeds one 200 ms span (10 blocks per track)
    // and waits for the writer's next tick before the next pass, so the test is never
    // more than one span ahead of the writer and the 64-slot channels never fill (a full
    // channel turns blocks into drops, zero-filled, and every mic level reads 0.0).
    let loud = |ticks: &[IndicatorTick]| {
        ticks
            .iter()
            .any(|tick| tick.mic_level > 0.1 && tick.system_level < 0.01)
    };
    let mut got = false;
    for pass in 0..20u64 {
        let before = rig.indicator.ticks().len();
        let t = pass * 200;
        rig.mono.advance(ms(200));
        rig.source.feed_both(t, t + 200, 0.5, 0.0);
        rig.source.set_stamp_ms(t + 200);
        assert!(
            wait(|| rig.indicator.ticks().len() > before),
            "pass {pass}: the writer published no tick: {:?}",
            rig.indicator.ticks()
        );
        if loud(&rig.indicator.ticks()) {
            got = true;
            break;
        }
    }
    assert!(
        got,
        "a tick with the mic loud and the system quiet: {:?}",
        rig.indicator.ticks()
    );
    rig.session.stop(StopReason::User);
    let flags = rig.indicator.held_flags();
    assert!(flags.len() >= 4, "{:?}", rig.indicator.calls());
    assert!(
        flags.iter().all(|held| *held),
        "an indicator call ran outside the lock: {:?}",
        rig.indicator.calls()
    );
    assert!(
        rig.shell.held_flags().iter().all(|held| !held),
        "{:?}",
        rig.shell.calls()
    );
    assert!(!rig.shell.calls().is_empty());
    assert!(rig.session.try_is_recording().is_some());
}

#[test]
fn the_reminder_shows_for_10_seconds_at_each_start() {
    // Acceptance 7: the reminder and the headphones hint show from the start until
    // start + 10 s on the monotonic clock, then one state hides them; a next start shows
    // them again. `next_deadline` is the end of the reminder (the shell's timer) and
    // `tick` applies it. Bite: 9 s or 11 s, a state republished at every tick, the
    // deadline kept after expiry, the reminder shown only at the first start, texts kept
    // while hidden.
    let rig = CallRig::new();
    let t0 = rig.mono.read();
    started(&rig, CallSource::Tray);
    let first = rig.indicator.states();
    assert_eq!(first.len(), 1);
    assert!(first[0].recording && first[0].reminder_visible);
    assert_eq!(
        first[0].reminder_text,
        "Tell the other party the call is being recorded"
    );
    assert_eq!(first[0].headphones_hint, "Better with headphones");
    assert_eq!(rig.session.next_deadline(), Some(t0 + ms(10_000)));

    rig.mono.advance(ms(9_999));
    rig.session.tick();
    assert_eq!(
        rig.indicator.states().len(),
        1,
        "nothing changes before 10 s"
    );
    assert_eq!(rig.session.next_deadline(), Some(t0 + ms(10_000)));

    rig.mono.advance(ms(1));
    rig.session.tick();
    let after = rig.indicator.states();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after[1].recording && !after[1].reminder_visible);
    assert_eq!(after[1].reminder_text, "");
    assert_eq!(after[1].headphones_hint, "");
    assert_eq!(rig.session.next_deadline(), None);
    assert!(
        rig.indicator.held_flags().iter().all(|held| *held),
        "the expiry state is published inside the session lock: {:?}",
        rig.indicator.calls()
    );
    rig.session.tick();
    assert_eq!(
        rig.indicator.states().len(),
        2,
        "the expiry is published once"
    );

    rig.session.stop(StopReason::User);
    let t1 = rig.mono.read();
    started(&rig, CallSource::Hotkey);
    let again = rig.indicator.states();
    let last = again.last().expect("a state");
    assert!(
        last.recording && last.reminder_visible,
        "shown again at the next start"
    );
    assert_eq!(rig.session.next_deadline(), Some(t1 + ms(10_000)));
    rig.session.stop(StopReason::User);
    assert_eq!(
        rig.session.next_deadline(),
        None,
        "no deadline once the call is off"
    );
}

#[test]
fn the_reminder_is_not_shown_at_all_when_it_is_off() {
    // Acceptance 7: `calls.reminder` off -> the state at the start has neither the
    // reminder nor the hint (empty texts), no deadline is due, a tick changes nothing.
    // Bite: the setting ignored, texts filled with the flag false, a deadline armed.
    let rig = CallRig::with(|s| s.calls.reminder = false);
    started(&rig, CallSource::Tray);
    let states = rig.indicator.states();
    assert_eq!(states.len(), 1);
    assert!(states[0].recording && !states[0].reminder_visible);
    assert_eq!(states[0].reminder_text, "");
    assert_eq!(states[0].headphones_hint, "");
    assert_eq!(rig.session.next_deadline(), None);
    rig.mono.advance(ms(60_000));
    rig.session.tick();
    assert_eq!(rig.indicator.states().len(), 1);
    rig.session.stop(StopReason::User);
}

// ---- the provider notice ----------------------------------------------------------------

fn has_notice(rig: &CallRig) -> bool {
    rig.indicator.states().iter().any(|s| {
        s.warnings
            .iter()
            .any(|w| w.kind == WarningKind::ProviderNotice)
    })
}

#[test]
fn provider_notice_once_per_new_non_loopback_host() {
    // Acceptance 8 (FR-031, I5): the first call on a new remote host shows
    // `providerNotice` (catalog text with the host) and remembers the host through the
    // settings port; the next call on that host shows nothing and remembers nothing; a
    // different host shows it again. Bite: the notice at every call, the host not
    // remembered (shown twice), remembered without being shown, the host text missing,
    // the host in a log line.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    let state = rig.indicator.states().last().cloned().expect("a state");
    let notice = state
        .warnings
        .iter()
        .find(|w| w.kind == WarningKind::ProviderNotice)
        .expect("the provider notice");
    assert_eq!(
        notice.text,
        "The other party's audio goes to provider api.example.com"
    );
    assert_eq!(rig.port.remembered(), vec!["api.example.com".to_string()]);
    rig.session.stop(StopReason::User);

    let second = CallRig::with(|s| s.calls.warned_hosts = vec!["api.example.com".to_string()]);
    started(&second, CallSource::Tray);
    assert!(
        !has_notice(&second),
        "a remembered host is not announced again"
    );
    assert!(second.port.remembered().is_empty());
    second.session.stop(StopReason::User);

    // The same rig, a second call on the same host after it was remembered.
    started(&rig, CallSource::Tray);
    let states = rig.indicator.states();
    let after_second_start = states.last().expect("a state");
    assert!(
        after_second_start
            .warnings
            .iter()
            .all(|w| w.kind != WarningKind::ProviderNotice),
        "{after_second_start:?}"
    );
    assert_eq!(rig.port.remembered().len(), 1);
    rig.session.stop(StopReason::User);

    rig.port
        .edit(|s| s.api.base_url = "https://api.other.example.org/v1".to_string());
    started(&rig, CallSource::Tray);
    let last = rig.indicator.states().last().cloned().expect("a state");
    let other = last
        .warnings
        .iter()
        .find(|w| w.kind == WarningKind::ProviderNotice)
        .expect("a new host is announced");
    assert!(
        other.text.ends_with("api.other.example.org"),
        "{}",
        other.text
    );
    assert_eq!(
        rig.port.remembered(),
        vec![
            "api.example.com".to_string(),
            "api.other.example.org".to_string()
        ]
    );
    rig.session.stop(StopReason::User);

    for raw in rig_log_lines(&rig) {
        assert!(!raw.contains("example"), "a host reached the log: {raw}");
    }
}

fn rig_log_lines(rig: &CallRig) -> Vec<String> {
    diag_support::all_lines(&rig.logs)
}

#[test]
fn a_loopback_host_gives_no_notice() {
    // Acceptance 8: localhost, 127.0.0.1 and ::1 are not providers: no notice, nothing
    // remembered. Bite: the loopback check missing.
    for url in [
        "http://localhost:8000/v1",
        "http://127.0.0.1:8000/v1",
        "http://[::1]:8000/v1",
    ] {
        let rig = CallRig::with(|s| s.api.base_url = url.to_string());
        started(&rig, CallSource::Tray);
        assert!(!has_notice(&rig), "{url}");
        assert!(rig.port.remembered().is_empty(), "{url}");
        rig.session.stop(StopReason::User);
    }
}

#[test]
fn a_host_that_could_not_be_remembered_is_announced_again() {
    // Acceptance 8, failure branch: the settings port fails the write exactly once
    // (F-024). The call still starts and the notice is shown, nothing is remembered; the
    // next call shows it again and this time remembers the host. Bite: the write error
    // refusing the start, the host counted as remembered after a failed write, the notice
    // withheld when the write failed.
    let rig = CallRig::new();
    rig.port.fail_next_remember();
    started(&rig, CallSource::Tray);
    assert!(has_notice(&rig));
    assert!(rig.port.remembered().is_empty());
    assert!(rig.session.is_recording());
    rig.session.stop(StopReason::User);

    started(&rig, CallSource::Tray);
    let state = rig.indicator.states().last().cloned().expect("a state");
    assert!(state
        .warnings
        .iter()
        .any(|w| w.kind == WarningKind::ProviderNotice));
    assert_eq!(rig.port.remembered(), vec!["api.example.com".to_string()]);
    rig.session.stop(StopReason::User);
}

// ---- #244: drops and gaps ---------------------------------------------------------------

fn meta_drops(rig: &CallRig) -> Vec<Value> {
    let folders = rig.folders();
    let Some(dir) = folders.first() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(dir.join("meta.json")) else {
        return Vec::new();
    };
    let meta: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    meta["drops"].as_array().cloned().unwrap_or_default()
}

#[test]
fn dropped_blocks_are_logged_once_and_kept_in_meta() {
    // Decision #244 (I7): every Report::Dropped the writer receives gives exactly one
    // `call_track_dropped track= blocks= ms= at_ms=` WARN line and one meta.json
    // `drops[]` entry with the same values, written while the call runs and still there
    // (not duplicated) after stop. The writer is paused while the burst is fed, so the
    // channel (capacity 4) drops exactly the blocks past the fourth: 3 blocks of 20 ms,
    // then 2. Bite: no line, a line without the meta entry (or the reverse), the line
    // per block instead of per Report, a duplicate on stop, at_ms 0, track swapped,
    // the Gap logged as a drop.
    let rig = CallRig::with_capacity(4, |_| {});
    started(&rig, CallSource::Tray);

    let pause = rig.session.pause_writer();
    rig.source.feed(Track::Mic, 0, 140, 0.2);
    drop(pause);
    assert!(
        wait(|| meta_drops(&rig).len() == 1 && rig.lines_named("call_track_dropped").len() == 1),
        "the first drop reaches the log and the meta: {:?} / {:?}",
        meta_drops(&rig),
        rig.lines_named("call_track_dropped")
    );
    let pause = rig.session.pause_writer();
    rig.source.feed(Track::Mic, 140, 260, 0.2);
    drop(pause);
    assert!(
        wait(|| meta_drops(&rig).len() == 2 && rig.lines_named("call_track_dropped").len() == 2),
        "the second drop is a second line and a second entry"
    );
    rig.session.stop(StopReason::User);

    let lines = rig.lines_named("call_track_dropped");
    let drops = meta_drops(&rig);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(drops.len(), 2, "{drops:?}");
    for (line, expect_blocks, expect_ms) in [(&lines[0], 3, 60), (&lines[1], 2, 40)] {
        assert!(line.raw.contains(" WARN "), "{}", line.raw);
        assert_eq!(line.get("track"), Some("mic"));
        assert_eq!(line.get("blocks"), Some(expect_blocks.to_string().as_str()));
        assert_eq!(line.get("ms"), Some(expect_ms.to_string().as_str()));
    }
    let mut last_at = 0;
    for (line, entry, blocks, span) in
        [(&lines[0], &drops[0], 3, 60), (&lines[1], &drops[1], 2, 40)]
    {
        let at_ms: u64 = line.get("at_ms").expect("at_ms").parse().expect("a number");
        assert_eq!(entry["at_ms"], json!(at_ms), "the entry and the line agree");
        assert_eq!(entry["track"], json!("mic"));
        assert_eq!(entry["blocks"], json!(blocks));
        assert_eq!(entry["ms"], json!(span));
        assert_eq!(entry.as_object().expect("an object").len(), 4, "{entry}");
        assert!(
            at_ms > last_at || (last_at == 0 && std::ptr::eq(entry, &drops[0])),
            "drops are in order and the second is later than the first: {at_ms} after {last_at}"
        );
        assert!(at_ms <= 400, "at_ms is a position in the call: {at_ms}");
        last_at = at_ms;
    }
    let meta = rig.only_meta();
    assert_eq!(meta["state"], json!("ready-to-transcribe"));
    assert_eq!(
        meta["device_changes"],
        json!([]),
        "a drop is not a device change"
    );
    assert!(rig.lines_named("call_device").is_empty());
}

#[test]
fn a_system_drop_names_the_system_track() {
    // Decision #244 (I7), contract literal `call_track_dropped track=<mic|system>`: a
    // drop on the system track is logged and stored as `system`, and no mic entry
    // appears. Same mechanics as the mic test: the writer is held while the burst is fed,
    // so the channel (capacity 4) drops exactly the blocks past the fourth (3 of 20 ms).
    // Bite: `Writer::apply` passing `Track::Mic` (or any fixed track) to `self.dropped`.
    let rig = CallRig::with_capacity(4, |_| {});
    started(&rig, CallSource::Tray);

    let pause = rig.session.pause_writer();
    rig.source.feed(Track::System, 0, 140, 0.2);
    drop(pause);
    assert!(
        wait(|| meta_drops(&rig).len() == 1 && rig.lines_named("call_track_dropped").len() == 1),
        "the drop reaches the log and the meta: {:?} / {:?}",
        meta_drops(&rig),
        rig.lines_named("call_track_dropped")
    );
    rig.session.stop(StopReason::User);

    let lines = rig.lines_named("call_track_dropped");
    let drops = meta_drops(&rig);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].get("track"), Some("system"), "{}", lines[0].raw);
    assert_eq!(lines[0].get("blocks"), Some("3"));
    assert_eq!(drops.len(), 1, "no entry for the mic: {drops:?}");
    assert_eq!(drops[0]["track"], json!("system"));
    assert_eq!(drops[0]["blocks"], json!(3));
    assert_eq!(drops[0]["ms"], json!(60));
}

#[test]
fn a_gap_without_a_device_change_leaves_no_line_and_no_entry() {
    // Decision #244: Report::Gap gives neither a call_track_dropped line nor a drops[]
    // entry (a gap tied to a device change is call_device, a later task). The mic stalls
    // for 500 ms between two blocks (a stamp jump, no channel drop). Bite: a Gap logged
    // or stored as a drop, a Gap recorded as a device change.
    // A capacity above the 65 blocks fed in one burst: no channel drop under load.
    let rig = CallRig::with_capacity(256, |_| {});
    started(&rig, CallSource::Tray);
    rig.source.feed(Track::System, 0, 1_300, 0.1);
    rig.source.feed(Track::Mic, 0, 400, 0.3);
    rig.source.feed(Track::Mic, 900, 1_300, 0.3);
    rig.session.stop(StopReason::User);
    assert!(rig.lines_named("call_track_dropped").is_empty());
    assert!(rig.lines_named("call_device").is_empty());
    let meta = rig.only_meta();
    assert_eq!(meta["drops"], json!([]));
    assert_eq!(meta["device_changes"], json!([]));
    assert_eq!(meta["state"], json!("ready-to-transcribe"));
}

#[test]
fn a_call_without_drops_has_an_empty_drops_list() {
    // Decision #244: `drops` is written as [] at create and stays [] after a clean stop.
    // Bite: the field absent or null at the start.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    assert_eq!(rig.only_meta()["drops"], json!([]));
    feed(&rig, 0, 400);
    rig.session.stop(StopReason::User);
    assert_eq!(rig.only_meta()["drops"], json!([]));
    assert!(rig.lines_named("call_track_dropped").is_empty());
}

/// Makes the next `meta.json` write of the only call folder fail: the file becomes a
/// directory, so the atomic rename onto it is refused (on every OS).
fn block_meta_writes(rig: &CallRig) {
    let folders = rig.folders();
    assert_eq!(folders.len(), 1, "premise: one call folder");
    std::fs::remove_file(folders[0].join("meta.json")).expect("remove meta.json");
    std::fs::create_dir(folders[0].join("meta.json")).expect("a directory named meta.json");
}

fn storage_failed_kinds(rig: &CallRig) -> Vec<String> {
    rig.lines_named("call_storage_failed")
        .iter()
        .map(|l| l.get("kind").unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_failed_final_meta_write_is_logged_and_the_stop_still_completes() {
    // The stop writes meta.json last; when that write fails the session logs
    // `call_storage_failed kind=meta`, still logs `call_stopped`, turns the indicator off
    // and reports the stop (the failure is not swallowed and not fatal). Bite: the error
    // dropped at the `write_meta` call in `CallSession::finish` (no line), the stop
    // abandoned after it (no call_stopped, indicator left on).
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 200);
    block_meta_writes(&rig);
    assert!(rig.session.stop(StopReason::User));
    assert_eq!(storage_failed_kinds(&rig), vec!["meta".to_string()]);
    assert_eq!(rig.lines_named("call_stopped").len(), 1);
    assert_eq!(rig.indicator.recording_flags().last(), Some(&false));
}

#[test]
fn a_failed_meta_write_at_a_drop_is_logged_and_the_call_goes_on() {
    // #244: the drop's meta write fails once the file is blocked; the line for the drop
    // is still written, `call_storage_failed kind=meta` follows while the call runs, and
    // the call keeps recording. Bite: the write error dropped in `Writer::dropped`
    // (no call_storage_failed until the stop), the drop line skipped after the failure.
    let rig = CallRig::with_capacity(4, |_| {});
    started(&rig, CallSource::Tray);
    block_meta_writes(&rig);
    let pause = rig.session.pause_writer();
    rig.source.feed(Track::Mic, 0, 140, 0.2);
    drop(pause);
    assert!(
        wait(|| !storage_failed_kinds(&rig).is_empty()
            && rig.lines_named("call_track_dropped").len() == 1),
        "the drop line and the failed write reach the log while recording: {:?}",
        rig.call_lines()
    );
    assert_eq!(storage_failed_kinds(&rig)[0], "meta");
    assert!(rig.session.is_recording(), "the call goes on");
    assert!(rig.session.stop(StopReason::User));
}

// ---- the narrow settings writer ---------------------------------------------------------

struct Settings {
    file: Arc<FakeSettingsFile>,
    service: SettingsService,
}

fn real_service() -> Settings {
    let bytes = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let file = Arc::new(FakeSettingsFile::with_bytes(&bytes));
    let (service, _) = SettingsService::load_or_init(
        SettingsDeps {
            file: file.clone(),
            credentials: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(FakeCallRoots::new()),
        },
        OS,
    );
    Settings { file, service }
}

fn host() -> voicen_core::settings::url::ProviderHost {
    let url = check_base_url("https://api.example.com/v1").expect("a valid url");
    provider_host(&url).expect("a remote host")
}

#[test]
fn remember_warned_host_writes_the_host_and_nothing_else() {
    // Acceptance 8 (I5, decision "narrow writer"): `SettingsService::remember_warned_host`
    // adds the host to `calls.warned_hosts`, writes the file once, publishes a new
    // snapshot, changes no other field, and is the `CallSettingsPort` of the real
    // service. Asking again for the same host writes nothing. Bite: a second writer path
    // that skips save_lock's file write (snapshot only, the host lost at restart), a
    // duplicate entry, another field touched, a write per call.
    let world = real_service();
    let before = world.service.snapshot();
    let writes_before = world.file.calls().len();
    let port: &dyn CallSettingsPort = &world.service;
    assert_eq!(port.remember_warned_host(&host()), Ok(()));
    let after = world.service.snapshot();
    assert_eq!(
        after.calls.warned_hosts,
        vec!["api.example.com".to_string()]
    );
    let mut expected = (*before).clone();
    expected.calls.warned_hosts = vec!["api.example.com".to_string()];
    assert_eq!(*after, expected, "only warned_hosts changed");
    assert_eq!(
        world.file.calls().len(),
        writes_before + 1,
        "one file write"
    );
    let stored: Value = serde_json::from_slice(&world.file.bytes().expect("a file")).expect("json");
    assert_eq!(stored["calls"]["warned_hosts"], json!(["api.example.com"]));
    assert_eq!(port.snapshot().calls.warned_hosts, after.calls.warned_hosts);

    assert_eq!(port.remember_warned_host(&host()), Ok(()));
    assert_eq!(
        world.file.calls().len(),
        writes_before + 1,
        "a known host writes nothing"
    );
    assert_eq!(world.service.snapshot().calls.warned_hosts.len(), 1);
}

#[test]
fn remember_warned_host_keeps_the_snapshot_when_the_write_fails() {
    // Acceptance 8, failure branch: the file write fails (once): the call returns
    // WarnedHostFailed, the snapshot in force is unchanged, the file is unchanged, and a
    // later call (the fake failed once) succeeds. Bite: the snapshot swapped before the
    // write, the error swallowed into Ok, the failure sticky.
    let world = real_service();
    let before = world.service.snapshot();
    let file_before = world.file.bytes();
    world.file.fail_write(io::ErrorKind::PermissionDenied);
    assert_eq!(
        world.service.remember_warned_host(&host()),
        Err(WarnedHostFailed)
    );
    assert_eq!(
        *world.service.snapshot(),
        *before,
        "the snapshot is unchanged"
    );
    assert_eq!(world.file.bytes(), file_before, "the file is unchanged");
    world.file.clear_failures();
    assert_eq!(world.service.remember_warned_host(&host()), Ok(()));
    assert_eq!(
        world.service.snapshot().calls.warned_hosts,
        vec!["api.example.com".to_string()]
    );
}

#[test]
fn remember_warned_host_writes_nothing_while_settings_are_unavailable() {
    // docs/decisions/settings.md "`Unavailable` (#19) writes nothing": with the settings
    // file unreadable at load, `remember_warned_host` returns WarnedHostFailed, the file
    // sees no write (only the load's read), and the snapshot in force is unchanged.
    // Bite: `SettingsService::remember_warned_host` without its `self.load.unavailable`
    // guard (the in-memory defaults written over the unreadable file).
    let file = Arc::new(FakeSettingsFile::new());
    file.fail_read(io::ErrorKind::PermissionDenied);
    let (service, outcome) = SettingsService::load_or_init(
        SettingsDeps {
            file: file.clone(),
            credentials: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(FakeCallRoots::new()),
        },
        OS,
    );
    assert!(
        matches!(outcome, voicen_core::settings::LoadOutcome::Unavailable(_)),
        "precondition: {outcome:?}"
    );
    let before = service.snapshot();
    let calls_before = file.calls();
    assert_eq!(service.remember_warned_host(&host()), Err(WarnedHostFailed));
    assert_eq!(file.calls(), calls_before, "no file call, no write");
    assert_eq!(file.bytes(), None, "the unreadable file is not replaced");
    assert_eq!(*service.snapshot(), *before, "the snapshot is unchanged");
}

#[test]
fn a_later_save_keeps_a_remembered_host() {
    // I5: the host written by the narrow writer survives an ordinary settings save (the
    // save keeps server-owned fields from the snapshot). Bite: the save resetting
    // warned_hosts to the draft's list.
    use voicen_core::secrets::KeyEdits;
    use voicen_core::settings::service::{SaveOutcome, SaveRequest};
    let world = real_service();
    assert_eq!(world.service.remember_warned_host(&host()), Ok(()));
    let mut draft = (*world.service.snapshot()).clone();
    draft.calls.warned_hosts = Vec::new();
    draft.auto_paste = !draft.auto_paste;
    let outcome = world.service.save(SaveRequest {
        settings: draft,
        keys: KeyEdits::default(),
    });
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");
    assert_eq!(
        world.service.snapshot().calls.warned_hosts,
        vec!["api.example.com".to_string()]
    );
}

#[test]
fn indicator_calls_are_the_closed_set() {
    // I4: the start publishes exactly the state and the recording flag, once each, in
    // that set of calls (no tick before any audio). Bite: a tick published at the start,
    // a second state for the same change.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    let calls = rig.indicator.calls();
    let kinds: Vec<&str> = calls
        .iter()
        .map(|c| match c {
            IndicatorCall::State(_) => "state",
            IndicatorCall::Tick(_) => "tick",
            IndicatorCall::Recording(_) => "recording",
        })
        .collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == "state").count(),
        1,
        "{kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "recording").count(),
        1,
        "{kinds:?}"
    );
    assert!(!kinds.contains(&"tick"), "{kinds:?}");
    rig.session.stop(StopReason::User);
}

// ---- T-130: dismissing a warning --------------------------------------------------------
//
// Assumed API (T-130 red tests; the developer implements exactly this):
//   CallSession::dismiss_warning(&self, id: u32) -> bool
// Under the session lock: removes the warning with that id from the call's warnings and
// publishes one new `IndicatorState` (same reminder state, the other warnings in order);
// `true` when a warning was removed. An unknown id, an id already dismissed, or no
// recording call: `false`, nothing published.

/// A rig whose start has two warnings: `MicChanged` (id 1) and, on the new host,
/// `ProviderNotice` (id 2).
fn two_warning_rig() -> CallRig {
    let rig = CallRig::with(|s| {
        s.microphone = Some(Microphone {
            id: "gone-mic".to_string(),
            name: "Unplugged".to_string(),
        });
    });
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    rig
}

fn warning_ids(state: &IndicatorState) -> Vec<u32> {
    state.warnings.iter().map(|w| w.id).collect()
}

#[test]
fn dismiss_warning_removes_it_until_reminder_end_republishes() {
    // T-130 Acceptance (commands: call_indicator_dismiss -> CallSession::dismiss_warning):
    // dismissing id 1 republishes the state inside the session lock without that warning
    // and with the reminder untouched; an unknown or repeated id changes and publishes
    // nothing; the reminder's end (`tick`) republishes a state that still lacks the
    // dismissed warning (the session's own copy was edited, not only the published one);
    // dismissing the last warning leaves an empty list. Bite: a dismiss that publishes
    // without editing `Call.warnings` (the warning returns at the reminder's end), that
    // edits without publishing, that removes by index instead of id, that publishes for an
    // unknown id, that publishes outside the lock, that touches the reminder or the
    // recording flag.
    let rig = two_warning_rig();
    started(&rig, CallSource::Tray);
    let first = rig.indicator.states().last().cloned().expect("a state");
    assert_eq!(warning_ids(&first), vec![1, 2], "premise: {first:?}");
    assert_eq!(first.warnings[0].kind, WarningKind::MicChanged);
    assert_eq!(first.warnings[1].kind, WarningKind::ProviderNotice);
    assert!(first.reminder_visible, "premise: the reminder shows");

    assert!(rig.session.dismiss_warning(1));
    let states = rig.indicator.states();
    assert_eq!(states.len(), 2, "one state per dismissal: {states:?}");
    let second = &states[1];
    assert!(second.recording && second.reminder_visible);
    assert_eq!(second.reminder_text, first.reminder_text);
    assert_eq!(second.headphones_hint, first.headphones_hint);
    assert_eq!(second.warnings, vec![first.warnings[1].clone()]);
    assert!(
        rig.indicator.held_flags().iter().all(|held| *held),
        "the dismissal state is published inside the session lock: {:?}",
        rig.indicator.calls()
    );
    assert_eq!(rig.indicator.recording_flags(), vec![true]);
    assert!(rig.indicator.ticks().is_empty());

    assert!(!rig.session.dismiss_warning(1), "already dismissed");
    assert!(!rig.session.dismiss_warning(99), "unknown id");
    assert_eq!(rig.indicator.states().len(), 2, "nothing published");

    rig.mono.advance(ms(10_000));
    rig.session.tick();
    let states = rig.indicator.states();
    assert_eq!(states.len(), 3, "{states:?}");
    assert!(!states[2].reminder_visible);
    assert_eq!(
        warning_ids(&states[2]),
        vec![2],
        "the reminder's end must not bring the dismissed warning back"
    );

    assert!(rig.session.dismiss_warning(2));
    let last = rig.indicator.states().last().cloned().expect("a state");
    assert!(last.recording && !last.reminder_visible);
    assert!(last.warnings.is_empty(), "{last:?}");
    rig.session.stop(StopReason::User);
}

#[test]
fn dismiss_warning_without_a_recording_call_changes_nothing() {
    // T-130: no call, or a call that was stopped: `false` and no indicator call. Bite: a
    // dismiss that publishes a recording state after the stop (the window would be
    // rebuilt), or one that panics without a call.
    let rig = two_warning_rig();
    assert!(!rig.session.dismiss_warning(1));
    assert!(
        rig.indicator.calls().is_empty(),
        "{:?}",
        rig.indicator.calls()
    );
    started(&rig, CallSource::Tray);
    assert!(rig.session.stop(StopReason::User));
    let before = rig.indicator.calls();
    assert!(!rig.session.dismiss_warning(1));
    assert!(!rig.session.dismiss_warning(2));
    assert_eq!(
        rig.indicator.calls(),
        before,
        "nothing published after the stop"
    );
}

#[test]
fn a_dismissed_warning_does_not_outlive_its_call() {
    // T-130: dismissal belongs to the call. After a stop and a new start the new call's
    // warning with the same id (1) is shown again. Bite: a dismissed-ids set kept in the
    // session across calls.
    let rig = two_warning_rig();
    started(&rig, CallSource::Tray);
    assert!(rig.session.dismiss_warning(1));
    rig.session.stop(StopReason::User);
    rig.source.fail_next(Track::Mic, OpenError::NoDevice);
    started(&rig, CallSource::Tray);
    let state = rig.indicator.states().last().cloned().expect("a state");
    assert_eq!(warning_ids(&state), vec![1], "{state:?}");
    assert_eq!(state.warnings[0].kind, WarningKind::MicChanged);
    rig.session.stop(StopReason::User);
}

// ---- T-130: the indicator position writer -----------------------------------------------
//
// Assumed API:
//   SettingsService::remember_indicator_position(&self, position: IndicatorPosition)
//       -> Result<(), IndicatorPositionFailed>
//   settings::service::IndicatorPositionFailed (unit struct; Debug, Clone, Copy, PartialEq, Eq)
// The `remember_warned_host` pattern: `Unavailable` -> Err and no file call; under
// `save_lock`; the same position as in force writes nothing; otherwise only
// `calls.indicator_position` changes, one file write, then swap and publish; a write
// failure keeps the snapshot and publishes nothing.

fn pos(x: i32, y: i32) -> IndicatorPosition {
    IndicatorPosition { x, y }
}

#[test]
fn remember_indicator_position_writes_only_that_field() {
    let world = real_service();
    let before = world.service.snapshot();
    assert_eq!(
        before.calls.indicator_position, None,
        "premise: no position yet"
    );
    let writes_before = world.file.calls().len();
    assert_eq!(
        world.service.remember_indicator_position(pos(-5, 12)),
        Ok(())
    );
    let after = world.service.snapshot();
    assert_eq!(after.calls.indicator_position, Some(pos(-5, 12)));
    let mut expected = (*before).clone();
    expected.calls.indicator_position = Some(pos(-5, 12));
    assert_eq!(*after, expected, "only indicator_position changed");
    assert_eq!(
        world.file.calls().len(),
        writes_before + 1,
        "one file write"
    );
    let stored: Value = serde_json::from_slice(&world.file.bytes().expect("a file")).expect("json");
    assert_eq!(
        stored["calls"]["indicator_position"],
        json!({ "x": -5, "y": 12 })
    );

    assert_eq!(
        world.service.remember_indicator_position(pos(-5, 12)),
        Ok(())
    );
    assert_eq!(
        world.file.calls().len(),
        writes_before + 1,
        "the position in force writes nothing"
    );

    assert_eq!(
        world.service.remember_indicator_position(pos(300, 40)),
        Ok(())
    );
    assert_eq!(
        world.file.calls().len(),
        writes_before + 2,
        "a new position writes"
    );
    assert_eq!(
        world.service.snapshot().calls.indicator_position,
        Some(pos(300, 40))
    );
}

#[test]
fn remember_indicator_position_keeps_the_snapshot_when_the_write_fails() {
    // Failure branch: the file write fails (once, F-024): the call returns the typed
    // failure, the snapshot and the file are unchanged, a later call succeeds. Bite: the
    // error swallowed into Ok, the snapshot swapped before the write, the failure sticky.
    let world = real_service();
    let before = world.service.snapshot();
    let file_before = world.file.bytes();
    world.file.fail_write(io::ErrorKind::PermissionDenied);
    assert_eq!(
        world.service.remember_indicator_position(pos(10, 20)),
        Err(IndicatorPositionFailed)
    );
    assert_eq!(
        *world.service.snapshot(),
        *before,
        "the snapshot is unchanged"
    );
    assert_eq!(world.file.bytes(), file_before, "the file is unchanged");
    world.file.clear_failures();
    assert_eq!(
        world.service.remember_indicator_position(pos(10, 20)),
        Ok(())
    );
    assert_eq!(
        world.service.snapshot().calls.indicator_position,
        Some(pos(10, 20))
    );
}

#[test]
fn remember_indicator_position_writes_nothing_while_settings_are_unavailable() {
    // docs/decisions/settings.md "`Unavailable` writes nothing": no file call, snapshot
    // unchanged, typed failure. Bite: the `self.load.unavailable` guard missing.
    let file = Arc::new(FakeSettingsFile::new());
    file.fail_read(io::ErrorKind::PermissionDenied);
    let (service, outcome) = SettingsService::load_or_init(
        SettingsDeps {
            file: file.clone(),
            credentials: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(FakeCallRoots::new()),
        },
        OS,
    );
    assert!(
        matches!(outcome, voicen_core::settings::LoadOutcome::Unavailable(_)),
        "precondition: {outcome:?}"
    );
    let before = service.snapshot();
    let calls_before = file.calls();
    assert_eq!(
        service.remember_indicator_position(pos(1, 2)),
        Err(IndicatorPositionFailed)
    );
    assert_eq!(file.calls(), calls_before, "no file call, no write");
    assert_eq!(file.bytes(), None);
    assert_eq!(*service.snapshot(), *before);
}

#[test]
fn a_later_save_keeps_a_remembered_indicator_position() {
    // The position is server-owned: an ordinary save (draft without it) keeps it. Bite:
    // the save taking indicator_position from the draft.
    use voicen_core::secrets::KeyEdits;
    use voicen_core::settings::service::{SaveOutcome, SaveRequest};
    let world = real_service();
    assert_eq!(world.service.remember_indicator_position(pos(7, 9)), Ok(()));
    let mut draft = (*world.service.snapshot()).clone();
    draft.calls.indicator_position = None;
    draft.auto_paste = !draft.auto_paste;
    let outcome = world.service.save(SaveRequest {
        settings: draft,
        keys: KeyEdits::default(),
    });
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");
    assert_eq!(
        world.service.snapshot().calls.indicator_position,
        Some(pos(7, 9))
    );
}
