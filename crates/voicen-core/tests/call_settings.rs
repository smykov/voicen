//! T-120 (spec 007 G06): the `calls` section through the settings service's public
//! API, with a fake `CallRoots` (research R19, decisions #196, #198, invariants
//! I1..I5 of the task's analysis).
//!
//! The probe is a port, so the failure branches (the probe file cannot be created, or
//! cannot be deleted) are exercised with a fake that refuses a chosen root; the real
//! disk is `settings::roots::tests`. Root paths below are never touched on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::secrets::{CredentialOp, FakeCredentialStore, KeyEdits};
use voicen_core::settings::file::{FakeSettingsFile, FileCall};
use voicen_core::settings::roots::{CallRoots, UnwritableReason};
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, ErrorCode, FieldError, FieldId, LoadOutcome, Settings};

const OS: Option<&str> = Some("en-US");
const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../e2e/fixtures/settings-wire.json"
));

/// A fake `CallRoots`: refuses the roots it is told to, records every probe.
struct FakeRoots {
    default: PathBuf,
    failing: Mutex<HashMap<PathBuf, UnwritableReason>>,
    probes: Mutex<Vec<PathBuf>>,
}

impl FakeRoots {
    fn new() -> FakeRoots {
        FakeRoots {
            default: std::env::temp_dir()
                .join("voicen-t120-fake-data")
                .join("calls"),
            failing: Mutex::new(HashMap::new()),
            probes: Mutex::new(Vec::new()),
        }
    }

    fn fail(&self, root: &Path, reason: UnwritableReason) {
        self.failing
            .lock()
            .expect("lock")
            .insert(root.to_path_buf(), reason);
    }

    fn heal(&self, root: &Path) {
        self.failing.lock().expect("lock").remove(root);
    }

    fn probes(&self) -> Vec<PathBuf> {
        self.probes.lock().expect("lock").clone()
    }
}

impl CallRoots for FakeRoots {
    fn default_root(&self) -> PathBuf {
        self.default.clone()
    }

    fn probe(&self, root: &Path) -> Result<(), UnwritableReason> {
        self.probes.lock().expect("lock").push(root.to_path_buf());
        match self.failing.lock().expect("lock").get(root) {
            Some(reason) => Err(*reason),
            None => Ok(()),
        }
    }
}

struct World {
    file: Arc<FakeSettingsFile>,
    creds: Arc<FakeCredentialStore>,
    autostart: Arc<FakeAutostart>,
    hotkeys: Arc<FakeHotkeyRegistrar>,
    roots: Arc<FakeRoots>,
}

impl World {
    fn new(file: FakeSettingsFile) -> World {
        World {
            file: Arc::new(file),
            creds: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            roots: Arc::new(FakeRoots::new()),
        }
    }

    fn first_run() -> World {
        World::new(FakeSettingsFile::new())
    }

    fn with_settings(value: &Value) -> World {
        World::new(FakeSettingsFile::with_bytes(
            &serde_json::to_vec(value).expect("bytes"),
        ))
    }

    fn load(&self) -> (SettingsService, LoadOutcome) {
        SettingsService::load_or_init(
            SettingsDeps {
                file: self.file.clone(),
                credentials: self.creds.clone(),
                autostart: self.autostart.clone(),
                hotkeys: self.hotkeys.clone(),
                local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
                clock: Arc::new(FakeClock::at(
                    UNIX_EPOCH + Duration::from_secs(1_709_251_199),
                )),
                call_roots: self.roots.clone(),
            },
            OS,
        )
    }

    /// The settings file as it is on "disk" now.
    fn stored(&self) -> Settings {
        let bytes = self.file.bytes().expect("a settings file");
        serde_json::from_slice(&bytes).expect("the file is a settings file")
    }

    /// Everything a refused save must leave untouched, to compare before and after.
    fn footprint(&self) -> (Vec<FileCall>, Option<Vec<u8>>, usize, usize, usize) {
        (
            self.file.calls(),
            self.file.bytes(),
            self.hotkeys.calls().len(),
            self.autostart.calls().len(),
            self.creds
                .calls()
                .iter()
                .filter(|c| c.op != CredentialOp::Read)
                .count(),
        )
    }
}

