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

use call_support::{reopened_id, CallRig, IndicatorCall, ShellCall, BUDGET, E_NOTFOUND, OS};
use serde_json::{json, Value};
use voicen_core::autostart::FakeAutostart;
use voicen_core::call::codec::{scan, OpusOggReader, ReadEnd, TrackDecoder};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick, WarningKind};
use voicen_core::call::session::{CallSettingsPort, Toggled};
use voicen_core::call::{
    CallNotice, CallSource, DeviceEvent, ListState, LossCause, OpenError, RefusedPart,
    RefusedReason, StopReason, Track, TrackDevice,
};
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::platform::DeviceId;
use voicen_core::secrets::FakeCredentialStore;
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::roots::{FakeCallRoots, UnwritableReason};
use voicen_core::settings::service::{SettingsDeps, SettingsService, WarnedHostFailed};
use voicen_core::settings::url::{check_base_url, provider_host};
use voicen_core::settings::{defaults, Microphone, Settings as CoreSettings};

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
    // Subtitles on: the consumer's sample counts prove what the writer handled (`drained`).
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    assert!(
        rig.session.try_is_recording().is_some(),
        "the lock is free between calls"
    );
    // F-031: a tick proves only that one block of one track was handled after a 100 ms
    // mono step, and the writer empties one track's channel before it reads the other, so
    // a tick is never the proof that the mic lane was read. Feed one 200 ms span (10 blocks
    // per track, under the 64-slot capacity: no drop), wait until the writer has handled
    // all of it on both tracks, then take the tick that follows (no loop, no tick count).
    let both = [Track::Mic, Track::System];
    rig.source.feed_both(0, 200, 0.5, 0.0);
    rig.source.set_stamp_ms(200);
    drained(&rig, &both, 200);
    let tick = barrier(&rig, &[(Track::Mic, 0.5), (Track::System, 0.0)], 200);
    assert!(
        tick.mic_level > 0.1 && tick.system_level < 0.01,
        "the mic loud, the system quiet: {:?}",
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

/// T-144: while a call records, the shell's timer always has a deadline, never more than
/// 10 s ahead (the disk poll and the 4 h limit are decided in `tick`).
fn due_within_10s(rig: &CallRig) {
    let due = rig
        .session
        .next_deadline()
        .expect("a deadline is due while a call records");
    assert!(
        due <= rig.mono.read() + ms(10_000),
        "the deadline is at most 10 s ahead"
    );
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
    started(&rig, CallSource::Tray);
    let first = rig.indicator.states();
    assert_eq!(first.len(), 1);
    assert!(first[0].recording && first[0].reminder_visible);
    assert_eq!(
        first[0].reminder_text,
        "Tell the other party the call is being recorded"
    );
    assert_eq!(first[0].headphones_hint, "Better with headphones");
    due_within_10s(&rig);

    rig.mono.advance(ms(9_999));
    rig.session.tick();
    assert_eq!(
        rig.indicator.states().len(),
        1,
        "nothing changes before 10 s"
    );
    due_within_10s(&rig);

    rig.mono.advance(ms(1));
    rig.session.tick();
    let after = rig.indicator.states();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after[1].recording && !after[1].reminder_visible);
    assert_eq!(after[1].reminder_text, "");
    assert_eq!(after[1].headphones_hint, "");
    due_within_10s(&rig);
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
    started(&rig, CallSource::Hotkey);
    let again = rig.indicator.states();
    let last = again.last().expect("a state");
    assert!(
        last.recording && last.reminder_visible,
        "shown again at the next start"
    );
    due_within_10s(&rig);
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
    due_within_10s(&rig);
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

// ---- T-149: device changes and silent-side warnings -------------------------------------
//
// Rules that bind these tests: F-018 (waits poll state: the device thread is observed
// through `FakeDevices::polls`, the writer through the subtitle consumer and the indicator
// ticks, never through a sleep), F-021 (the 2 s and 60 s rules run on the fake monotonic
// clock and on stamps; no wall-clock ceiling), F-024 (every error fake fails exactly one
// call, except `fail_all`, used only where the guarantee is about repeated failures: the
// retries every 2 s).

fn quiet(s: &mut CoreSettings) {
    s.calls.warned_hosts = vec!["api.example.com".to_string()];
}

fn quiet_with_mic(s: &mut CoreSettings) {
    quiet(s);
    s.microphone = Some(Microphone {
        id: "mic-1".to_string(),
        name: "Desk mic".to_string(),
    });
}

fn kind_count(rig: &CallRig, kind: WarningKind) -> usize {
    rig.indicator
        .states()
        .last()
        .map_or(0, |s| s.warnings.iter().filter(|w| w.kind == kind).count())
}

fn warning_ids(rig: &CallRig, kind: WarningKind) -> Vec<u32> {
    rig.indicator.states().last().map_or(Vec::new(), |s| {
        s.warnings
            .iter()
            .filter(|w| w.kind == kind)
            .map(|w| w.id)
            .collect()
    })
}

/// Every warning kind of the last published state except the provider notice.
fn warnings_but_provider(rig: &CallRig) -> Vec<WarningKind> {
    warnings_of(rig)
        .into_iter()
        .filter(|k| *k != WarningKind::ProviderNotice)
        .collect()
}

/// The device thread has gone round at least four more times.
fn polled(rig: &CallRig) {
    let before = rig.devices.polls();
    assert!(
        wait(|| rig.devices.polls() >= before + 4),
        "the device thread keeps polling"
    );
}

/// Moves the session's monotonic clock and waits until the device thread has read it.
fn advance(rig: &CallRig, d: Duration) {
    rig.mono.advance(d);
    polled(rig);
}

/// The consumer has all `upto_ms` of `tracks`, so the writer handled those blocks. Needs
/// subtitles on.
fn drained(rig: &CallRig, tracks: &[Track], upto_ms: u64) {
    let want = fed(0, upto_ms);
    for track in tracks {
        assert!(
            wait(|| rig.subtitles.samples(*track) >= want),
            "the writer took {track:?} up to {upto_ms} ms"
        );
    }
}

/// A tick made after the blocks fed here: ticks are made when a block is handled once
/// 100 ms of monotonic time passed, so the clock moves and each of `feeds` (track, amplitude)
/// gets one more 20 ms block at `at` ms (the next block of its stream). A tick proves only
/// that one block of one track was handled, so this proves the consumption of earlier
/// blocks only when the queues were empty before it: call it right after `drained()`.
fn barrier(rig: &CallRig, feeds: &[(Track, f32)], at: u64) -> IndicatorTick {
    let n = rig.indicator.ticks().len();
    rig.mono.advance(ms(150));
    for (track, amp) in feeds {
        rig.source.feed(*track, at, at + 20, *amp);
    }
    assert!(
        wait(|| rig.indicator.ticks().len() > n),
        "a tick follows the writer's round"
    );
    rig.indicator.ticks().last().cloned().expect("a tick")
}

fn meta_changes(rig: &CallRig) -> Vec<Value> {
    let folders = rig.folders();
    let Some(dir) = folders.first() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(dir.join("meta.json")) else {
        return Vec::new();
    };
    let meta: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    meta["device_changes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A stored track decoded to samples in [-1, 1].
fn track_samples(rig: &CallRig, file: &str) -> Vec<f32> {
    let dir = rig.folders().first().cloned().expect("a call folder");
    let mut reader =
        OpusOggReader::open(File::open(dir.join(file)).expect(file)).expect("a readable track");
    let mut out = Vec::new();
    while let Some(window) = reader.next_window(1_000).expect("the track decodes") {
        out.extend(window.samples().iter().map(|s| f32::from(*s) / 32_768.0));
    }
    out
}

fn rms_between(samples: &[f32], from_ms: usize, to_ms: usize) -> f32 {
    let part = samples
        .get(from_ms * 16..to_ms * 16)
        .expect("the track reaches there");
    (part.iter().map(|x| x * x).sum::<f32>() / part.len() as f32).sqrt()
}

#[derive(Debug, Clone, Copy)]
enum Trigger {
    /// The shell reports the change (`DeviceEvents`).
    Event,
    /// The stream itself dies (`TrackSink::lost`).
    Loss,
}

fn device_lines(rig: &CallRig) -> Vec<voicen_core_support::Line> {
    rig.lines_named("call_device")
}

mod voicen_core_support {
    pub use crate::diag_support::Line;
}

#[test]
fn a_default_output_change_reopens_the_system_track_with_its_gap_in_meta_and_log() {
    // Acceptance 1 (T068/T069): a default-output change, whether the shell reports it or the
    // stream dies, reopens the system track on the new default (the old handle is stopped
    // before the new one opens, never two at once); the gap (<= 2 s) is silence in the stored
    // track and appears once in `call_device track=system cause=default_output_changed
    // gap_ms= to=<new id>` and once in meta.json `device_changes`, written while the call
    // runs; the mic is not touched, the call is not stopped, nothing is warned.
    // Bite: no reopen; the new open before the stop of the old one; gap_ms 0 or taken from
    // Report::Gap (silent under 200 ms); the line without the entry or the entry only at
    // stop; the entry's `to` the old id; a duplicate line for the revive's own Gap; a
    // reopen that stops the call; the mic reopened too.
    for trigger in [Trigger::Event, Trigger::Loss] {
        for gap in [120u64, 400, 1_900] {
            let tag = format!("{trigger:?}, gap {gap} ms");
            let rig = CallRig::with_capacity(1_024, quiet);
            started(&rig, CallSource::Tray);
            rig.source.feed_tone(Track::System, 0, 1_000, 0.2);
            rig.source.feed(Track::Mic, 0, 1_000, 0.3);
            rig.source.set_stamp_ms(1_000);
            match trigger {
                Trigger::Event => rig.devices.push(DeviceEvent::DefaultOutputChanged),
                Trigger::Loss => rig
                    .source
                    .lose(Track::System, LossCause::NotAvailable, 1_000),
            }
            assert!(
                wait(|| rig.source.ok_opens(Track::System) == 2),
                "{tag}: the system track is reopened"
            );
            let first = 1_000 + gap;
            rig.source.feed_tone(Track::System, first, first + 200, 0.2);
            assert!(
                wait(|| device_lines(&rig).len() == 1 && meta_changes(&rig).len() == 1),
                "{tag}: one line and one entry while the call runs: {:?} / {:?}",
                device_lines(&rig),
                meta_changes(&rig)
            );
            rig.source.feed(Track::Mic, 1_000, first + 200, 0.3);
            let end = first + 800;
            let mut t = first + 200;
            while t < end {
                rig.source.feed_tone(Track::System, t, t + 100, 0.2);
                rig.source.feed(Track::Mic, t, t + 100, 0.3);
                t += 100;
            }
            assert!(rig.session.is_recording(), "{tag}: the call goes on");
            rig.session.stop(StopReason::User);

            let trace = rig.source.trace();
            let stop_old = trace
                .iter()
                .position(|e| e == "stop system")
                .expect("stopped");
            let reopen = trace
                .iter()
                .enumerate()
                .filter(|(_, e)| *e == "open system")
                .map(|(i, _)| i)
                .nth(1)
                .expect("opened again");
            assert!(stop_old < reopen, "{tag}: stop before open: {trace:?}");
            assert_eq!(rig.source.peak_handles(Track::System), 1, "{tag}");
            assert_eq!(
                rig.source.ok_opens(Track::Mic),
                1,
                "{tag}: the mic is left alone"
            );
            assert_eq!(
                rig.source.opens()[2],
                (Track::System, TrackDevice::DefaultOutputLoopback),
                "{tag}"
            );

            let lines = device_lines(&rig);
            assert_eq!(lines.len(), 1, "{tag}: {lines:?}");
            let line = &lines[0];
            assert_eq!(line.get("track"), Some("system"), "{tag}");
            assert_eq!(line.get("cause"), Some("default_output_changed"), "{tag}");
            let new_id = reopened_id(Track::System, 2);
            assert_eq!(line.get("to"), Some(new_id.as_str()), "{tag}");
            assert!(!line.raw.contains(" WARN "), "{tag}: {}", line.raw);
            let gap_ms: u64 = line
                .get("gap_ms")
                .expect("gap_ms")
                .parse()
                .expect("a number");
            assert!(
                gap_ms + 25 >= gap && gap_ms <= gap + 25,
                "{tag}: gap_ms is the silence inserted: {gap_ms}"
            );

            let changes = meta_changes(&rig);
            assert_eq!(changes.len(), 1, "{tag}: {changes:?}");
            let entry = &changes[0];
            assert_eq!(entry["track"], json!("system"), "{tag}");
            assert_eq!(entry["cause"], json!("default_output_changed"), "{tag}");
            assert_eq!(entry["to"], json!(new_id), "{tag}");
            assert_eq!(
                entry["gap_ms"],
                json!(gap_ms),
                "{tag}: the line and the entry agree"
            );
            let at_ms = entry["at_ms"].as_u64().expect("at_ms");
            assert!(
                (950..=1_050).contains(&at_ms),
                "{tag}: where it happened: {at_ms}"
            );
            assert_eq!(
                entry.as_object().expect("object").len(),
                5,
                "{tag}: {entry}"
            );

            assert!(rig.lines_named("call_track_silent").is_empty(), "{tag}");
            assert!(rig.lines_named("call_track_dropped").is_empty(), "{tag}");
            assert!(warnings_but_provider(&rig).is_empty(), "{tag}: no warning");
            let meta = rig.only_meta();
            assert_eq!(meta["state"], json!("ready-to-transcribe"), "{tag}");
            let duration = meta["duration_ms"].as_u64().expect("a duration");
            assert!(
                duration + 200 >= end && duration <= end + 100,
                "{tag}: the call covers the gap: {duration} vs {end}"
            );

            let system = track_samples(&rig, "system.opus");
            assert!(rms_between(&system, 100, 900) > 0.03, "{tag}: audio before");
            if gap >= 400 {
                let gap_from = 1_150usize;
                let gap_to = usize::try_from(first).expect("fits") - 150;
                assert!(
                    rms_between(&system, gap_from, gap_to) < 0.005,
                    "{tag}: the gap is silence"
                );
            }
            let after = usize::try_from(first).expect("fits");
            assert!(
                rms_between(&system, after + 150, after + 700) > 0.03,
                "{tag}: audio after"
            );
        }
    }
}

#[test]
fn a_default_output_event_and_its_loss_reopen_once() {
    // Acceptance 1 / D1: one change, two signals. The reopen is running (the open is held
    // like a slow device) when the shell's event for the same change arrives; the new open
    // is already on the new default, so the event is absorbed: one reopen, one line, one
    // entry. Bite: a second reopen for the second signal (three opens, two lines), a trigger
    // queue that is not absorbed after the open, two handles at once.
    let rig = CallRig::with_capacity(1_024, quiet);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 1_000);
    rig.source.set_stamp_ms(1_000);
    rig.source.hold_opens();
    rig.source
        .lose(Track::System, LossCause::NotAvailable, 1_000);
    assert!(
        wait(|| rig.source.blocked_opens() == 1),
        "the reopen is running"
    );
    rig.devices.push(DeviceEvent::DefaultOutputChanged);
    rig.source.release_opens();
    assert!(wait(|| rig.source.ok_opens(Track::System) == 2));
    assert!(wait(|| rig.devices.pending() == 0), "the event was taken");
    rig.source.feed(Track::System, 1_200, 1_400, 0.1);
    assert!(wait(|| device_lines(&rig).len() == 1));
    polled(&rig);
    rig.source.feed(Track::System, 1_400, 1_800, 0.1);
    rig.source.feed(Track::Mic, 1_000, 1_800, 0.3);
    polled(&rig);
    rig.session.stop(StopReason::User);
    assert_eq!(rig.source.ok_opens(Track::System), 2, "one reopen");
    assert_eq!(rig.source.attempts(Track::System), 2, "one reopen attempt");
    assert_eq!(rig.source.peak_handles(Track::System), 1);
    assert_eq!(device_lines(&rig).len(), 1, "{:?}", device_lines(&rig));
    assert_eq!(meta_changes(&rig).len(), 1, "{:?}", meta_changes(&rig));
}

#[test]
fn a_lost_selected_mic_reopens_on_the_default_input_with_mic_changed_and_keeps_recording() {
    // Acceptance 2 (T068): the selected mic is removed (the shell says so, or its stream
    // dies): the mic track reopens on the default input, the indicator shows
    // `micChanged`, `call_device track=mic cause=mic_lost` and one `device_changes` entry
    // say so, the call is not stopped and the system track is not touched.
    // Bite: no reopen; a reopen on `Selected` again; no MicChanged warning; the call
    // stopped; the system track reopened; the cause `track_lost`.
    for trigger in [Trigger::Event, Trigger::Loss] {
        let tag = format!("{trigger:?}");
        let rig = CallRig::with_capacity(1_024, quiet_with_mic);
        started(&rig, CallSource::Tray);
        feed(&rig, 0, 1_000);
        rig.source.set_stamp_ms(1_000);
        match trigger {
            Trigger::Event => rig
                .devices
                .push(DeviceEvent::DeviceRemoved(DeviceId("mic-1".to_string()))),
            Trigger::Loss => rig.source.lose(Track::Mic, LossCause::NotAvailable, 1_000),
        }
        assert!(
            wait(|| rig.source.ok_opens(Track::Mic) == 2),
            "{tag}: the mic is reopened"
        );
        rig.source.feed(Track::Mic, 1_200, 1_400, 0.3);
        assert!(wait(|| device_lines(&rig).len() == 1), "{tag}");
        assert!(
            wait(|| kind_count(&rig, WarningKind::MicChanged) == 1),
            "{tag}: the notice is shown: {:?}",
            warnings_of(&rig)
        );
        rig.source.feed(Track::System, 1_000, 1_400, 0.1);
        assert!(rig.session.is_recording(), "{tag}: the call is not stopped");
        assert!(rig.lines_named("call_stopped").is_empty(), "{tag}");
        rig.session.stop(StopReason::User);

        assert_eq!(
            rig.source.opens(),
            vec![
                (Track::System, TrackDevice::DefaultOutputLoopback),
                (
                    Track::Mic,
                    TrackDevice::Selected(DeviceId("mic-1".to_string()))
                ),
                (Track::Mic, TrackDevice::DefaultInput),
            ],
            "{tag}"
        );
        let trace = rig.source.trace();
        let stop_old = trace.iter().position(|e| e == "stop mic").expect("stopped");
        let reopen = trace.iter().rposition(|e| e == "open mic").expect("opened");
        assert!(stop_old < reopen, "{tag}: stop before open: {trace:?}");
        let lines = device_lines(&rig);
        assert_eq!(lines.len(), 1, "{tag}: {lines:?}");
        assert_eq!(lines[0].get("track"), Some("mic"), "{tag}");
        assert_eq!(lines[0].get("cause"), Some("mic_lost"), "{tag}");
        let new_id = reopened_id(Track::Mic, 2);
        assert_eq!(lines[0].get("to"), Some(new_id.as_str()), "{tag}");
        let changes = meta_changes(&rig);
        assert_eq!(changes.len(), 1, "{tag}: {changes:?}");
        assert_eq!(changes[0]["track"], json!("mic"), "{tag}");
        assert_eq!(changes[0]["cause"], json!("mic_lost"), "{tag}");
        assert_eq!(changes[0]["to"], json!(new_id), "{tag}");
        assert_eq!(
            rig.only_meta()["state"],
            json!("ready-to-transcribe"),
            "{tag}"
        );
    }
}

#[test]
fn a_default_mic_follows_a_default_input_change() {
    // Acceptance 2 / FR-012 ("loss or change ... with a notice"): the mic is on "default"
    // (no saved microphone) and the default input changes: the mic reopens on the default
    // input, `call_device track=mic cause=mic_default_changed`, the notice shows, the call
    // goes on. Bite: the event ignored; the cause `mic_lost`; no notice.
    let rig = CallRig::with_capacity(1_024, quiet);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 1_000);
    rig.source.set_stamp_ms(1_000);
    rig.devices.push(DeviceEvent::DefaultInputChanged);
    assert!(wait(|| rig.source.ok_opens(Track::Mic) == 2));
    rig.source.feed(Track::Mic, 1_200, 1_400, 0.3);
    assert!(wait(|| device_lines(&rig).len() == 1));
    assert!(wait(|| kind_count(&rig, WarningKind::MicChanged) == 1));
    rig.session.stop(StopReason::User);
    assert_eq!(
        rig.source.opens().last(),
        Some(&(Track::Mic, TrackDevice::DefaultInput))
    );
    let lines = device_lines(&rig);
    assert_eq!(lines[0].get("track"), Some("mic"));
    assert_eq!(lines[0].get("cause"), Some("mic_default_changed"));
    assert_eq!(meta_changes(&rig)[0]["cause"], json!("mic_default_changed"));
    assert_eq!(rig.source.ok_opens(Track::System), 1);
}

#[test]
fn events_that_do_not_concern_the_open_devices_change_nothing() {
    // D1: a selected mic is explicit, so a default-input change does not touch it; the
    // removal of a device that is not in use does not either. Bite: every event reopening
    // something; the selected mic reopened on a default-input change.
    let rig = CallRig::with_capacity(1_024, quiet_with_mic);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 400);
    rig.devices.push(DeviceEvent::DefaultInputChanged);
    rig.devices.push(DeviceEvent::DeviceRemoved(DeviceId(
        "not-in-use".to_string(),
    )));
    assert!(wait(|| rig.devices.pending() == 0), "the events were taken");
    polled(&rig);
    advance(&rig, ms(2_500));
    assert_eq!(rig.source.attempts(Track::Mic), 1);
    assert_eq!(rig.source.attempts(Track::System), 1);
    assert!(device_lines(&rig).is_empty());
    assert!(meta_changes(&rig).is_empty());
    assert!(warnings_but_provider(&rig).is_empty());
    assert_eq!(rig.source.open_handles(), 2);
    rig.session.stop(StopReason::User);
}

