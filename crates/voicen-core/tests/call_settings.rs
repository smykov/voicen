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
use voicen_core::hotkey_registrar::{FakeHotkeyRegistrar, RegistrarCall};
use voicen_core::models::FakeDownloadedModels;
use voicen_core::secrets::{CredentialOp, FakeCredentialStore, KeyEdits};
use voicen_core::settings::file::{FakeSettingsFile, FileCall};
use voicen_core::settings::hotkey::parse_hotkey;
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
    std::env::temp_dir()
        .join("voicen-t120-fake-roots")
        .join(name)
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
    assert_eq!(
        world.file.calls(),
        vec![FileCall::Read],
        "read once, no write"
    );
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
        assert_eq!(
            world.footprint(),
            before,
            "{side}: a refused save touched something"
        );
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

    // Changing the call hotkey is enough (the dictation hotkey is not re-registered: the
    // registrar sees only the call slot prepared and committed, T-129 I4).
    unrelated.calls.hotkey = "Ctrl+Alt+T".into();
    expect_saved(service.save(req(unrelated)));
    assert_eq!(service.snapshot().calls.hotkey, "Ctrl+Alt+T");
    assert_eq!(service.snapshot().hotkey, "Ctrl+Alt+R");
    let new_call = parse_hotkey("Ctrl+Alt+T").expect("a canonical test hotkey");
    assert_eq!(
        world.hotkeys.calls(),
        vec![
            RegistrarCall::PrepareCall(new_call),
            RegistrarCall::Commit(new_call)
        ],
        "only the call slot is prepared and committed; the dictation hotkey is not re-registered"
    );
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

        assert_eq!(
            errors,
            vec![FieldError::storage_unwritable(reason)],
            "{reason:?}"
        );
        assert_eq!(errors[0].field, FieldId::StorageRoot);
        assert_eq!(errors[0].code, ErrorCode::StorageUnwritable);
        assert_eq!(errors[0].reason, Some(reason));
        assert_eq!(
            wire["Refused"]["errors"][0]["reason"],
            serde_json::to_value(reason).expect("serializes"),
            "the reason is on the wire"
        );
        assert_eq!(
            wire["Refused"]["errors"][0]["field"],
            json!("calls.storage_root")
        );
        assert_eq!(
            wire["Refused"]["errors"][0]["code"],
            json!("storage.unwritable")
        );
        assert_eq!(
            world.roots.probes(),
            vec![chosen],
            "{reason:?}: probed once"
        );
        assert_eq!(
            world.footprint(),
            before,
            "{reason:?}: a refused save touched something"
        );
        assert_eq!(world.hotkeys.calls(), vec![]);
        assert_eq!(*service.snapshot(), defaults(OS));
        assert_eq!(
            snapshot_rx.try_recv(),
            Err(TryRecvError::Empty),
            "nothing published"
        );
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
    for (error, wire_error) in errors
        .iter()
        .zip(wire["Refused"]["errors"].as_array().expect("array"))
    {
        assert_eq!(error.reason, None);
        assert!(wire_error.get("reason").is_none(), "{wire_error}");
        assert_eq!(
            wire_error.as_object().expect("object").len(),
            2,
            "{wire_error}"
        );
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
    assert_eq!(world.roots.probes(), Vec::<PathBuf>::new());
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
    world
        .roots
        .fail(&world.roots.default_root(), UnwritableReason::AccessDenied);

    let mut d = draft(&service);
    d.history.size = 5;
    expect_saved(service.save(req(d)));
    assert_eq!(service.snapshot().history.size, 5);
    assert_eq!(service.snapshot().calls.storage_root, Some(chosen));
    assert_eq!(
        world.roots.probes(),
        Vec::<PathBuf>::new(),
        "no probe for an unchanged root"
    );
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
    assert_eq!(world.roots.probes(), Vec::<PathBuf>::new());
    assert_eq!(
        service.snapshot().calls.earlier_roots,
        Vec::<PathBuf>::new()
    );

    let mut d = draft(&service);
    d.calls.storage_root = Some(PathBuf::new());
    expect_saved(service.save(req(d)));
    assert_eq!(
        service.snapshot().calls.storage_root,
        None,
        "an empty root is stored as the default (one encoding)"
    );
    assert_eq!(world.stored().calls.storage_root, None);
    assert_eq!(world.roots.probes(), Vec::<PathBuf>::new());
    assert_eq!(
        service.snapshot().calls.earlier_roots,
        Vec::<PathBuf>::new()
    );
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

    let step = |new_root: Option<&PathBuf>| {
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
        assert_eq!(
            world.stored(),
            *snapshot,
            "the file holds what the service holds"
        );
        snapshot.calls.earlier_roots.clone()
    };

    assert_eq!(step(Some(&a)), vec![default_root.clone()], "None -> A");
    assert_eq!(
        step(Some(&b)),
        vec![default_root.clone(), a.clone()],
        "A -> B"
    );
    assert_eq!(
        step(Some(&a)),
        vec![default_root.clone(), b.clone()],
        "B -> A"
    );
    assert_eq!(
        step(Some(&a)),
        vec![default_root.clone(), b.clone()],
        "A -> A"
    );
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
    assert!(
        !service.snapshot().calls.reminder,
        "ordinary fields come from the draft"
    );
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
fn a_hand_edited_file_that_lists_the_current_root_does_not_make_a_duplicate() {
    // I3: `earlier_roots` has no duplicates after an accepted save, even when the file
    // on disk already lists the root in force (hand-edited). Bite: the old root pushed
    // without the `contains` check.
    let a = root("a");
    let b = root("b");
    let mut file = serde_json::to_value(defaults(OS)).expect("serializes");
    file["calls"]["storage_root"] = json!(a);
    file["calls"]["earlier_roots"] = json!([a]);
    let world = World::with_settings(&file);
    let (service, outcome) = world.load();
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");
    let mut d = draft(&service);
    d.calls.storage_root = Some(b.clone());
    expect_saved(service.save(req(d)));
    let earlier = service.snapshot().calls.earlier_roots.clone();
    no_duplicates(&earlier);
    assert_eq!(earlier, vec![a]);
    assert_eq!(world.stored().calls.earlier_roots, earlier);
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
    assert_eq!(
        service.snapshot().calls.earlier_roots,
        Vec::<PathBuf>::new()
    );

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

// ---- T-155: the provider host on the settings view, and the Calls-tab wire samples ---
//
// The Calls tab shows "The other party's audio goes to provider {host}" (FR-031) from
// a host core computed (the UI classifies no URL, P-010): `SettingsView.provider_host`,
// text or null, from `check_base_url(api.base_url)` then `url::provider_host`, the one
// rule `CallSession::new_provider_host` uses. Everything below reads the serialized
// wire (`serde_json::Value`), so a missing field is an assertion failure.

/// A first-run service as the fixture is made: no OS language, no downloaded models.
fn fixture_service(world: &World) -> SettingsService {
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
    service
}

/// The wire of the view after a save of defaults(None) with `api.base_url` = `url`
/// (engine none, so the URL is not validated and the save goes through).
fn view_wire_with_api_url(url: &str) -> Value {
    let world = World::first_run();
    let service = fixture_service(&world);
    let mut d = defaults(None);
    d.api.base_url = url.into();
    let saved = service.save(req(d));
    let SaveOutcome::Saved { view, .. } = saved else {
        panic!("expected Saved for {url:?}, got {saved:?}");
    };
    let from_save = serde_json::to_value(view).expect("serializes");
    let from_view = serde_json::to_value(service.view()).expect("serializes");
    assert_eq!(
        from_save["provider_host"], from_view["provider_host"],
        "the Saved view and view() must carry the same provider_host"
    );
    from_view
}

#[test]
fn view_carries_provider_host_for_a_remote_api_url() {
    // FR-031 / I4: the default `https://api.openai.com/v1` gives the text
    // "api.openai.com"; other remote hosts give their host text, whatever the scheme
    // or port. Bite: the field missing, always null, taken from local_server.
    for (url, host) in [
        ("https://api.openai.com/v1", "api.openai.com"),
        ("http://example.com/v1", "example.com"),
        ("https://192.0.2.5:8443/v1", "192.0.2.5"),
    ] {
        let wire = view_wire_with_api_url(url);
        assert_eq!(wire["provider_host"], json!(host), "{url}: {wire}");
    }
}

#[test]
fn view_provider_host_is_null_for_loopback_api_urls() {
    // The line is absent for a loopback host (FR-031). The key must be present with a
    // JSON null, not missing. Bite: no loopback check, loopback missing for one
    // spelling (`localhost`, `127.0.0.1`, `[::1]`).
    for url in [
        "http://localhost:8000/v1",
        "http://127.0.0.1:8000/v1",
        "http://[::1]:8000/v1",
    ] {
        let wire = view_wire_with_api_url(url);
        let object = wire.as_object().expect("view is an object");
        assert!(object.contains_key("provider_host"), "{url}: {wire}");
        assert_eq!(wire["provider_host"], Value::Null, "{url}: {wire}");
    }
}

#[test]
fn view_provider_host_is_null_when_the_api_url_is_not_a_valid_url() {
    // A URL `check_base_url` refuses (empty, malformed, a scheme that is not http(s))
    // gives no host: null, never the raw text. Bite: the raw base_url (or a naive
    // split) used when the check fails.
    for url in ["", "not a url", "ftp://files.example.com/v1"] {
        let wire = view_wire_with_api_url(url);
        let object = wire.as_object().expect("view is an object");
        assert!(object.contains_key("provider_host"), "{url:?}: {wire}");
        assert_eq!(wire["provider_host"], Value::Null, "{url:?}: {wire}");
    }
}

#[test]
fn view_provider_host_follows_the_api_url_not_the_local_server_url() {
    // The line is for the API engine's host whatever `engine` is (the call session's
    // final engine is always the API one): a remote api URL with a loopback
    // local_server URL gives the api host, and the reverse gives null. Bite: computed
    // from local_server.base_url, or from the selected engine's URL.
    let world = World::first_run();
    let service = fixture_service(&world);
    let mut d = defaults(None);
    d.api.base_url = "https://api.example.com/v1".into();
    d.local_server.base_url = "http://localhost:8000/v1".into();
    expect_saved(service.save(req(d.clone())));
    let wire = serde_json::to_value(service.view()).expect("serializes");
    assert_eq!(wire["provider_host"], json!("api.example.com"), "{wire}");

    d.api.base_url = "http://localhost:8000/v1".into();
    d.local_server.base_url = "https://remote.example.com/v1".into();
    expect_saved(service.save(req(d)));
    let wire = serde_json::to_value(service.view()).expect("serializes");
    assert_eq!(wire["provider_host"], Value::Null, "{wire}");
}

#[test]
fn the_fixtures_first_run_view_carries_the_provider_host() {
    // The first-run view a window receives names the default API host. (The
    // whole-view comparison is `service::tests::e2e_settings_wire_fixture_matches_core`;
    // this one names the key so a failure says what is missing.) Bite: the fixture not
    // regenerated, or a view without the field.
    let fixture: Value = serde_json::from_str(FIXTURE).expect("settings-wire.json is valid JSON");
    assert_eq!(
        fixture["first_run_view"]["provider_host"],
        json!("api.openai.com")
    );
    let world = World::first_run();
    let service = fixture_service(&world);
    let wire = serde_json::to_value(service.view()).expect("serializes");
    assert_eq!(wire["provider_host"], json!("api.openai.com"), "{wire}");
    assert_eq!(fixture["first_run_view"], wire);
}

#[test]
fn e2e_settings_wire_fixture_loopback_api_view_matches_core() {
    // T-155: the Playwright "no provider line" scenario scripts core's real view.
    // `loopback_api_view` is the SettingsView wire (`service.view()`) of a first run
    // with no OS language and no keys after a save of defaults(None) with
    // api.base_url `http://localhost:8000/v1`: provider_host null. Bite: the key not
    // in e2e/fixtures/settings-wire.json, a hand-edited host, a stale view shape.
    let wire = view_wire_with_api_url("http://localhost:8000/v1");
    assert_eq!(
        wire["settings"]["api"]["base_url"],
        "http://localhost:8000/v1"
    );
    assert_eq!(wire["provider_host"], Value::Null);
    let fixture: Value = serde_json::from_str(FIXTURE).expect("settings-wire.json is valid JSON");
    assert_eq!(
        fixture["loopback_api_view"],
        wire,
        "e2e/fixtures/settings-wire.json loopback_api_view differs from core; core says:\n{}",
        serde_json::to_string_pretty(&wire).expect("pretty")
    );
}

/// The Refused wire of a first-run save of `defaults(None)` after `edit`, with the
/// registrar set up by `setup`; the errors must be exactly `calls.hotkey` / `code`.
fn refused_hotkey_wire(
    edit: impl FnOnce(&mut Settings),
    setup: impl FnOnce(&FakeHotkeyRegistrar),
    code: &str,
) -> Value {
    let world = World::first_run();
    setup(&world.hotkeys);
    let service = fixture_service(&world);
    let mut d = defaults(None);
    edit(&mut d);
    let wire = serde_json::to_value(service.save(req(d))).expect("serializes");
    assert_eq!(
        wire["Refused"]["errors"],
        json!([ { "field": "calls.hotkey", "code": code } ]),
        "{wire}"
    );
    assert_eq!(wire["Refused"]["form_error"], Value::Null, "{wire}");
    wire
}

fn assert_fixture_key(key: &str, wire: &Value) {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("settings-wire.json is valid JSON");
    assert_eq!(
        fixture[key],
        *wire,
        "e2e/fixtures/settings-wire.json {key} differs from core; core says:\n{}",
        serde_json::to_string_pretty(wire).expect("pretty")
    );
}

#[test]
fn e2e_settings_wire_fixture_refused_call_hotkey_equals_dictation_matches_core() {
    // T-155 / FR-004: the call side changed to the dictation hotkey. Core's real
    // refusal: calls.hotkey `hotkey.equals_dictation`, no form error. Bite: the error
    // on `recording.hotkey`, another code, the key not in the fixture.
    let wire = refused_hotkey_wire(
        |d| d.calls.hotkey = d.hotkey.clone(),
        |_| {},
        "hotkey.equals_dictation",
    );
    assert_fixture_key("refused_call_hotkey_equals_dictation", &wire);
}

#[test]
fn e2e_settings_wire_fixture_refused_dictation_hotkey_equals_call_matches_core() {
    // T-155 / FR-004: the dictation side changed to the call hotkey (`Ctrl+Alt+R`).
    // The error still sits on `calls.hotkey`, never on `recording.hotkey` (the Calls
    // tab's field; on another tab it is listed at form level). Bite: the error moved
    // to the dictation field, the key not in the fixture.
    let wire = refused_hotkey_wire(
        |d| d.hotkey = d.calls.hotkey.clone(),
        |_| {},
        "hotkey.equals_dictation",
    );
    assert_fixture_key("refused_dictation_hotkey_equals_call", &wire);
}

#[test]
fn e2e_settings_wire_fixture_refused_call_hotkey_unavailable_matches_core() {
    // T-155 / T-129: a save-time registration conflict on a changed call hotkey: the
    // registrar refuses `prepare_call` -> calls.hotkey `hotkey.unavailable`. Bite: the
    // error on `recording.hotkey`, another code, the key not in the fixture.
    let wire = refused_hotkey_wire(
        |d| d.calls.hotkey = "Ctrl+Alt+F10".into(),
        |hotkeys| hotkeys.fail_prepare_call(true),
        "hotkey.unavailable",
    );
    assert_fixture_key("refused_call_hotkey_unavailable", &wire);
}

// ---- T-232: `SettingsView.call_hotkey_unavailable` (decisions #306, #373) ------------
//
// The Calls tab marks the call hotkey after a startup registration conflict. The fact
// lives in the hotkey thread; core reads it at build time in `view_with` through
// `HotkeyRegistrar::call_hotkey_unavailable()` (default false), so view(), the Saved
// view and the change bridge carry the same bool. The fake gets
// `set_call_hotkey_unavailable(bool)` and clears the state when a call-slot
// registration is committed (as the shell thread's `call_result(true)` does).

fn unavailable_of(view: voicen_core::settings::service::SettingsView) -> Value {
    serde_json::to_value(view).expect("serializes")["call_hotkey_unavailable"].clone()
}

#[track_caller]
fn saved_view(outcome: SaveOutcome) -> voicen_core::settings::service::SettingsView {
    match outcome {
        SaveOutcome::Saved { view, .. } => view,
        other => panic!("expected Saved, got {other:?}"),
    }
}

#[test]
fn view_carries_call_hotkey_unavailable_from_the_registrar() {
    // The field is a bool on every view (never omitted): false by default, true while
    // the registrar says the call slot is unavailable, in view() and in the Saved view
    // of a save that does not touch the call hotkey; false again once cleared.
    // Bite: view_with fills a constant false; the field read once at load; the Saved
    // view built from the pre-save state.
    let world = World::first_run();
    let service = fixture_service(&world);
    assert_eq!(unavailable_of(service.view()), json!(false));

    world.hotkeys.set_call_hotkey_unavailable(true);
    assert_eq!(unavailable_of(service.view()), json!(true), "view()");

    let mut d = defaults(None);
    d.api.base_url = "http://example.com/v1".into();
    let view = saved_view(service.save(req(d)));
    assert_eq!(
        unavailable_of(view),
        json!(true),
        "an unrelated save leaves the call slot as it was: the Saved view still says true"
    );
    assert!(
        !world
            .hotkeys
            .calls()
            .iter()
            .any(|c| matches!(c, RegistrarCall::PrepareCall(_))),
        "the save must not have re-registered the call slot"
    );

    world.hotkeys.set_call_hotkey_unavailable(false);
    assert_eq!(
        unavailable_of(service.view()),
        json!(false),
        "cleared view()"
    );
    let mut d = defaults(None);
    d.api.base_url = "http://example.com/v2".into();
    assert_eq!(
        unavailable_of(saved_view(service.save(req(d)))),
        json!(false)
    );
}

#[test]
fn saved_view_after_a_call_slot_commit_reads_the_cleared_state() {
    // A save that moves calls.hotkey to a free combination prepares and commits the
    // call slot; the fake clears the state on that commit, and the Saved view (built
    // after the commits) and the next view() say false. Bite: the Saved view reads the
    // flag before the commits; view_with caches it.
    let world = World::first_run();
    let service = fixture_service(&world);
    world.hotkeys.set_call_hotkey_unavailable(true);
    assert_eq!(unavailable_of(service.view()), json!(true));

    let mut d = defaults(None);
    d.calls.hotkey = "Ctrl+Alt+F10".into();
    let view = saved_view(service.save(req(d)));
    let moved = parse_hotkey("Ctrl+Alt+F10").expect("valid hotkey");
    assert!(
        world
            .hotkeys
            .calls()
            .iter()
            .any(|c| matches!(c, RegistrarCall::Commit(h) if *h == moved)),
        "the call slot was committed"
    );
    assert_eq!(
        unavailable_of(view),
        json!(false),
        "Saved view after the commit"
    );
    assert_eq!(unavailable_of(service.view()), json!(false), "next view()");
}

#[test]
fn a_dictation_slot_commit_does_not_clear_the_call_hotkey_mark() {
    // Only a call-slot commit clears the state; moving the dictation hotkey leaves the
    // call slot's failure standing. Bite: the fake (or the view) clearing on any commit.
    let world = World::first_run();
    let service = fixture_service(&world);
    world.hotkeys.set_call_hotkey_unavailable(true);

    let mut d = defaults(None);
    d.hotkey = "Ctrl+Alt+F9".into();
    let view = saved_view(service.save(req(d)));
    assert_eq!(unavailable_of(view), json!(true), "Saved view");
    assert_eq!(unavailable_of(service.view()), json!(true), "view()");
}

#[test]
fn a_refused_call_hotkey_save_keeps_the_mark_in_the_view() {
    // The failure branch of the save: the new call hotkey is taken too -> Refused, no
    // commit, the call slot's old failure stands, the next view() still says true.
    // Bite: a refused save clearing the state.
    let world = World::first_run();
    let service = fixture_service(&world);
    world.hotkeys.set_call_hotkey_unavailable(true);
    world.hotkeys.fail_prepare_call(true);

    let mut d = defaults(None);
    d.calls.hotkey = "Ctrl+Alt+F10".into();
    expect_refused(service.save(req(d)));
    assert_eq!(unavailable_of(service.view()), json!(true));
}