fn req(settings: Settings) -> SaveRequest {
    SaveRequest {
        settings,
        keys: KeyEdits::default(),
    }
}

fn draft(service: &SettingsService) -> Settings {
    (*service.snapshot()).clone()
}

#[track_caller]
fn expect_saved(outcome: SaveOutcome) {
    match outcome {
        SaveOutcome::Saved { .. } => {}
        other => panic!("expected Saved, got {other:?}"),
    }
}

#[track_caller]
fn expect_refused(outcome: SaveOutcome) -> Vec<FieldError> {
    match outcome {
        SaveOutcome::Refused { errors, form_error } => {
            assert_eq!(form_error, None, "no form error expected");
            errors
        }
        other => panic!("expected Refused, got {other:?}"),
    }
}

fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join("voicen-t120-fake-roots").join(name)
}

fn no_duplicates(roots: &[PathBuf]) {
    let mut seen = std::collections::HashSet::new();
    for r in roots {
        assert!(seen.insert(r.clone()), "{r:?} listed twice in {roots:?}");
    }
}

// ---- load: defaults, the older file, the primary-language fill --------------------

#[test]
fn a_release_1_file_without_calls_loads_with_defaults_and_is_not_rewritten() {
    // Acceptance 1 / I1: an older `settings.json` loads (Loaded, no Reset, no backup, no
    // rewrite) with the data-model defaults, SCHEMA_VERSION 1, and a primary language
    // taken from the file's `ui_language`. Bite: the container default missing (the
    // file is reset), the file rewritten at load, the fill missing (`en` for a Russian
    // user) or applied to a value the file has.
    let mut file = serde_json::to_value(defaults(Some("ru-RU"))).expect("serializes");
    file.as_object_mut().expect("object").remove("calls");
    assert_eq!(file["ui_language"], json!("ru"));
    let world = World::with_settings(&file);
    let (service, outcome) = world.load();

    let LoadOutcome::Loaded(loaded) = outcome else {
        panic!("a release-1 file must load, got {outcome:?}");
    };
    let mut expected = defaults(Some("ru-RU"));
    expected.calls.primary_language = "ru".into();
    assert_eq!(loaded, expected);
    assert_eq!(loaded.schema_version, 1);
    assert_eq!(loaded.calls.hotkey, "Ctrl+Alt+R");
    assert!(loaded.calls.reminder);
    assert_eq!(loaded.calls.storage_root, None);
    assert_eq!(*service.snapshot(), expected);
    assert_eq!(world.file.calls(), vec![FileCall::Read], "read once, no write");
    assert!(world.file.backups().is_empty());
}

#[test]
fn an_explicit_primary_language_in_the_file_is_kept() {
    // The fill is for a missing value only. Bite: the fill overwrites.
    let mut file = serde_json::to_value(defaults(Some("ru-RU"))).expect("serializes");
    file["calls"]["primary_language"] = json!("de");
    let (service, outcome) = World::with_settings(&file).load();
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");
    assert_eq!(service.snapshot().calls.primary_language, "de");
}

#[test]
fn first_run_takes_the_primary_language_from_the_os_language() {
    // FirstRun, Reset and Unavailable get it from `defaults(os_tag)` (no second fill).
    let world = World::first_run();
    let (service, outcome) = SettingsService::load_or_init(
        SettingsDeps {
            file: world.file.clone(),
            credentials: world.creds.clone(),
            autostart: world.autostart.clone(),
            hotkeys: world.hotkeys.clone(),
            local_models: Arc::new(FakeDownloadedModels::new(&[])),
            clock: Arc::new(FakeClock::at(UNIX_EPOCH + Duration::from_secs(1))),
            call_roots: world.roots.clone(),
        },
        Some("ru-RU"),
    );
    assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
    assert_eq!(service.snapshot().calls.primary_language, "ru");
    assert_eq!(world.stored().calls.primary_language, "ru");
    assert_eq!(world.stored(), *service.snapshot());
}

// ---- save: the call hotkey --------------------------------------------------------