#[test]
fn a_track_not_reopened_within_2_s_is_silent_warned_and_retried_every_2_s() {
    // Acceptance 3, failure branch (T068, FR-012, F-021): the system stream dies and every
    // open fails (Busy). Nothing is declared at 1.9 s; at 2 s: one `track_lost` warning
    // (published), one `call_track_silent track=system rule=lost` (WARN), one
    // `call_device cause=track_lost to=none` (WARN) with its entry; then one more open
    // attempt at 4 s and one at 6 s, none in between, and no second warning or line. When
    // the device is back the next attempt reopens it (`cause=reopened`, to=<new id>); the
    // warning stays until dismissed. The stored system track is zeros from the loss to the
    // new audio, and the call covers it. Bite: the declaration at 1 s or at 3 s; declared
    // without the warning, the line or the entry; the retries missing (opens stay at the
    // first attempt) or too fast; a warning per retry; the warning removed by the reopen;
    // the file short (no zeros while lost).
    let rig = CallRig::with_capacity(2_048, quiet);
    started(&rig, CallSource::Tray);
    rig.source.feed_tone(Track::System, 0, 1_000, 0.2);
    rig.source.feed(Track::Mic, 0, 1_000, 0.3);
    rig.source.set_stamp_ms(1_000);
    rig.source.fail_all(Track::System, OpenError::Busy);
    rig.source
        .lose(Track::System, LossCause::NotAvailable, 1_000);
    rig.source.feed(Track::Mic, 1_000, 9_000, 0.3);
    assert!(
        wait(|| rig.source.attempts(Track::System) >= 2),
        "the first attempt comes at once"
    );

    advance(&rig, ms(1_900));
    assert!(rig.lines_named("call_track_silent").is_empty());
    assert!(device_lines(&rig).is_empty());
    assert!(warnings_but_provider(&rig).is_empty());

    advance(&rig, ms(100));
    assert!(
        wait(|| {
            rig.lines_named("call_track_silent").len() == 1
                && device_lines(&rig).len() == 1
                && kind_count(&rig, WarningKind::TrackLost) == 1
                && meta_changes(&rig).len() == 1
        }),
        "silent at 2 s: {:?} / {:?} / {:?}",
        rig.lines_named("call_track_silent"),
        device_lines(&rig),
        warnings_of(&rig)
    );
    let silent = &rig.lines_named("call_track_silent")[0];
    assert_eq!(silent.get("track"), Some("system"));
    assert_eq!(silent.get("rule"), Some("lost"));
    assert!(silent.raw.contains(" WARN "), "{}", silent.raw);
    let changed = &device_lines(&rig)[0];
    assert_eq!(changed.get("track"), Some("system"));
    assert_eq!(changed.get("cause"), Some("track_lost"));
    assert_eq!(changed.get("to"), Some("none"));
    assert!(changed.raw.contains(" WARN "), "{}", changed.raw);
    let entry = &meta_changes(&rig)[0];
    assert_eq!(entry["track"], json!("system"));
    assert_eq!(entry["cause"], json!("track_lost"));
    assert_eq!(entry["to"], Value::Null);

    polled(&rig);
    let n = rig.source.attempts(Track::System);
    advance(&rig, ms(1_900));
    assert_eq!(rig.source.attempts(Track::System), n, "none before 4 s");
    advance(&rig, ms(100));
    assert!(
        wait(|| rig.source.attempts(Track::System) == n + 1),
        "one attempt at 4 s"
    );
    advance(&rig, ms(1_900));
    assert_eq!(rig.source.attempts(Track::System), n + 1, "none before 6 s");
    advance(&rig, ms(100));
    assert!(
        wait(|| rig.source.attempts(Track::System) == n + 2),
        "one attempt at 6 s"
    );
    polled(&rig);
    assert_eq!(rig.source.attempts(Track::System), n + 2);
    assert_eq!(rig.lines_named("call_track_silent").len(), 1, "once");
    assert_eq!(device_lines(&rig).len(), 1);
    assert_eq!(kind_count(&rig, WarningKind::TrackLost), 1);
    assert_eq!(rig.source.ok_opens(Track::System), 1);

    rig.source.heal(Track::System);
    advance(&rig, ms(2_000));
    assert!(
        wait(|| rig.source.ok_opens(Track::System) == 2),
        "the 8 s attempt reopens it"
    );
    rig.source.feed_tone(Track::System, 9_000, 9_200, 0.2);
    assert!(wait(
        || device_lines(&rig).len() == 2 && meta_changes(&rig).len() == 2
    ));
    rig.source.feed_tone(Track::System, 9_200, 9_800, 0.2);
    rig.source.feed(Track::Mic, 9_000, 9_800, 0.3);
    // The indicator goes idle at stop (it carries no warnings then): read the warning
    // while the call still runs.
    let lost_before_stop = kind_count(&rig, WarningKind::TrackLost);
    rig.session.stop(StopReason::User);

    let back = &device_lines(&rig)[1];
    assert_eq!(back.get("cause"), Some("reopened"));
    let new_id = reopened_id(Track::System, 2);
    assert_eq!(back.get("to"), Some(new_id.as_str()));
    let gap: u64 = back
        .get("gap_ms")
        .expect("gap_ms")
        .parse()
        .expect("a number");
    assert!((7_000..=9_000).contains(&gap), "the silence it cost: {gap}");
    assert_eq!(meta_changes(&rig)[1]["cause"], json!("reopened"));
    assert_eq!(lost_before_stop, 1, "the warning stays until dismissed");
    assert_eq!(rig.lines_named("call_track_silent").len(), 1);
    let duration = rig.only_meta()["duration_ms"].as_u64().expect("a duration");
    assert!(
        duration >= 9_600,
        "the call covers the lost stretch: {duration}"
    );
    let system = track_samples(&rig, "system.opus");
    assert!(rms_between(&system, 100, 900) > 0.03);
    assert!(
        rms_between(&system, 1_200, 8_800) < 0.005,
        "zeros from the loss on"
    );
    assert!(rms_between(&system, 9_150, 9_700) > 0.03);
}

