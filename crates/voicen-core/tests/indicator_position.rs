//! T-130 (spec 007 T033): `SettingsService::remember_indicator_position`, the narrow
//! writer of `calls.indicator_position` (the `remember_warned_host` pattern, tests in
//! `call_session.rs`). Red until the developer adds:
//!
//! ```text
//! voicen_core::settings::service::IndicatorPositionFailed   // unit struct, Debug, Clone, Copy, PartialEq, Eq
//! SettingsService::remember_indicator_position(&self, position: IndicatorPosition)
//!     -> Result<(), IndicatorPositionFailed>
//!     // under save_lock; the same position as in force: nothing written, Ok; otherwise
//!     // only calls.indicator_position changes, file written once, then swap + publish;
//!     // Unavailable or a write failure: snapshot kept, nothing published, Err.
//! ```
//!
//! F-024: every error fake fails exactly one call. Fake data only.

use std::io;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::secrets::{self as keys};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::roots::FakeCallRoots;
use voicen_core::settings::service::{IndicatorPositionFailed, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, IndicatorPosition};

const OS: Option<&str> = Some("en-US");

struct World {
    file: Arc<FakeSettingsFile>,
    service: SettingsService,
}

fn deps(file: &Arc<FakeSettingsFile>) -> SettingsDeps {
    SettingsDeps {
        file: file.clone(),
        credentials: Arc::new(keys::FakeCredentialStore::new()),
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_709_251_199),
        )),
        call_roots: Arc::new(FakeCallRoots::new()),
    }
}

fn world() -> World {
    let bytes = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let file = Arc::new(FakeSettingsFile::with_bytes(&bytes));
    let (service, _) = SettingsService::load_or_init(deps(&file), OS);
    World { file, service }
}

fn at(x: i32, y: i32) -> IndicatorPosition {
    IndicatorPosition { x, y }
}

#[test]
fn remember_indicator_position_writes_only_that_field() {
    // AC "position saved in calls.indicator_position": the position (negative
    // coordinates included: a monitor left of the primary) is stored, the file is written
    // once, the snapshot carries it, and no other field changes. Bite: a writer that
    // skips the file write (lost at restart), one that touches another field, one that
    // drops the sign or swaps x and y, a write per call for the same value.
    let w = world();
    let before = w.service.snapshot();
    assert_eq!(before.calls.indicator_position, None, "premise");
    let writes_before = w.file.calls().len();
    assert_eq!(
        w.service.remember_indicator_position(at(-1200, 340)),
        Ok(())
    );
    let after = w.service.snapshot();
    assert_eq!(after.calls.indicator_position, Some(at(-1200, 340)));
    let mut expected = (*before).clone();
    expected.calls.indicator_position = Some(at(-1200, 340));
    assert_eq!(*after, expected, "only indicator_position changed");
    assert_eq!(w.file.calls().len(), writes_before + 1, "one file write");
    let stored: Value = serde_json::from_slice(&w.file.bytes().expect("a file")).expect("json");
    assert_eq!(
        stored["calls"]["indicator_position"],
        json!({ "x": -1200, "y": 340 })
    );

    // The same value again writes nothing; a different one writes once more.
    assert_eq!(
        w.service.remember_indicator_position(at(-1200, 340)),
        Ok(())
    );
    assert_eq!(
        w.file.calls().len(),
        writes_before + 1,
        "same value, no write"
    );
    assert_eq!(w.service.remember_indicator_position(at(10, 20)), Ok(()));
    assert_eq!(w.file.calls().len(), writes_before + 2);
    assert_eq!(
        w.service.snapshot().calls.indicator_position,
        Some(at(10, 20))
    );
}

#[test]
fn remember_indicator_position_keeps_the_snapshot_when_the_write_fails() {
    // Failure branch: the file write fails (once): Err, the snapshot and the file are
    // unchanged, a later call succeeds. Bite: snapshot swapped before the write, the
    // error swallowed into Ok, the failure sticky.
    let w = world();
    let before = w.service.snapshot();
    let file_before = w.file.bytes();
    w.file.fail_write(io::ErrorKind::PermissionDenied);
    assert_eq!(
        w.service.remember_indicator_position(at(5, 6)),
        Err(IndicatorPositionFailed)
    );
    assert_eq!(*w.service.snapshot(), *before, "the snapshot is unchanged");
    assert_eq!(w.file.bytes(), file_before, "the file is unchanged");
    w.file.clear_failures();
    assert_eq!(w.service.remember_indicator_position(at(5, 6)), Ok(()));
    assert_eq!(
        w.service.snapshot().calls.indicator_position,
        Some(at(5, 6))
    );
}

#[test]
fn remember_indicator_position_writes_nothing_while_settings_are_unavailable() {
    // decision #19: `Unavailable` writes nothing. Bite: no `load.unavailable` guard (the
    // in-memory defaults written over the unreadable file).
    let file = Arc::new(FakeSettingsFile::new());
    file.fail_read(io::ErrorKind::PermissionDenied);
    let (service, outcome) = SettingsService::load_or_init(deps(&file), OS);
    assert!(
        matches!(outcome, voicen_core::settings::LoadOutcome::Unavailable(_)),
        "precondition: {outcome:?}"
    );
    let before = service.snapshot();
    let calls_before = file.calls();
    assert_eq!(
        service.remember_indicator_position(at(1, 2)),
        Err(IndicatorPositionFailed)
    );
    assert_eq!(file.calls(), calls_before, "no file call, no write");
    assert_eq!(file.bytes(), None, "the unreadable file is not replaced");
    assert_eq!(*service.snapshot(), *before, "the snapshot is unchanged");
}

#[test]
fn remember_indicator_position_publishes_the_new_snapshot() {
    // The narrow writer publishes like every writer (a subscriber sees the position).
    // Bite: swap without publish.
    let w = world();
    let rx = w.service.subscribe();
    assert_eq!(w.service.remember_indicator_position(at(7, 8)), Ok(()));
    let got = rx.try_recv().expect("a published snapshot");
    assert_eq!(got.calls.indicator_position, Some(at(7, 8)));
}