#[test]
fn a_call_hotkey_equal_to_the_dictation_hotkey_is_refused_from_either_side() {
    // Acceptance 2, through the service: refused on `calls.hotkey`, before any side
    // effect (the hotkey registrar, autostart, the file). Bite: the rule only in
    // `validate` of a draft whose call hotkey changed; the dictation side not covered;
    // the refusal after the hotkey `prepare`.
    for side in ["dictation hotkey changed", "call hotkey changed"] {
        let world = World::first_run();
        let (service, _) = world.load();
        let before = world.footprint();

        let mut d = draft(&service);
        d.start_with_windows = true; // would call autostart if the save got that far
        match side {
            "dictation hotkey changed" => d.hotkey = d.calls.hotkey.clone(),
            _ => d.calls.hotkey = d.hotkey.clone(),
        }
        let errors = expect_refused(service.save(req(d)));
        assert_eq!(
            errors,
            vec![FieldError::new(
                FieldId::CallHotkey,
                ErrorCode::HotkeyEqualsDictation
            )],
            "{side}"
        );
        assert_eq!(errors[0].reason, None);
        assert_eq!(world.footprint(), before, "{side}: a refused save touched something");
        assert_eq!(*service.snapshot(), defaults(OS));
    }
}

#[test]
fn a_dictation_hotkey_change_to_the_call_hotkey_never_reaches_the_registrar() {
    // Same rule, the registrar's view: `prepare` of the new dictation hotkey must not
    // be called for a draft the rule refuses. Bite: the equality check placed after
    // step (4).
    let world = World::first_run();
    let (service, _) = world.load();
    let mut d = draft(&service);
    d.hotkey = "Ctrl+Alt+R".into(); // the default call hotkey
    let errors = expect_refused(service.save(req(d)));
    assert_eq!(
        errors,
        vec![FieldError::new(
            FieldId::CallHotkey,
            ErrorCode::HotkeyEqualsDictation
        )]
    );
    assert_eq!(world.hotkeys.calls(), vec![]);
}

#[test]
fn release_1_file_with_dictation_hotkey_ctrl_alt_r_loads_but_saves_are_refused_until_one_changes() {
    // Edge (Investigation): nothing validates at load, so the file loads (no Reset);
    // then every save is refused on `calls.hotkey` until one of the hotkeys changes.
    // Bite: validation at load (reset), the default call hotkey exempted, or a refusal
    // on `recording.hotkey`.
    let mut file = serde_json::to_value(defaults(OS)).expect("serializes");
    file.as_object_mut().expect("object").remove("calls");
    file["hotkey"] = json!("Ctrl+Alt+R");
    let world = World::with_settings(&file);
    let (service, outcome) = world.load();
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");
    assert_eq!(service.snapshot().hotkey, "Ctrl+Alt+R");
    assert_eq!(service.snapshot().calls.hotkey, "Ctrl+Alt+R");
    let before = world.footprint();

    let mut unrelated = draft(&service);
    unrelated.history.size = 5;
    let errors = expect_refused(service.save(req(unrelated.clone())));
    assert_eq!(
        errors,
        vec![FieldError::new(
            FieldId::CallHotkey,
            ErrorCode::HotkeyEqualsDictation
        )]
    );
    assert_eq!(world.footprint(), before);

    // Changing the call hotkey is enough (the dictation hotkey is not re-registered).
    unrelated.calls.hotkey = "Ctrl+Alt+T".into();
    expect_saved(service.save(req(unrelated)));
    assert_eq!(service.snapshot().calls.hotkey, "Ctrl+Alt+T");
    assert_eq!(service.snapshot().hotkey, "Ctrl+Alt+R");
    assert_eq!(world.hotkeys.calls(), vec![], "dictation hotkey unchanged");
    assert_eq!(world.stored().calls.hotkey, "Ctrl+Alt+T");
}

// ---- save: the primary language ---------------------------------------------------