#[test]
fn a_trigger_while_a_reopen_is_pending_is_absorbed() {
    // Invariant D1 (validation 1, V1): the system stream dies and every open fails (Busy),
    // so a reopen stays pending. A `DefaultOutputChanged` that comes 1 s after the loss and
    // another after the silent declaration are absorbed: no open of their own, the 2 s
    // deadline counts from the loss (silent exactly at loss + 2 s, not at event + 2 s), one
    // `call_track_silent`, one `to=none` line and entry, attempts only at the 2 s marks, and
    // the later success still says `cause=reopened` (not the event's cause). Bite: the guard
    // in `DeviceThread::trigger` removed (a fresh `Reopen` per event: an extra attempt at
    // once, the deadline restarted, a second declaration, the success with
    // `default_output_changed`).
    let rig = CallRig::with_capacity(2_048, quiet);
    started(&rig, CallSource::Tray);
    rig.source.feed(Track::System, 0, 1_000, 0.2);
    rig.source.feed(Track::Mic, 0, 1_000, 0.3);
    rig.source.set_stamp_ms(1_000);
    rig.source.fail_all(Track::System, OpenError::Busy);
    rig.source
        .lose(Track::System, LossCause::NotAvailable, 1_000);
    rig.source.feed(Track::Mic, 1_000, 9_000, 0.3);
    assert!(
        wait(|| rig.source.attempts(Track::System) >= 2),
        "the first attempt comes at once"
    );
    polled(&rig);
    let first = rig.source.attempts(Track::System);

    // 1 s into the pending reopen: the event is absorbed.
    advance(&rig, ms(1_000));
    rig.devices.push(DeviceEvent::DefaultOutputChanged);
    assert!(wait(|| rig.devices.pending() == 0), "the event was taken");
    polled(&rig);
    assert_eq!(
        rig.source.attempts(Track::System),
        first,
        "no open for an event while a reopen is pending"
    );
    advance(&rig, ms(900));
    assert!(rig.lines_named("call_track_silent").is_empty());
    assert!(device_lines(&rig).is_empty());

    // Loss + 2 s (event + 1 s): silent now, on the loss's deadline.
    advance(&rig, ms(100));
    assert!(
        wait(|| {
            rig.lines_named("call_track_silent").len() == 1
                && device_lines(&rig).len() == 1
                && meta_changes(&rig).len() == 1
        }),
        "silent at loss + 2 s: {:?} / {:?}",
        rig.lines_named("call_track_silent"),
        device_lines(&rig)
    );
    assert_eq!(device_lines(&rig)[0].get("to"), Some("none"));
    assert_eq!(device_lines(&rig)[0].get("cause"), Some("track_lost"));
    assert!(
        wait(|| rig.source.attempts(Track::System) == first + 1),
        "one attempt at the 2 s mark"
    );

    // After the declaration: the second event is absorbed too.
    rig.devices.push(DeviceEvent::DefaultOutputChanged);
    assert!(wait(|| rig.devices.pending() == 0), "the event was taken");
    polled(&rig);
    assert_eq!(
        rig.source.attempts(Track::System),
        first + 1,
        "no open for an event after the declaration"
    );
    advance(&rig, ms(1_900));
    assert_eq!(
        rig.source.attempts(Track::System),
        first + 1,
        "none before 4 s"
    );
    advance(&rig, ms(100));
    assert!(
        wait(|| rig.source.attempts(Track::System) == first + 2),
        "one attempt at 4 s"
    );
    advance(&rig, ms(1_900));
    assert_eq!(
        rig.source.attempts(Track::System),
        first + 2,
        "none before 6 s"
    );
    assert_eq!(
        rig.lines_named("call_track_silent").len(),
        1,
        "declared once"
    );
    assert_eq!(device_lines(&rig).len(), 1, "{:?}", device_lines(&rig));
    assert_eq!(meta_changes(&rig).len(), 1, "{:?}", meta_changes(&rig));

    // The 6 s attempt reopens it: the cause is the reopen, not the absorbed event.
    rig.source.heal(Track::System);
    advance(&rig, ms(100));
    assert!(
        wait(|| rig.source.ok_opens(Track::System) == 2),
        "the 6 s attempt reopens it"
    );
    assert_eq!(rig.source.attempts(Track::System), first + 3);
    rig.source.feed(Track::System, 7_000, 7_400, 0.2);
    assert!(
        wait(|| device_lines(&rig).len() == 2 && meta_changes(&rig).len() == 2),
        "{:?}",
        device_lines(&rig)
    );
    rig.session.stop(StopReason::User);
    let lines = device_lines(&rig);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1].get("cause"), Some("reopened"));
    assert_eq!(
        lines.iter().filter(|l| l.get("to") == Some("none")).count(),
        1
    );
    let changes = meta_changes(&rig);
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[1]["cause"], json!("reopened"));
    assert_eq!(
        changes.iter().filter(|c| c["to"] == Value::Null).count(),
        1,
        "{changes:?}"
    );
    assert_eq!(rig.lines_named("call_track_silent").len(), 1);
    assert_eq!(rig.source.ok_opens(Track::System), 2);
}

#[test]
fn no_device_left_gives_no_device() {
    // Acceptance 3, failure branch: the track is lost and the open says NoDevice (no output
    // or no input left): at 2 s the same silence, warning and `call_track_silent rule=lost`,
    // but the device line says `cause=no_device to=none`; each track names its own track
    // in the line and has its own warning text. Bite: `track_lost` instead of `no_device`;
    // the mic and the system track sharing one text; a track other than the lost one named.
    let mut texts = Vec::new();
    for track in [Track::System, Track::Mic] {
        let name = track.as_str();
        let rig = CallRig::with_capacity(1_024, quiet);
        started(&rig, CallSource::Tray);
        feed(&rig, 0, 1_000);
        rig.source.set_stamp_ms(1_000);
        rig.source.fail_all(track, OpenError::NoDevice);
        rig.source.lose(track, LossCause::NotAvailable, 1_000);
        assert!(wait(|| rig.source.attempts(track) >= 2), "{name}");
        advance(&rig, ms(2_000));
        assert!(
            wait(|| rig.lines_named("call_track_silent").len() == 1
                && device_lines(&rig).len() == 1
                && kind_count(&rig, WarningKind::TrackLost) == 1),
            "{name}: {:?} / {:?}",
            rig.lines_named("call_track_silent"),
            device_lines(&rig)
        );
        let silent = &rig.lines_named("call_track_silent")[0];
        assert_eq!(silent.get("track"), Some(name));
        assert_eq!(silent.get("rule"), Some("lost"));
        let changed = &device_lines(&rig)[0];
        assert_eq!(changed.get("track"), Some(name));
        assert_eq!(changed.get("cause"), Some("no_device"));
        assert_eq!(changed.get("to"), Some("none"));
        assert_eq!(meta_changes(&rig)[0]["cause"], json!("no_device"), "{name}");
        assert!(rig.session.is_recording(), "{name}: the call goes on");
        let state = rig.indicator.states().last().cloned().expect("a state");
        let warning = state
            .warnings
            .iter()
            .find(|w| w.kind == WarningKind::TrackLost)
            .expect("the warning");
        texts.push(warning.text.clone());
        rig.session.stop(StopReason::User);
    }
    assert_ne!(texts[0], texts[1], "one text per track: {texts:?}");
}