#[test]
fn an_unknown_primary_language_is_refused_and_nothing_is_written() {
    // Acceptance 5. Bite: no rule in the service path; a repair to a default instead of a
    // refusal.
    for bad in ["xx", "EN", "", "en-US", "jw"] {
        let world = World::first_run();
        let (service, _) = world.load();
        let before = world.footprint();
        let mut d = draft(&service);
        d.calls.primary_language = bad.into();
        let errors = expect_refused(service.save(req(d)));
        assert_eq!(
            errors,
            vec![FieldError::new(
                FieldId::PrimaryLanguage,
                ErrorCode::LanguageUnsupported
            )],
            "{bad:?}"
        );
        assert_eq!(world.footprint(), before, "{bad:?}");
        assert_eq!(service.snapshot().calls.primary_language, "en");
    }
}

#[test]
fn a_valid_primary_language_is_saved_as_entered() {
    let world = World::first_run();
    let (service, _) = world.load();
    let mut d = draft(&service);
    d.calls.primary_language = "ru".into();
    d.calls.reminder = false;
    expect_saved(service.save(req(d)));
    assert_eq!(service.snapshot().calls.primary_language, "ru");
    assert!(!service.snapshot().calls.reminder);
    assert_eq!(world.stored().calls.primary_language, "ru");
}

// ---- save: the storage root probe -------------------------------------------------

#[test]
fn an_unwritable_root_is_refused_on_the_storage_root_with_its_reason_and_nothing_is_written() {
    // Acceptance 3 / I2 / I5. The fake plays "the probe file cannot be created"
    // (access denied) and "cannot be deleted" (other, the io kind of a delete failure):
    // either way the whole save is refused on `calls.storage_root` with
    // `storage.unwritable` and the closed reason, before the registrar, autostart,
    // keys and file are touched. Bite: probe after step (4) (the registrar sees a
    // prepare), a reason dropped or replaced by a constant, the refusal on another
    // field, the root accepted without a probe.
    for reason in [
        UnwritableReason::Missing,
        UnwritableReason::NotAFolder,
        UnwritableReason::Relative,
        UnwritableReason::AccessDenied,
        UnwritableReason::DiskFull,
        UnwritableReason::Other,
    ] {
        let world = World::first_run();
        let (service, _) = world.load();
        let (_, snapshot_rx) = (0, service.subscribe());
        let chosen = root("usb");
        world.roots.fail(&chosen, reason);
        let before = world.footprint();

        let mut d = draft(&service);
        d.calls.storage_root = Some(chosen.clone());
        d.hotkey = "Ctrl+Shift+F9".into(); // would be prepared at step (4)
        d.start_with_windows = true; // would be set at step (5)
        let outcome = service.save(req(d));
        let wire = serde_json::to_value(&outcome).expect("serializes");
        let errors = expect_refused(outcome);

        assert_eq!(errors, vec![FieldError::storage_unwritable(reason)], "{reason:?}");
        assert_eq!(errors[0].field, FieldId::StorageRoot);
        assert_eq!(errors[0].code, ErrorCode::StorageUnwritable);
        assert_eq!(errors[0].reason, Some(reason));
        assert_eq!(
            wire["Refused"]["errors"][0]["reason"],
            serde_json::to_value(reason).expect("serializes"),
            "the reason is on the wire"
        );
        assert_eq!(wire["Refused"]["errors"][0]["field"], json!("calls.storage_root"));
        assert_eq!(wire["Refused"]["errors"][0]["code"], json!("storage.unwritable"));
        assert_eq!(world.roots.probes(), vec![chosen], "{reason:?}: probed once");
        assert_eq!(world.footprint(), before, "{reason:?}: a refused save touched something");
        assert_eq!(world.hotkeys.calls(), vec![]);
        assert_eq!(*service.snapshot(), defaults(OS));
        assert_eq!(snapshot_rx.try_recv(), Err(TryRecvError::Empty), "nothing published");
    }
}