#[test]
fn a_track_without_packets_still_grows_with_the_other_track() {
    // D3 / R3 (#244: a paused loopback sends no packets): a track that delivers nothing is
    // advanced with zeros to the other track's stamps, so both files and `duration_ms`
    // cover the call and later times never shift. Bite: no advance (the silent track's file
    // and the duration stay at 0); the advance only for a lost track.
    for silent in [Track::System, Track::Mic] {
        let live = match silent {
            Track::System => Track::Mic,
            Track::Mic => Track::System,
        };
        let name = silent.as_str();
        let rig = CallRig::with_capacity(1_024, quiet);
        started(&rig, CallSource::Tray);
        rig.source.feed(live, 0, 3_000, 0.3);
        rig.session.stop(StopReason::User);
        let file = format!("{name}.opus");
        let dir = rig.folders()[0].clone();
        let scanned = scan(File::open(dir.join(&file)).expect("file")).expect("readable");
        assert_eq!(scanned.end, ReadEnd::Complete, "{name}");
        assert!(
            scanned.duration_48k >= 48 * 2_600,
            "{name}: grew with the other track: {}",
            scanned.duration_48k
        );
        let duration = rig.only_meta()["duration_ms"].as_u64().expect("a duration");
        assert!(duration >= 2_600, "{name}: {duration}");
        let samples = track_samples(&rig, &file);
        assert!(rms_between(&samples, 100, 2_500) < 0.002, "{name}: zeros");
    }
}

#[test]
fn the_communications_check_gives_the_warning_only_for_a_difference() {
    // Acceptance 4 (T068, FR-010): the check runs once per record start, before
    // `call_started`. Ok(true) is a difference: the `communications_device_differs`
    // warning is in the first published state and `call_started` says comms_differs=1.
    // Ok(false), Err(E_NOTFOUND) and any other Err are no difference (no warning, 0, no
    // extra line, the start succeeds). Bite: E_NOTFOUND (or every Err) as a difference;
    // an Err that refuses the start; the value not reaching the log; the check after the
    // start line; no check at all.
    let cases: [(Result<bool, i32>, bool, &str); 4] = [
        (Ok(true), true, "Ok(true)"),
        (Ok(false), false, "Ok(false)"),
        (Err(E_NOTFOUND), false, "Err(E_NOTFOUND)"),
        (Err(5), false, "Err(other)"),
    ];
    for (answer, differs, tag) in cases {
        let rig = CallRig::with(quiet);
        rig.comms.set(answer);
        started(&rig, CallSource::Tray);
        assert_eq!(rig.comms.calls(), 1, "{tag}: once per record start");
        let first = rig.indicator.states().first().cloned().expect("a state");
        let listed = first
            .warnings
            .iter()
            .filter(|w| w.kind == WarningKind::CommunicationsDevice)
            .count();
        assert_eq!(listed, usize::from(differs), "{tag}: {:?}", first.warnings);
        let lines = rig.lines_named("call_started");
        assert_eq!(lines.len(), 1, "{tag}");
        assert_eq!(
            lines[0].get("comms_differs"),
            Some(if differs { "1" } else { "0" }),
            "{tag}"
        );
        assert!(rig.lines_named("call_refused").is_empty(), "{tag}");
        rig.session.stop(StopReason::User);
        rig.session.toggle_record(true, CallSource::Tray);
        assert_eq!(rig.comms.calls(), 2, "{tag}: and again at the next start");
        rig.session.stop(StopReason::User);
    }
    // A subtitles start never asks; the record start on its captures asks once.
    let rig = CallRig::with(quiet);
    rig.comms.set(Ok(true));
    assert_eq!(
        rig.session.toggle_subtitles(true, CallSource::Switch),
        Toggled::Started
    );
    assert_eq!(rig.comms.calls(), 0, "subtitles only: no check");
    started(&rig, CallSource::Tray);
    assert_eq!(rig.comms.calls(), 1);
    rig.session.stop(StopReason::User);
}

/// FR-013 / R21 on one side: `silent` delivers audio of zeros while the other track speaks.
fn cant_hear_run(silent: Track) {
    let (kind, name) = match silent {
        Track::System => (WarningKind::CantHearOtherParty, "system"),
        Track::Mic => (WarningKind::CantHearYou, "mic"),
    };
    let speaking = match silent {
        Track::System => Track::Mic,
        Track::Mic => Track::System,
    };
    let rig = CallRig::with_capacity(16_384, quiet);
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    let both = [Track::Mic, Track::System];
    let flag = |tick: &IndicatorTick| match silent {
        Track::System => tick.system_silent,
        Track::Mic => tick.mic_silent,
    };
    let pos = std::cell::Cell::new(0u64);
    // Feeds `len` ms to both tracks (the silent one at `silent_amp`) and waits for the writer.
    let run = |len: u64, silent_amp: f32| {
        rig.source.feed(speaking, pos.get(), pos.get() + len, 0.3);
        rig.source
            .feed(silent, pos.get(), pos.get() + len, silent_amp);
        pos.set(pos.get() + len);
        drained(&rig, &both, pos.get());
    };
    // One more 20 ms of both tracks and the tick that follows it.
    let probe = |silent_amp: f32| {
        let tick = barrier(&rig, &[(speaking, 0.3), (silent, silent_amp)], pos.get());
        pos.set(pos.get() + 20);
        tick
    };

    run(59_000, 0.0);
    assert_eq!(
        kind_count(&rig, kind),
        0,
        "{name}: 59 s is not more than 60 s"
    );
    assert!(cant_hear_lines(&rig).is_empty(), "{name}");
    assert!(!flag(&probe(0.0)), "{name}: not silent yet");

    run(2_980, 0.0);
    assert!(
        wait(|| kind_count(&rig, kind) == 1 && cant_hear_lines(&rig).len() == 1),
        "{name}: after 60 s of silence: {:?} / {:?}",
        warnings_of(&rig),
        cant_hear_lines(&rig)
    );
    assert_eq!(cant_hear_lines(&rig)[0].get("track"), Some(name));
    assert!(cant_hear_lines(&rig)[0].raw.contains(" WARN "));
    assert!(flag(&probe(0.0)), "{name}: the tick says silent");

    run(63_000, 0.0);
    assert_eq!(kind_count(&rig, kind), 1, "{name}: once per stretch");
    assert_eq!(cant_hear_lines(&rig).len(), 1, "{name}: once per stretch");

    run(2_000, 0.3);
    assert!(!flag(&probe(0.3)), "{name}: speech clears the flag");
    assert_eq!(kind_count(&rig, kind), 1, "{name}: the warning stays");
    let id = warning_ids(&rig, kind)[0];
    assert!(rig.session.dismiss_warning(id), "{name}");
    assert_eq!(kind_count(&rig, kind), 0, "{name}");

    run(63_000, 0.0);
    assert!(
        wait(|| kind_count(&rig, kind) == 1 && cant_hear_lines(&rig).len() == 2),
        "{name}: a second stretch warns again: {:?} / {:?}",
        warnings_of(&rig),
        cant_hear_lines(&rig)
    );
    assert_ne!(warning_ids(&rig, kind)[0], id, "{name}: a new entry");
    rig.session.stop(StopReason::User);
}

#[test]
fn cant_hear_other_party_after_60_s_once_per_stretch_and_rearmed_by_speech() {
    // Acceptance 5 (T068, FR-013, R21): the mic has speech and the system track has none for
    // more than 60 s: one `cant_hear_other_party` warning, one `call_track_silent
    // rule=cant_hear track=system` line and the tick's `system_silent`; not at 59 s; not
    // again within the stretch; one speech second re-arms it (and clears the flag); a second
    // stretch gives a second line and, the first warning being dismissed, a new entry.
    // Bite: the threshold at 30 s or 90 s; a warning or line per second after 60 s; no
    // re-arm; `system_silent` always false; mic and system swapped.
    cant_hear_run(Track::System);
}

#[test]
fn cant_hear_you_after_60_s_of_mic_silence_mirrors_the_system_case() {
    // Acceptance 5, mirror case: the system track has speech and the mic none for more than
    // 60 s: `cant_hear_you`, `call_track_silent track=mic rule=cant_hear`, `mic_silent`.
    // Same bites as the system case, plus the kinds swapped.
    cant_hear_run(Track::Mic);
}

#[test]
fn no_cant_hear_when_nobody_speaks_or_the_silent_track_is_lost() {
    // Acceptance 5 boundaries (D4): 65 s of silence on both tracks gives no warning (the
    // other party did not speak either); a lost system track (zeros from the loss) gives
    // `track_lost` territory, never `cant_hear_other_party`. Bite: the rule firing on
    // silence alone; the rule not excluding a lost track.
    let rig = CallRig::with_capacity(8_192, quiet);
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    let both = [Track::Mic, Track::System];
    feed_silence_both(&rig);
    drained(&rig, &both, 65_000);
    barrier(&rig, &[(Track::Mic, 0.0), (Track::System, 0.0)], 65_000);
    assert!(hear_kinds(&rig).is_empty(), "{:?}", warnings_of(&rig));
    assert!(cant_hear_lines(&rig).is_empty());
    rig.session.stop(StopReason::User);

    let rig = CallRig::with_capacity(8_192, quiet);
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 1_000);
    rig.source.set_stamp_ms(1_000);
    rig.source.fail_all(Track::System, OpenError::NoDevice);
    rig.source
        .lose(Track::System, LossCause::NotAvailable, 1_000);
    rig.source.feed(Track::Mic, 1_000, 65_000, 0.3);
    drained(&rig, &[Track::Mic], 65_000);
    assert!(hear_kinds(&rig).is_empty(), "{:?}", warnings_of(&rig));
    assert!(cant_hear_lines(&rig).is_empty());
    rig.session.stop(StopReason::User);
}