#[test]
fn no_error_but_the_unwritable_root_carries_a_reason() {
    // I5: `reason` only for `storage.unwritable`. Every other refusal of the section
    // has none, on the struct and on the wire (the key is absent).
    let world = World::first_run();
    let (service, _) = world.load();
    let mut d = draft(&service);
    d.calls.hotkey = d.hotkey.clone();
    d.calls.primary_language = "xx".into();
    let outcome = service.save(req(d));
    let wire = serde_json::to_value(&outcome).expect("serializes");
    let errors = expect_refused(outcome);
    assert_eq!(errors.len(), 2);
    for (error, wire_error) in errors.iter().zip(wire["Refused"]["errors"].as_array().expect("array")) {
        assert_eq!(error.reason, None);
        assert!(wire_error.get("reason").is_none(), "{wire_error}");
        assert_eq!(wire_error.as_object().expect("object").len(), 2, "{wire_error}");
    }
}

#[test]
fn validation_errors_win_and_no_probe_runs_for_a_draft_that_already_failed() {
    // The probe is a side effect (it creates and deletes a file, or the default folder):
    // it runs only for a draft that passed `validate`. Bite: the probe before validate.
    let world = World::first_run();
    let (service, _) = world.load();
    let chosen = root("usb");
    world.roots.fail(&chosen, UnwritableReason::AccessDenied);
    let mut d = draft(&service);
    d.calls.storage_root = Some(chosen);
    d.calls.primary_language = "xx".into();
    let errors = expect_refused(service.save(req(d)));
    assert_eq!(
        errors,
        vec![FieldError::new(
            FieldId::PrimaryLanguage,
            ErrorCode::LanguageUnsupported
        )]
    );
    assert_eq!(world.roots.probes(), vec![]);
}

#[test]
fn an_unchanged_unwritable_root_does_not_block_an_unrelated_save() {
    // I2 "when the effective root differs from the snapshot in force": a chosen root on
    // an unplugged drive must not make every other setting unsaveable (the autostart
    // step runs only on change for the same reason). Bite: the probe on every save.
    let mut file = serde_json::to_value(defaults(OS)).expect("serializes");
    let chosen = root("unplugged-usb");
    file["calls"]["storage_root"] = json!(chosen);
    let world = World::with_settings(&file);
    let (service, outcome) = world.load();
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");
    world.roots.fail(&chosen, UnwritableReason::Missing);
    world.roots.fail(&world.roots.default_root(), UnwritableReason::AccessDenied);

    let mut d = draft(&service);
    d.history.size = 5;
    expect_saved(service.save(req(d)));
    assert_eq!(service.snapshot().history.size, 5);
    assert_eq!(service.snapshot().calls.storage_root, Some(chosen));
    assert_eq!(world.roots.probes(), vec![], "no probe for an unchanged root");
}

#[test]
fn the_default_root_written_out_or_left_empty_is_the_same_root() {
    // data-model: `None` = the default. `Some(default)` and `Some("")` are the same
    // effective root as `None`: no probe, no earlier-roots entry. Bite: comparing the
    // raw `Option`s, `Some("")` kept as a second encoding of the default.
    let world = World::first_run();
    let (service, _) = world.load();
    let default_root = world.roots.default_root();

    let mut d = draft(&service);
    d.calls.storage_root = Some(default_root.clone());
    expect_saved(service.save(req(d)));
    assert_eq!(world.roots.probes(), vec![]);
    assert_eq!(service.snapshot().calls.earlier_roots, Vec::<PathBuf>::new());

    let mut d = draft(&service);
    d.calls.storage_root = Some(PathBuf::new());
    expect_saved(service.save(req(d)));
    assert_eq!(
        service.snapshot().calls.storage_root,
        None,
        "an empty root is stored as the default (one encoding)"
    );
    assert_eq!(world.stored().calls.storage_root, None);
    assert_eq!(world.roots.probes(), vec![]);
    assert_eq!(service.snapshot().calls.earlier_roots, Vec::<PathBuf>::new());
}