#[test]
fn a_lost_track_gives_no_cant_hear_once_its_zeros_are_placed() {
    // Acceptance 5 failure branch (D4), added by the developer: the lost system track is
    // filled with zeros up to the mic's last block (D3) - checked after the stop, when the
    // zeros are surely placed (the duration covers them) - and those zeros never count as
    // a silent stretch. Bite: lost-track time counted as silence.
    let rig = CallRig::with_capacity(8_192, quiet);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 1_000);
    rig.source.set_stamp_ms(1_000);
    rig.source.fail_all(Track::System, OpenError::NoDevice);
    rig.source
        .lose(Track::System, LossCause::NotAvailable, 1_000);
    rig.source.feed(Track::Mic, 1_000, 65_000, 0.3);
    rig.session.stop(StopReason::User);
    let duration = rig.only_meta()["duration_ms"].as_u64().expect("a duration");
    assert!(duration >= 64_000, "the lost track was filled: {duration}");
    assert!(
        cant_hear_lines(&rig).is_empty(),
        "{:?}",
        cant_hear_lines(&rig)
    );
}

fn feed_silence_both(rig: &CallRig) {
    rig.source.feed_both(0, 65_000, 0.0, 0.0);
}

fn hear_kinds(rig: &CallRig) -> Vec<WarningKind> {
    warnings_of(rig)
        .into_iter()
        .filter(|k| {
            matches!(
                k,
                WarningKind::CantHearOtherParty | WarningKind::CantHearYou
            )
        })
        .collect()
}

fn cant_hear_lines(rig: &CallRig) -> Vec<voicen_core_support::Line> {
    rig.lines_named("call_track_silent")
        .into_iter()
        .filter(|l| l.get("rule") == Some("cant_hear"))
        .collect()
}

fn mic_default_change(rig: &CallRig, n: usize, from: u64) {
    rig.devices.push(DeviceEvent::DefaultInputChanged);
    assert!(
        wait(|| rig.source.ok_opens(Track::Mic) == n + 1),
        "change {n}: the mic is reopened"
    );
    rig.source.feed(Track::Mic, from, from + 200, 0.3);
    assert!(wait(|| device_lines(rig).len() == n), "change {n}: logged");
}

#[test]
fn a_dismissed_warning_comes_back_on_its_next_trigger_and_a_listed_one_is_not_duplicated() {
    // Acceptance 6 (T068, FR-006, D4): a warning stays in the list until dismissed. A second
    // trigger of a kind that is still listed adds no second entry (but is still a device
    // change: line and entry). `dismiss_warning(id)` removes that entry, republishes the
    // indicator state inside the session lock and answers true; an unknown or already
    // removed id, or a session that does not record, answers false. The next trigger of the
    // dismissed kind adds a new entry with a new id. Bite: a duplicate entry per trigger; a
    // dismiss that removes nothing, removes by kind, or does not republish; a dismiss of an
    // unknown id answering true; a dismissed kind that never comes back.
    let rig = CallRig::with_capacity(1_024, quiet);
    assert!(!rig.session.dismiss_warning(1), "no call records");
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 400);

    mic_default_change(&rig, 1, 400);
    assert!(wait(|| kind_count(&rig, WarningKind::MicChanged) == 1));
    let first = warning_ids(&rig, WarningKind::MicChanged);

    mic_default_change(&rig, 2, 800);
    barrier(&rig, &[(Track::Mic, 0.3)], 1_000);
    assert_eq!(
        warning_ids(&rig, WarningKind::MicChanged),
        first,
        "a listed kind is not added again"
    );
    assert_eq!(meta_changes(&rig).len(), 2, "every change is an entry");

    assert!(rig.session.dismiss_warning(first[0]));
    assert_eq!(kind_count(&rig, WarningKind::MicChanged), 0);
    assert_eq!(
        rig.indicator.held_flags().last(),
        Some(&true),
        "republished inside the lock"
    );
    assert!(!rig.session.dismiss_warning(first[0]), "already gone");
    assert!(!rig.session.dismiss_warning(first[0] + 1_000), "unknown");

    mic_default_change(&rig, 3, 1_200);
    assert!(wait(|| kind_count(&rig, WarningKind::MicChanged) == 1));
    let again = warning_ids(&rig, WarningKind::MicChanged);
    assert_ne!(again, first, "a new entry with a new id");
    rig.session.stop(StopReason::User);
    assert!(!rig.session.dismiss_warning(again[0]), "the call is over");
}

#[test]
fn subtitles_only_reopens_a_lost_track_silently() {
    // D6: with subtitles on and record off, a lost track is still reopened (the captions
    // go on), but nothing reaches the log, the calls root, the indicator or the shell.
    // Bite: the reopen skipped while record is off; a `call_device` line or a meta entry
    // without a call; an indicator state or notice.
    let rig = CallRig::with_capacity(256, quiet);
    assert_eq!(
        rig.session.toggle_subtitles(true, CallSource::Switch),
        Toggled::Started
    );
    rig.source.feed_both(0, 400, 0.3, 0.1);
    assert!(consumer_has(&rig, fed(0, 400)));
    rig.source.set_stamp_ms(400);
    rig.source.lose(Track::System, LossCause::NotAvailable, 400);
    assert!(
        wait(|| rig.source.ok_opens(Track::System) == 2),
        "the track is reopened"
    );
    rig.source.feed(Track::System, 500, 700, 0.1);
    assert!(wait(
        || rig.subtitles.samples(Track::System) >= fed(0, 400) + fed(500, 700)
    ));
    let trace = rig.source.trace();
    let stop_old = trace
        .iter()
        .position(|e| e == "stop system")
        .expect("stopped");
    let reopen = trace
        .iter()
        .rposition(|e| e == "open system")
        .expect("opened");
    assert!(stop_old < reopen, "{trace:?}");
    assert!(rig.call_lines().is_empty(), "{:?}", rig.call_lines());
    assert!(rig.root_files().is_empty(), "{:?}", rig.root_files());
    assert!(rig.indicator.calls().is_empty());
    assert!(rig.shell.calls().is_empty());
    rig.session.toggle_subtitles(false, CallSource::Switch);
}

#[test]
fn closing_while_a_reopen_is_pending_leaves_no_handle_open() {
    // D1: a stop that comes while a reopen is running (the open is held like a slow
    // device) waits for it, stops the handle it produced and joins the device thread: no
    // handle stays open, every success was stopped once, the event stream is dropped, the
    // call is finished. Bite: the device thread not joined or not stopped at close (a
    // leaked handle); a handle opened after the stop kept; the stream not dropped.
    let rig = CallRig::with_capacity(256, quiet);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 400);
    rig.source.set_stamp_ms(400);
    assert_eq!(rig.devices.opened(), 1);
    rig.source.hold_opens();
    rig.source.lose(Track::System, LossCause::NotAvailable, 400);
    assert!(
        wait(|| rig.source.blocked_opens() == 1),
        "the reopen is running"
    );
    let session = rig.session.clone();
    let stopper = std::thread::spawn(move || session.stop(StopReason::Exit));
    assert!(
        wait(|| rig.session.try_is_recording().is_none()),
        "stop holds the session lock while it closes"
    );
    rig.source.release_opens();
    assert!(
        stopper.join().expect("stop returns"),
        "a call was recording"
    );
    assert_eq!(rig.source.open_handles(), 0, "{:?}", rig.source.trace());
    assert_eq!(
        rig.source.stops(),
        rig.source.ok_opens(Track::System) + rig.source.ok_opens(Track::Mic),
        "every handle that opened was stopped once"
    );
    assert_eq!(rig.source.peak_handles(Track::System), 1);
    assert_eq!(rig.devices.alive(), 0, "the event stream is dropped");
    assert_eq!(rig.only_meta()["state"], json!("ready-to-transcribe"));
    assert_eq!(rig.lines_named("call_stopped").len(), 1);
}

// ---- T-144: storage failure, low disk, the 4 h limit ------------------------------------

fn track_scan(rig: &CallRig, file: &str) -> voicen_core::call::codec::TrackScan {
    scan(File::open(rig.folders()[0].join(file)).expect(file)).expect("a readable track")
}

fn track_sizes(rig: &CallRig) -> [u64; 2] {
    ["mic.opus", "system.opus"].map(|f| std::fs::metadata(rig.folders()[0].join(f)).expect(f).len())
}

fn call_meta_bytes(rig: &CallRig) -> Vec<u8> {
    std::fs::read(rig.folders()[0].join("meta.json")).expect("meta.json")
}

/// Ticks once per mono second for `secs` seconds.
fn tick_secs(rig: &CallRig, secs: u64) {
    for _ in 0..secs {
        rig.mono.advance(ms(1_000));
        rig.session.tick();
    }
}

#[test]
fn a_write_failure_stops_the_call_keeps_flushed_audio_and_leaves_meta_recording() {
    // Acceptance 4 (failure branch): the mic file's write fails once (F-024: one call).
    // The writer stops writing BOTH tracks (the sizes stay), no mid-call meta write
    // follows (a device change after the failure included), `tick` stops the call:
    // `meta.json` stays `recording` byte for byte, `call_storage_failed kind=write` and
    // `call_stopped reason=storage_error` are logged, the only notice is
    // `Stopped{StorageError}` (never `CallSavedTranscribing`), the indicator is off and the
    // flushed audio reads back. Bite: the stop swallowed (still recording after tick), the
    // other track still written, `finish` writing ready-to-transcribe, a mid-call
    // write_meta after the failure, the saved notice, the failure not logged.
    let rig = CallRig::with_capacity(8_192, quiet);
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    let both = [Track::Mic, Track::System];
    // 4 s before the fault: the timeline and resampler hold a little back, so a bit under
    // 4 s reaches each encoder, which is at least two full 1 s Ogg pages on disk (a
    // 2 s feed put only one there: 47 688 granule, and the next write was the fault).
    rig.source.feed_both(0, 4_000, 0.3, 0.2);
    rig.source.set_stamp_ms(4_000);
    rig.mono.advance(ms(4_000));
    drained(&rig, &both, 4_000);
    let meta_before = call_meta_bytes(&rig);
    // What the tracks hold before the fault: the stop must keep at least this.
    let synced = ["mic.opus", "system.opus"].map(|file| track_scan(&rig, file).duration_48k);
    for (file, kept) in ["mic.opus", "system.opus"].iter().zip(synced) {
        assert!(kept >= 2 * 48_000 - 312, "{file}: two pages synced, {kept}");
    }

    rig.opener.fail_next_write("mic.opus");
    rig.source.feed_both(4_000, 6_000, 0.3, 0.2);
    rig.source.set_stamp_ms(6_000);
    rig.mono.advance(ms(2_000));
    drained(&rig, &both, 6_000);
    assert!(
        wait(|| !rig.lines_named("call_storage_failed").is_empty()),
        "the write failure is logged"
    );
    let failed = rig.lines_named("call_storage_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].get("kind"), Some("write"));

    rig.source.feed_both(6_000, 7_000, 0.3, 0.2);
    rig.source.set_stamp_ms(7_000);
    drained(&rig, &both, 7_000);
    let baseline = track_sizes(&rig);
    // A device change after the failure: the mic reopens and `call_device` is logged, which
    // is where the session would write `device_changes` into meta.json mid-call.
    rig.devices.push(DeviceEvent::DefaultInputChanged);
    assert!(wait(|| rig.source.ok_opens(Track::Mic) == 2));
    rig.source.feed(Track::Mic, 7_400, 7_800, 0.3);
    rig.source.feed(Track::System, 7_000, 9_000, 0.2);
    rig.source.set_stamp_ms(9_000);
    rig.mono.advance(ms(2_000));
    drained(&rig, &[Track::System], 9_000);
    assert!(
        wait(|| device_lines(&rig).len() == 1),
        "the change was handled"
    );
    assert_eq!(call_meta_bytes(&rig), meta_before, "no mid-call meta write");

    rig.session.tick();
    assert!(!rig.session.is_recording(), "the tick stopped the call");
    assert_eq!(track_sizes(&rig), baseline, "no byte after the failure");
    for (file, kept) in ["mic.opus", "system.opus"].iter().zip(synced) {
        let scanned = track_scan(&rig, file);
        assert_eq!(scanned.end, ReadEnd::Truncated, "{file}");
        assert!(
            scanned.duration_48k >= kept,
            "{file}: flushed audio kept, {} < {kept}",
            scanned.duration_48k
        );
    }
    assert_eq!(call_meta_bytes(&rig), meta_before, "meta.json untouched");
    let meta = rig.only_meta();
    assert_eq!(meta["state"], json!("recording"));
    assert!(rig
        .root_files()
        .iter()
        .all(|p| !p.to_string_lossy().ends_with(".tmp")));
    let stopped = rig.lines_named("call_stopped");
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].get("reason"), Some("storage_error"));
    assert_eq!(
        rig.shell.calls(),
        vec![ShellCall::Notice(CallNotice::Stopped {
            reason: StopReason::StorageError
        })]
    );
    assert_eq!(rig.indicator.recording_flags().last(), Some(&false));
    // US4 #5, I-b: the stopped call is no longer the store's active call, so its
    // `recording` meta lists as Recovered in the same run (bite: `finish` not releasing).
    let listed = rig.store.list(&call_support::settings(&rig.root).calls);
    let states: Vec<ListState> = listed.entries.iter().map(|e| e.state).collect();
    assert_eq!(states, vec![ListState::Recovered]);
}

#[test]
fn after_a_write_failure_a_drop_is_not_written_into_meta() {
    // Acceptance 4 (failure branch), I-d: after the first failed track write the writer
    // makes no mid-call `write_meta`, a drop included: the drop is still logged
    // (`call_track_dropped`), but meta.json stays byte-equal to the one of the start until
    // the stop, which leaves it `recording`. The mic's first audio page write fails once
    // (F-024). Fed in 80 ms steps the channel (capacity 4) drops nothing until the writer
    // is held for the burst, which drops exactly the blocks past the fourth. Bite: the
    // `rec.failed` return in the writer's drop handling removed.
    let rig = CallRig::with_capacity(4, |_| {});
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    let meta_before = call_meta_bytes(&rig);
    rig.opener.fail_next_write("mic.opus");
    let mut at = 0;
    while rig.lines_named("call_storage_failed").is_empty() {
        assert!(at < 4_000, "the mic's first page write fails");
        rig.source.feed(Track::Mic, at, at + 80, 0.3);
        at += 80;
        drained(&rig, &[Track::Mic], at);
    }
    assert!(rig.lines_named("call_track_dropped").is_empty());

    let pause = rig.session.pause_writer();
    rig.source.feed(Track::Mic, at, at + 140, 0.3);
    drop(pause);
    assert!(
        wait(|| rig.lines_named("call_track_dropped").len() == 1),
        "the drop after the failure is handled"
    );
    assert_eq!(call_meta_bytes(&rig), meta_before, "no mid-call meta write");

    rig.session.tick();
    assert!(!rig.session.is_recording(), "the tick stopped the call");
    assert_eq!(call_meta_bytes(&rig), meta_before, "meta.json untouched");
    assert_eq!(rig.only_meta()["state"], json!("recording"));
}

/// What any stop after a failed track write leaves (Acceptance 4, FR-016, I-d; review
/// round 1 #1): whatever reason was asked, the stop is a storage-error stop. meta.json is
/// byte-equal to `meta_before` (still `recording`, no duration that counts audio never
/// written), `call_stopped reason=storage_error`, the only notice `Stopped{StorageError}`
/// (never `CallSavedTranscribing`), the indicator off, and the call lists as Recovered.
#[track_caller]
fn stopped_as_storage_error(rig: &CallRig, meta_before: &[u8]) {
    assert!(!rig.session.is_recording(), "the stop ended the call");
    // Compared as text so a rewrite reads in the failure (state, duration, stop reason).
    assert_eq!(
        String::from_utf8_lossy(&call_meta_bytes(rig)),
        String::from_utf8_lossy(meta_before),
        "meta.json untouched after a write failure"
    );
    assert_eq!(rig.only_meta()["state"], json!("recording"));
    let stopped = rig.lines_named("call_stopped");
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    assert_eq!(stopped[0].get("reason"), Some("storage_error"));
    assert_eq!(
        rig.shell.calls(),
        vec![ShellCall::Notice(CallNotice::Stopped {
            reason: StopReason::StorageError
        })]
    );
    assert_eq!(rig.indicator.recording_flags().last(), Some(&false));
    let listed = rig.store.list(&call_support::settings(&rig.root).calls);
    let states: Vec<ListState> = listed.entries.iter().map(|e| e.state).collect();
    assert_eq!(states, vec![ListState::Recovered]);
}

#[test]
fn a_user_stop_after_a_write_failure_is_a_storage_error_stop() {
    // Review round 1 #1 (second path): the writer's failure only raises a flag that `tick`
    // acts on later; a user stop in that window (no tick at all here) must still stop as
    // `storage_error` from the writer's own fact, not rewrite meta.json ready-to-transcribe.
    // Subtitles on: the stop goes through the writer's Detach. The mic's write fails once
    // (F-024); progress is read from the log line and the consumer, never from ticks
    // (F-031). Bite: `finish` deciding keep_meta / the logged reason from the asked
    // reason only, `stop` noticing from the asked reason.
    let rig = CallRig::with_capacity(8_192, quiet);
    rig.session.toggle_subtitles(true, CallSource::Switch);
    started(&rig, CallSource::Tray);
    let both = [Track::Mic, Track::System];
    rig.source.feed_both(0, 4_000, 0.3, 0.2);
    rig.source.set_stamp_ms(4_000);
    rig.mono.advance(ms(4_000));
    drained(&rig, &both, 4_000);
    let meta_before = call_meta_bytes(&rig);

    rig.opener.fail_next_write("mic.opus");
    rig.source.feed_both(4_000, 6_000, 0.3, 0.2);
    rig.source.set_stamp_ms(6_000);
    drained(&rig, &both, 6_000);
    assert!(
        wait(|| !rig.lines_named("call_storage_failed").is_empty()),
        "the write failure is logged"
    );
    let baseline = track_sizes(&rig);

    assert_eq!(
        rig.session.toggle_record(false, CallSource::Tray),
        Toggled::Stopped
    );
    stopped_as_storage_error(&rig, &meta_before);
    assert_eq!(track_sizes(&rig), baseline, "no byte after the failure");
    assert!(rig.session.is_subtitles(), "subtitles go on");
}