#[test]
fn a_changed_root_is_probed_once_and_the_old_one_joins_the_earlier_roots_without_duplicates() {
    // Acceptance 4 / I3: A -> B -> A -> default. After every accepted save
    // `earlier_roots` has no duplicates, never holds the current effective root, and
    // holds every effective root ever replaced. The default root counts as a root
    // (`None` is its spelling). Bite: no append, an append on every save, no removal of
    // the new root from the list (A -> B -> A leaves A twice), the default path not
    // appended when the old root was `None`, a list that is not persisted.
    let world = World::first_run();
    let (service, _) = world.load();
    let default_root = world.roots.default_root();
    let (a, b) = (root("a"), root("b"));

    let mut step = |new_root: Option<&PathBuf>| {
        let mut d = draft(&service);
        d.calls.storage_root = new_root.cloned();
        expect_saved(service.save(req(d)));
        let snapshot = service.snapshot();
        no_duplicates(&snapshot.calls.earlier_roots);
        let effective = snapshot
            .calls
            .storage_root
            .clone()
            .unwrap_or_else(|| default_root.clone());
        assert!(
            !snapshot.calls.earlier_roots.contains(&effective),
            "the current root {effective:?} is in {:?}",
            snapshot.calls.earlier_roots
        );
        assert_eq!(world.stored(), *snapshot, "the file holds what the service holds");
        snapshot.calls.earlier_roots.clone()
    };

    assert_eq!(step(Some(&a)), vec![default_root.clone()], "None -> A");
    assert_eq!(step(Some(&b)), vec![default_root.clone(), a.clone()], "A -> B");
    assert_eq!(step(Some(&a)), vec![default_root.clone(), b.clone()], "B -> A");
    assert_eq!(step(Some(&a)), vec![default_root.clone(), b.clone()], "A -> A");
    assert_eq!(step(None), vec![b.clone(), a.clone()], "A -> default");
    assert_eq!(
        world.roots.probes(),
        vec![a.clone(), b, a, default_root],
        "one probe per changed root, none for the unchanged save"
    );
}

#[test]
fn a_draft_cannot_overwrite_the_server_owned_fields() {
    // I3: `earlier_roots`, `last_combination`, `warned_hosts` and `indicator_position`
    // come from the snapshot in force, never from the draft (the window sends the whole
    // draft). Bite: the draft's values stored; the draft's `earlier_roots` used as the
    // base of the append.
    let a = root("a");
    let b = root("b");
    let mut file = serde_json::to_value(defaults(OS)).expect("serializes");
    file["calls"]["storage_root"] = json!(a);
    file["calls"]["earlier_roots"] = json!([root("old-1")]);
    file["calls"]["last_combination"] = json!({ "record": false });
    file["calls"]["warned_hosts"] = json!(["api.example.com"]);
    file["calls"]["indicator_position"] = json!({ "x": 10, "y": 20 });
    let world = World::with_settings(&file);
    let (service, outcome) = world.load();
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");
    let owned = |s: &Settings| {
        let v = serde_json::to_value(s).expect("serializes");
        (
            v["calls"]["earlier_roots"].clone(),
            v["calls"]["last_combination"].clone(),
            v["calls"]["warned_hosts"].clone(),
            v["calls"]["indicator_position"].clone(),
        )
    };

    // A draft that lies about all four and changes ordinary fields.
    let tamper = |s: &Settings| -> Settings {
        let mut v = serde_json::to_value(s).expect("serializes");
        v["calls"]["earlier_roots"] = json!([root("evil")]);
        v["calls"]["last_combination"] = json!({ "record": true });
        v["calls"]["warned_hosts"] = json!([]);
        v["calls"]["indicator_position"] = Value::Null;
        serde_json::from_value(v).expect("a draft")
    };

    // No root change: the four stay as they were.
    let before = owned(&service.snapshot());
    let mut d = tamper(&draft(&service));
    d.calls.reminder = false;
    expect_saved(service.save(req(d)));
    assert_eq!(owned(&service.snapshot()), before);
    assert!(!service.snapshot().calls.reminder, "ordinary fields come from the draft");
    assert_eq!(owned(&world.stored()), before);

    // A root change: the old root joins the snapshot's list, not the draft's.
    let mut d = tamper(&draft(&service));
    d.calls.storage_root = Some(b.clone());
    expect_saved(service.save(req(d)));
    let after = owned(&service.snapshot());
    assert_eq!(after.0, json!([root("old-1"), a]));
    assert_eq!(after.1, before.1);
    assert_eq!(after.2, before.2);
    assert_eq!(after.3, before.3);
    assert_eq!(service.snapshot().calls.storage_root, Some(b));
    assert_eq!(owned(&world.stored()), after);
}