#[test]
fn an_exit_stop_after_a_write_failure_is_a_storage_error_stop() {
    // Review round 1 #1 (second path): the quit-time `stop(Exit)` in the window between
    // the writer's failure and the next tick (no tick here). Subtitles off: the stop goes
    // through the captures' close. The mic's write fails once (F-024); the failure is
    // observed on the log line only (F-031). Bite: as above, through the close path.
    let rig = CallRig::with_capacity(8_192, quiet);
    started(&rig, CallSource::Tray);
    let meta_before = call_meta_bytes(&rig);
    rig.opener.fail_next_write("mic.opus");
    rig.source.feed_both(0, 4_000, 0.3, 0.2);
    rig.source.set_stamp_ms(4_000);
    rig.mono.advance(ms(4_000));
    assert!(
        wait(|| !rig.lines_named("call_storage_failed").is_empty()),
        "the write failure is logged"
    );

    assert!(rig.session.stop(StopReason::Exit));
    stopped_as_storage_error(&rig, &meta_before);
}

#[test]
fn free_space_is_polled_every_10_s_warns_once_below_1_gb_and_stops_below_200_mb() {
    // Acceptance 5: the call folder's free space is polled every 10 s of mono time (the
    // first poll within 10 s of the start); below 1 GB exactly one `low_disk` warning
    // (not repeated at later polls, not back after a dismissal); below 200 MB the call
    // stops with saving (`call_storage_failed kind=low_disk`, `call_stopped
    // reason=low_disk`, ready-to-transcribe, readable tracks, notice Stopped{LowDisk}).
    // A failing poll is skipped silently. Bite: a poll every 5 s or 20 s, the wrong
    // threshold, the warning repeated, the stop without a log line, the meta left
    // `recording`, the poll error stopping the call.
    let rig = CallRig::with(quiet);
    started(&rig, CallSource::Tray);
    let start = rig.mono.read();
    feed(&rig, 0, 2_000);
    tick_secs(&rig, 35);
    let polls = rig.disk.poll_times();
    assert!(polls.len() >= 3, "{polls:?}");
    assert!(
        polls[0] <= start + ms(10_000),
        "the first poll is due by 10 s"
    );
    for pair in polls.windows(2) {
        assert_eq!(pair[1] - pair[0], ms(10_000), "one poll every 10 s");
    }
    for dir in rig.disk.polled_dirs() {
        assert_eq!(dir, rig.folders()[0], "the call folder is asked");
    }
    assert!(warnings_but_provider(&rig).is_empty());

    rig.disk.set_error(io::ErrorKind::PermissionDenied);
    tick_secs(&rig, 25);
    assert!(rig.session.is_recording(), "a failed poll is skipped");
    assert!(rig.lines_named("call_storage_failed").is_empty());
    assert!(warnings_but_provider(&rig).is_empty());

    rig.disk.set_free(900 * call_support::MB);
    tick_secs(&rig, 25);
    assert!(rig.session.is_recording());
    assert_eq!(warnings_but_provider(&rig), vec![WarningKind::LowDisk]);
    let id = rig.indicator.states().last().expect("state").warnings[0].id;
    tick_secs(&rig, 30);
    let low = rig
        .indicator
        .states()
        .iter()
        .flat_map(|s| s.warnings.iter())
        .filter(|w| w.kind == WarningKind::LowDisk)
        .map(|w| w.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(low.len(), 1, "one warning id over the whole call: {low:?}");
    assert!(rig.session.dismiss_warning(id));
    tick_secs(&rig, 30);
    assert!(
        warnings_but_provider(&rig).is_empty(),
        "not listed again after a dismissal"
    );
    assert!(rig.session.is_recording());

    rig.disk.set_free(100 * call_support::MB);
    tick_secs(&rig, 10);
    assert!(!rig.session.is_recording(), "below 200 MB the call stops");
    let failed = rig.lines_named("call_storage_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].get("kind"), Some("low_disk"));
    let stopped = rig.lines_named("call_stopped");
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].get("reason"), Some("low_disk"));
    let meta = rig.only_meta();
    assert_eq!(meta["state"], json!("ready-to-transcribe"));
    assert_eq!(meta["stop_reason"], json!("low_disk"));
    for file in ["mic.opus", "system.opus"] {
        let scanned = track_scan(&rig, file);
        assert_eq!(scanned.end, ReadEnd::Complete, "{file}");
    }
    assert!(rig
        .shell
        .calls()
        .contains(&ShellCall::Notice(CallNotice::Stopped {
            reason: StopReason::LowDisk
        })));
    assert_eq!(rig.session.next_deadline(), None, "off: no deadline");
}

#[test]
fn between_200_mb_and_1_gb_the_call_keeps_recording() {
    // Acceptance 5 boundary: 200 MB exactly is not "below 200 MB". Bite: `<=`.
    let rig = CallRig::with(quiet);
    started(&rig, CallSource::Tray);
    rig.disk.set_free(200 * call_support::MB);
    tick_secs(&rig, 40);
    assert!(rig.session.is_recording());
    assert_eq!(warnings_but_provider(&rig), vec![WarningKind::LowDisk]);
    rig.session.stop(StopReason::User);
}

#[test]
fn a_poll_just_under_200_mb_stops_the_call_with_low_disk() {
    // Acceptance 5 boundary (validation round 1, mutant 44): one byte under 200 MB is
    // "below 200 MB", so the call stops with saving. Bite: a lower stop threshold
    // (150 MB, 199 MB) that the 100 MB poll of the main test lets through.
    let rig = CallRig::with(quiet);
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 2_000);
    rig.disk.set_free(200 * call_support::MB - 1);
    tick_secs(&rig, 10);
    assert!(!rig.session.is_recording(), "one byte under 200 MB stops");
    let failed = rig.lines_named("call_storage_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].get("kind"), Some("low_disk"));
    let stopped = rig.lines_named("call_stopped");
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    assert_eq!(stopped[0].get("reason"), Some("low_disk"));
    assert_eq!(rig.only_meta()["stop_reason"], json!("low_disk"));
}

#[test]
fn a_poll_just_under_1_gb_warns_and_exactly_1_gb_does_not() {
    // Acceptance 5 boundary: exactly 1 GB is not "below 1 GB" (no warning over three
    // polls); one byte under it gives the one `low_disk` warning and the call keeps
    // recording. Bite: a lower warn threshold (950 MB, the 900 MB poll of the main test
    // lets it through), `<=` instead of `<`.
    let rig = CallRig::with(quiet);
    started(&rig, CallSource::Tray);
    rig.disk.set_free(call_support::GB);
    tick_secs(&rig, 30);
    assert!(!rig.disk.poll_times().is_empty(), "the disk was polled");
    assert!(rig.session.is_recording());
    assert!(
        warnings_but_provider(&rig).is_empty(),
        "exactly 1 GB is not below 1 GB"
    );
    rig.disk.set_free(call_support::GB - 1);
    tick_secs(&rig, 10);
    assert!(rig.session.is_recording(), "a warning, not a stop");
    assert_eq!(warnings_but_provider(&rig), vec![WarningKind::LowDisk]);
    assert!(rig.lines_named("call_storage_failed").is_empty());
    rig.session.stop(StopReason::User);
}

#[test]
fn four_hours_stop_with_max_length() {
    // Acceptance 6: at 4 h of mono time since the start the call stops with
    // `max_length`, saved as ready-to-transcribe, notice Stopped{MaxLength}; one second
    // before it still records. Bite: 3 h or 5 h, no stop, the reason `user`.
    let rig = CallRig::new();
    started(&rig, CallSource::Tray);
    feed(&rig, 0, 2_000);
    rig.mono.advance(Duration::from_secs(4 * 3_600 - 1));
    rig.session.tick();
    assert!(rig.session.is_recording(), "4 h minus 1 s: still recording");
    rig.mono.advance(ms(1_000));
    rig.session.tick();
    assert!(!rig.session.is_recording(), "4 h: stopped");
    let meta = rig.only_meta();
    assert_eq!(meta["state"], json!("ready-to-transcribe"));
    assert_eq!(meta["stop_reason"], json!("max_length"));
    assert_eq!(
        rig.lines_named("call_stopped")[0].get("reason"),
        Some("max_length")
    );
    assert!(rig
        .shell
        .calls()
        .contains(&ShellCall::Notice(CallNotice::Stopped {
            reason: StopReason::MaxLength
        })));
}