#[test]
fn a_root_that_became_writable_again_is_accepted_and_a_failed_try_leaves_the_list_alone() {
    // A refused root change records nothing in `earlier_roots` (the old root is still
    // the current one); the same root accepted later appends once. Bite: the append
    // before the probe.
    let world = World::first_run();
    let (service, _) = world.load();
    let chosen = root("usb");
    world.roots.fail(&chosen, UnwritableReason::Missing);
    let mut d = draft(&service);
    d.calls.storage_root = Some(chosen.clone());
    let errors = expect_refused(service.save(req(d.clone())));
    assert_eq!(
        errors,
        vec![FieldError::storage_unwritable(UnwritableReason::Missing)]
    );
    assert_eq!(service.snapshot().calls.earlier_roots, Vec::<PathBuf>::new());

    world.roots.heal(&chosen);
    expect_saved(service.save(req(d)));
    assert_eq!(
        service.snapshot().calls.earlier_roots,
        vec![world.roots.default_root()]
    );
    assert_eq!(service.snapshot().calls.storage_root, Some(chosen));
}

// ---- the wire fixture -------------------------------------------------------------

#[test]
fn e2e_settings_wire_fixture_refused_storage_unwritable_matches_core() {
    // T-120 / P-010: the refusal G41's Playwright scenarios script is core's real one,
    // reason included (no hand-written SaveOutcome in e2e). `refused_storage_unwritable`
    // is the wire of a save, over a first run with no OS language and no keys, of
    // defaults(None) with `calls.storage_root` set to an unwritable folder (access
    // denied): Refused with calls.storage_root `storage.unwritable` and reason
    // `access_denied`, no form error. Bite: no reason on the wire, a field or code
    // renamed, without adding the entry to e2e/fixtures/settings-wire.json.
    let world = World::first_run();
    let (service, outcome) = SettingsService::load_or_init(
        SettingsDeps {
            file: world.file.clone(),
            credentials: world.creds.clone(),
            autostart: world.autostart.clone(),
            hotkeys: world.hotkeys.clone(),
            local_models: Arc::new(FakeDownloadedModels::new(&[])),
            clock: Arc::new(FakeClock::at(UNIX_EPOCH + Duration::from_secs(1))),
            call_roots: world.roots.clone(),
        },
        None,
    );
    assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
    let chosen = root("unwritable");
    world.roots.fail(&chosen, UnwritableReason::AccessDenied);
    let mut d = defaults(None);
    d.calls.storage_root = Some(chosen);
    let wire = serde_json::to_value(service.save(req(d))).expect("serializes");

    assert_eq!(
        wire,
        json!({ "Refused": {
            "errors": [ {
                "field": "calls.storage_root",
                "code": "storage.unwritable",
                "reason": "access_denied",
            } ],
            "form_error": null,
        } })
    );
    let fixture: Value = serde_json::from_str(FIXTURE).expect("settings-wire.json is valid JSON");
    assert_eq!(
        fixture["refused_storage_unwritable"],
        wire,
        "e2e/fixtures/settings-wire.json refused_storage_unwritable differs from core; core says:\n{}",
        serde_json::to_string_pretty(&wire).expect("pretty")
    );
}

#[test]
fn the_fixtures_first_run_view_carries_the_calls_section() {
    // The first-run view of the fixture is the view a window receives: its `calls`
    // section is the data-model defaults. (The whole-view comparison is
    // `service::tests::e2e_settings_wire_fixture_matches_core`; this one names the
    // section so a failure says what is missing.) Bite: the fixture not regenerated.
    let fixture: Value = serde_json::from_str(FIXTURE).expect("settings-wire.json is valid JSON");
    assert_eq!(
        fixture["first_run_view"]["settings"]["calls"],
        json!({
            "hotkey": "Ctrl+Alt+R",
            "last_combination": { "record": true },
            "reminder": true,
            "primary_language": "en",
            "storage_root": null,
            "earlier_roots": [],
            "warned_hosts": [],
            "indicator_position": null,
        })
    );
}
