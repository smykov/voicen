//! T-022: the transcript history module `voicen_core::history` (spec 005,
//! contracts/core-history.md C1-C12), through its public API only. Red until the
//! module exists: the whole binary fails to compile on the missing `history`
//! module.
//!
//! Disk failures are injected with a fake store that fails exactly the n-th call of
//! one kind (once), so a later call cannot re-raise the same error and hide a
//! swallowed one at its origin. The real `FileHistoryStore` is checked on a
//! `TempDir`. Fake data only; the sentinels below stand for transcript text.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};
use voicen_core::history::{
    EngineKind, EngineLabel, FileHistoryStore, History, HistoryEntry, HistoryError,
    HistoryErrorKind, HistoryNotice, HistoryOp, HistoryStore, StoredHistory,
};
use voicen_core::test_support::TempDir;

const SENTINEL: &str = "SENTINEL-TEXT-7f3a91";

// ---- fake store -----------------------------------------------------------------

#[derive(Default)]
struct Inner {
    doc: Option<StoredHistory>,
    loads: usize,
    saves: usize,
    deletes: usize,
    /// 1-based call numbers (per kind) that fail, each once.
    fail_save: BTreeSet<usize>,
    fail_delete: BTreeSet<usize>,
    load_error: Option<HistoryErrorKind>,
}

/// A store the test keeps a handle to after `History` took ownership of it.
#[derive(Clone, Default)]
struct FakeStore(Arc<Mutex<Inner>>);

impl FakeStore {
    fn new() -> FakeStore {
        FakeStore::default()
    }
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn failing_save(self, calls: &[usize]) -> FakeStore {
        self.inner().fail_save.extend(calls.iter().copied());
        self
    }
    fn failing_delete(self, calls: &[usize]) -> FakeStore {
        self.inner().fail_delete.extend(calls.iter().copied());
        self
    }
    fn with_doc(self, doc: StoredHistory) -> FakeStore {
        self.inner().doc = Some(doc);
        self
    }
    fn with_load_error(self, kind: HistoryErrorKind) -> FakeStore {
        self.inner().load_error = Some(kind);
        self
    }
    fn doc(&self) -> Option<StoredHistory> {
        self.inner().doc.clone()
    }
    fn loads(&self) -> usize {
        self.inner().loads
    }
    fn saves(&self) -> usize {
        self.inner().saves
    }
    fn deletes(&self) -> usize {
        self.inner().deletes
    }
    /// The entries on "disk" (none when nothing is stored).
    fn stored_entries(&self) -> Vec<HistoryEntry> {
        self.doc().map(|d| d.entries).unwrap_or_default()
    }
}

impl HistoryStore for FakeStore {
    fn load(&mut self) -> Result<Option<StoredHistory>, HistoryError> {
        let mut s = self.inner();
        s.loads += 1;
        if let Some(kind) = s.load_error.clone() {
            return Err(HistoryError {
                op: HistoryOp::Load,
                kind,
            });
        }
        Ok(s.doc.clone())
    }
    fn save(&mut self, doc: &StoredHistory) -> Result<(), HistoryError> {
        let mut s = self.inner();
        s.saves += 1;
        if s.fail_save.contains(&s.saves) {
            return Err(HistoryError {
                op: HistoryOp::Save,
                kind: HistoryErrorKind::Io(ErrorKind::PermissionDenied),
            });
        }
        s.doc = Some(doc.clone());
        Ok(())
    }
    fn delete(&mut self) -> Result<(), HistoryError> {
        let mut s = self.inner();
        s.deletes += 1;
        if s.fail_delete.contains(&s.deletes) {
            return Err(HistoryError {
                op: HistoryOp::Delete,
                kind: HistoryErrorKind::Io(ErrorKind::PermissionDenied),
            });
        }
        s.doc = None;
        Ok(())
    }
}

// ---- helpers --------------------------------------------------------------------

fn label() -> EngineLabel {
    EngineLabel {
        kind: EngineKind::Api,
        model: "whisper-1".to_string(),
    }
}

fn entry(id: u64, text: &str) -> HistoryEntry {
    HistoryEntry {
        id,
        text: text.to_string(),
        ready_at_ms: 1_790_000_000_000 + id as i64,
        engine: label(),
    }
}

fn texts(h: &History<impl HistoryStore>) -> Vec<String> {
    h.view().entries.into_iter().map(|e| e.text).collect()
}

fn ids(h: &History<impl HistoryStore>) -> Vec<u64> {
    h.view().entries.into_iter().map(|e| e.id).collect()
}

/// Records `n` texts "t01".."tNN" with increasing times.
fn record_n(h: &mut History<impl HistoryStore>, n: usize) {
    for i in 1..=n {
        assert_eq!(
            h.record(format!("t{i:02}"), 1_000 + i as i64, label()),
            None,
            "record {i}"
        );
    }
}

/// C1's second half: what is stored is exactly what the view shows.
#[track_caller]
fn assert_synced(h: &History<impl HistoryStore>, store: &FakeStore) {
    assert_eq!(
        store.stored_entries(),
        h.view().entries,
        "the stored document must equal the view"
    );
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the temp dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// How many files under `dir` contain `needle` as bytes (names are not searched).
fn files_containing(dir: &Path, needle: &str) -> usize {
    std::fs::read_dir(dir)
        .expect("read the temp dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter(|e| {
            let bytes = std::fs::read(e.path()).expect("read file");
            bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
        })
        .count()
}

fn no_history_files(dir: &Path) {
    let leftovers: Vec<String> = file_names(dir)
        .into_iter()
        .filter(|n| n.starts_with("history.json"))
        .collect();
    assert_eq!(
        leftovers,
        Vec::<String>::new(),
        "history.json* must not exist"
    );
}

// ---- C1: bounded and in sync -------------------------------------------------------

#[test]
fn c1_25_records_with_size_20_keep_the_20_newest_first_and_the_stored_doc_equals_the_view() {
    let store = FakeStore::new();
    let (mut h, warning) = History::open(store.clone(), true, 20);
    assert_eq!(warning, None);
    record_n(&mut h, 25);

    let view = h.view();
    assert!(view.enabled);
    assert_eq!(view.notice, None);
    let want: Vec<String> = (6..=25).rev().map(|i| format!("t{i:02}")).collect();
    assert_eq!(texts(&h), want, "20 newest, newest first");
    assert_eq!(
        ids(&h),
        (6..=25).rev().collect::<Vec<u64>>(),
        "ids start at 1"
    );
    assert_synced(&h, &store);
    assert_eq!(store.doc().map(|d| d.next_id), Some(26));
}

#[test]
fn c1_after_any_sequence_of_operations_len_is_within_size_and_the_stored_doc_equals_the_view() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 5);
    let mut size = 5usize;
    let mut n = 0usize;
    // (record count, then an operation) steps; the invariant is checked after each.
    let steps: [(usize, Option<(bool, u8)>); 8] = [
        (12, None),
        (0, Some((true, 3))),
        (2, None),
        (0, Some((true, 100))),
        (30, None),
        (0, Some((true, 10))),
        (1, Some((false, 10))),
        (3, Some((true, 7))),
    ];
    for (records, apply) in steps {
        for _ in 0..records {
            n += 1;
            h.record(format!("op{n}"), n as i64, label());
            assert!(h.view().entries.len() <= size, "after record {n}");
            assert_synced(&h, &store);
        }
        if let Some((enabled, new_size)) = apply {
            assert_eq!(h.apply_settings(enabled, new_size), None);
            size = new_size as usize;
            assert!(
                h.view().entries.len() <= size,
                "after apply ({enabled}, {new_size})"
            );
            assert_synced(&h, &store);
        }
    }
    assert_eq!(h.clear(), None);
    assert_eq!(h.view().entries, vec![]);
    assert_synced(&h, &store);
}

#[test]
fn c1_lowering_the_size_trims_to_the_newest_and_saves() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 10);
    record_n(&mut h, 6);
    assert_eq!(h.apply_settings(true, 2), None);
    assert_eq!(texts(&h), vec!["t06".to_string(), "t05".to_string()]);
    assert_synced(&h, &store);
}

#[test]
fn open_trims_a_stored_document_above_the_size_and_rewrites_it() {
    // data-model: entries beyond `size` in a loaded file are trimmed and the file
    // rewritten.
    let doc = StoredHistory {
        next_id: 6,
        entries: (1..=5)
            .rev()
            .map(|i| entry(i, &format!("old{i}")))
            .collect(),
    };
    let store = FakeStore::new().with_doc(doc);
    let (h, warning) = History::open(store.clone(), true, 3);
    assert_eq!(warning, None);
    assert_eq!(
        texts(&h),
        vec!["old5".to_string(), "old4".to_string(), "old3".to_string()]
    );
    assert_synced(&h, &store);
    assert_eq!(store.doc().map(|d| d.next_id), Some(6), "next_id is kept");
}

// ---- C2/C5: off writes nothing ------------------------------------------------------

#[test]
fn c2_while_disabled_the_store_sees_no_save_at_all() {
    let store = FakeStore::new();
    let (mut h, warning) = History::open(store.clone(), false, 20);
    assert_eq!(warning, None);
    assert!(!h.view().enabled);
    assert_eq!(h.record(SENTINEL.to_string(), 1, label()), None);
    assert_eq!(h.clear(), None);
    assert_eq!(h.apply_settings(false, 5), None);
    assert_eq!(h.view().entries, vec![]);
    assert_eq!(store.saves(), 0, "no save while disabled");
    assert_eq!(store.doc(), None);
}

#[test]
fn c2_file_store_dir_has_no_history_json_while_disabled() {
    let dir = TempDir::new();
    let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), false, 20);
    h.record(SENTINEL.to_string(), 1, label());
    h.apply_settings(false, 20);
    no_history_files(dir.path());
}

#[test]
fn c5_open_disabled_deletes_leftovers_and_never_loads() {
    let doc = StoredHistory {
        next_id: 2,
        entries: vec![entry(1, SENTINEL)],
    };
    let store = FakeStore::new().with_doc(doc);
    let (h, warning) = History::open(store.clone(), false, 20);
    assert_eq!(warning, None);
    assert_eq!(store.loads(), 0, "open(enabled=false) never calls load");
    assert_eq!(store.deletes(), 1);
    assert_eq!(store.doc(), None);
    assert_eq!(h.view().entries, vec![]);
}

#[test]
fn c5_open_disabled_removes_history_json_and_the_interrupted_tmp_from_the_dir() {
    let dir = TempDir::new();
    {
        let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
        h.record(SENTINEL.to_string(), 1, label());
    }
    std::fs::write(
        dir.path().join("history.json.tmp"),
        format!("{{\"x\":\"{SENTINEL}\""),
    )
    .expect("tmp");
    assert_eq!(
        files_containing(dir.path(), SENTINEL),
        2,
        "premise: both files hold the text"
    );

    let (_h, warning) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), false, 20);
    assert_eq!(warning, None);
    assert_eq!(file_names(dir.path()), Vec::<String>::new());
}

#[test]
fn turning_on_from_off_starts_empty_and_writes_nothing_until_the_next_record() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), false, 20);
    assert_eq!(h.apply_settings(true, 20), None);
    assert!(h.view().enabled);
    assert_eq!(h.view().entries, vec![]);
    assert_eq!(store.saves(), 0, "nothing written on turning on");
    h.record("after on".to_string(), 1, label());
    assert_eq!(store.saves(), 1);
    assert_synced(&h, &store);
}

// ---- C3: off deletes, leaving no bytes ----------------------------------------------

#[test]
fn c3_apply_settings_off_deletes_both_files_and_no_byte_of_the_texts_remains() {
    let dir = TempDir::new();
    let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
    for i in 0..3 {
        h.record(format!("{SENTINEL}-{i}"), i, label());
    }
    std::fs::write(
        dir.path().join("history.json.tmp"),
        format!("partial {SENTINEL}-1"),
    )
    .expect("tmp");
    assert!(
        files_containing(dir.path(), SENTINEL) >= 1,
        "premise: the text is on disk"
    );

    assert_eq!(h.apply_settings(false, 20), None);
    no_history_files(dir.path());
    assert_eq!(files_containing(dir.path(), SENTINEL), 0);
    let view = h.view();
    assert!(!view.enabled);
    assert_eq!(view.entries, vec![]);
}

#[test]
fn c3_off_wins_over_a_size_change_in_the_same_call() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 10);
    record_n(&mut h, 4);
    assert_eq!(h.apply_settings(false, 2), None);
    assert_eq!(store.doc(), None, "deleted, not trimmed and saved");
    assert_eq!(h.view().entries, vec![]);
    assert!(!h.view().enabled);
}

#[test]
fn c3_delete_failure_on_off_keeps_the_notice_and_the_next_operation_retries() {
    // record x2 -> saves #1, #2; off -> delete #1 fails, fallback save #3 fails.
    let store = FakeStore::new().failing_delete(&[1]).failing_save(&[3]);
    let (mut h, _) = History::open(store.clone(), true, 10);
    record_n(&mut h, 2);
    let err = h.apply_settings(false, 10);
    assert!(
        matches!(
            &err,
            Some(HistoryError {
                op: HistoryOp::Delete,
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(h.view().entries, vec![], "memory is empty regardless");
    assert_eq!(h.view().notice, Some(HistoryNotice::DeleteFailed));
    assert_eq!(
        store.stored_entries().len(),
        2,
        "premise: nothing was removed yet"
    );
    // The next operation retries the pending deletion.
    assert_eq!(h.clear(), None);
    assert_eq!(store.stored_entries(), vec![]);
    assert_eq!(h.view().notice, None);
}

#[test]
fn c3_a_failed_deletion_on_off_is_retried_by_the_next_settings_apply_while_off() {
    // FR-009 (T-022 failure-branch bite, mutant 07): history is already off and its
    // deletion failed once; the next settings apply (still off, e.g. a size change
    // saved while off) is the next change and must retry the deletion, not skip it
    // because history "was already off".
    let store = FakeStore::new().failing_delete(&[1]);
    let (mut h, _) = History::open(store.clone(), true, 10);
    record_n(&mut h, 2);
    assert!(
        h.apply_settings(false, 10).is_some(),
        "premise: delete #1 fails"
    );
    assert_eq!(
        store.stored_entries().len(),
        2,
        "premise: text still stored"
    );

    assert_eq!(h.apply_settings(false, 5), None);
    assert_eq!(store.deletes(), 2, "the off apply retried the deletion");
    assert_eq!(store.doc(), None, "the stored text is gone");
    assert_eq!(h.view().notice, None);
    assert_eq!(store.saves(), 2, "nothing written while off");
}

// ---- C4: clear -----------------------------------------------------------------------

#[test]
fn c4_clear_empties_the_list_and_deletes_the_document() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 20);
    record_n(&mut h, 3);
    assert_eq!(h.clear(), None);
    assert_eq!(h.view().entries, vec![]);
    assert!(h.view().enabled, "clear does not switch history off");
    assert_eq!(store.doc(), None);
    assert_eq!(store.deletes(), 1);
}

#[test]
fn c4_clear_on_the_file_store_leaves_no_history_json_and_no_text_bytes() {
    let dir = TempDir::new();
    let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
    h.record(SENTINEL.to_string(), 1, label());
    assert_eq!(files_containing(dir.path(), SENTINEL), 1, "premise");
    assert_eq!(h.clear(), None);
    no_history_files(dir.path());
    assert_eq!(files_containing(dir.path(), SENTINEL), 0);
}

#[test]
fn c4_clear_works_while_disabled_and_is_idempotent() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), false, 20);
    assert_eq!(h.clear(), None);
    assert_eq!(h.clear(), None);
    assert_eq!(store.doc(), None);
}

#[test]
fn c4_delete_failure_falls_back_to_saving_an_empty_document() {
    let store = FakeStore::new().failing_delete(&[1]);
    let (mut h, _) = History::open(store.clone(), true, 20);
    h.record(SENTINEL.to_string(), 1, label());
    let saves_before = store.saves();
    assert_eq!(h.clear(), None, "the fallback worked, nothing to report");
    assert_eq!(h.view().entries, vec![]);
    assert_eq!(
        store.saves(),
        saves_before + 1,
        "an empty document was saved"
    );
    assert_eq!(
        store.stored_entries(),
        vec![],
        "the text is no longer stored"
    );
    assert_eq!(h.view().notice, None);
}

#[test]
fn c4_both_delete_and_fallback_failing_gives_delete_failed_and_the_next_op_retries() {
    // record -> save #1; clear -> delete #1 fails, fallback save #2 fails.
    let store = FakeStore::new().failing_delete(&[1]).failing_save(&[2]);
    let (mut h, _) = History::open(store.clone(), true, 20);
    h.record(SENTINEL.to_string(), 1, label());

    let err = h.clear();
    assert!(
        matches!(
            &err,
            Some(HistoryError {
                op: HistoryOp::Delete,
                ..
            })
        ),
        "{err:?}"
    );
    let view = h.view();
    assert_eq!(view.entries, vec![], "memory is cleared anyway");
    assert_eq!(view.notice, Some(HistoryNotice::DeleteFailed));

    // The next change retries the pending disk operation (FR-009) and clears the notice.
    assert_eq!(h.record("next".to_string(), 2, label()), None);
    assert_eq!(h.view().notice, None);
    assert_eq!(
        store
            .stored_entries()
            .into_iter()
            .map(|e| e.text)
            .collect::<Vec<_>>(),
        vec!["next".to_string()],
        "the old text is gone from the store"
    );
    assert!(store.deletes() >= 2, "the pending deletion was retried");
}

// ---- C6: corrupt or unreadable ----------------------------------------------------------

fn corrupt_documents() -> Vec<(&'static str, String)> {
    vec![
        ("invalid json", format!("not json {SENTINEL}{{")),
        (
            "unknown schema",
            json!({"schema": 2, "next_id": 2, "entries": [
                {"id": 1, "text": SENTINEL, "ready_at_ms": 1, "engine": {"kind": "api", "model": "m"}}
            ]})
            .to_string(),
        ),
        (
            "empty text",
            json!({"schema": 1, "next_id": 2, "entries": [
                {"id": 1, "text": "  \n", "ready_at_ms": 1, "engine": {"kind": "api", "model": "m"}}
            ]})
            .to_string(),
        ),
        (
            "ids not strictly decreasing",
            json!({"schema": 1, "next_id": 5, "entries": [
                {"id": 1, "text": SENTINEL, "ready_at_ms": 1, "engine": {"kind": "api", "model": "m"}},
                {"id": 2, "text": "b", "ready_at_ms": 1, "engine": {"kind": "api", "model": "m"}}
            ]})
            .to_string(),
        ),
        (
            "unknown engine kind",
            json!({"schema": 1, "next_id": 2, "entries": [
                {"id": 1, "text": SENTINEL, "ready_at_ms": 1, "engine": {"kind": "cloud", "model": "m"}}
            ]})
            .to_string(),
        ),
    ]
}

#[test]
fn c6_a_corrupt_file_gives_an_empty_history_a_corrupt_error_and_no_backup() {
    for (what, content) in corrupt_documents() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("history.json"), &content).expect("seed");
        let (mut h, warning) =
            History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
        assert_eq!(
            warning,
            Some(HistoryError {
                op: HistoryOp::Load,
                kind: HistoryErrorKind::Corrupt
            }),
            "{what}"
        );
        let view = h.view();
        assert!(view.enabled, "{what}");
        assert_eq!(view.entries, vec![], "{what}");
        for name in file_names(dir.path()) {
            assert_eq!(name, "history.json", "{what}: no backup or other file");
        }
        // The history keeps working and replaces the unreadable file.
        assert_eq!(h.record("fresh".to_string(), 1, label()), None, "{what}");
        let reloaded = FileHistoryStore::new(dir.path().to_path_buf())
            .load()
            .expect("a valid document now");
        assert_eq!(
            reloaded.map(|d| d.entries.into_iter().map(|e| e.text).collect::<Vec<_>>()),
            Some(vec!["fresh".to_string()]),
            "{what}"
        );
    }
}

#[test]
fn c6_an_unreadable_store_gives_an_empty_history_and_the_io_error() {
    let store = FakeStore::new().with_load_error(HistoryErrorKind::Io(ErrorKind::PermissionDenied));
    let (h, warning) = History::open(store, true, 20);
    assert_eq!(
        warning,
        Some(HistoryError {
            op: HistoryOp::Load,
            kind: HistoryErrorKind::Io(ErrorKind::PermissionDenied)
        })
    );
    assert_eq!(h.view().entries, vec![]);
    assert!(h.view().enabled);
}

// ---- C7: save failure ----------------------------------------------------------------------

#[test]
fn c7_a_save_failure_keeps_the_entry_sets_save_failed_and_record_returns() {
    let store = FakeStore::new().failing_save(&[2]);
    let (mut h, _) = History::open(store.clone(), true, 20);
    assert_eq!(h.record("one".to_string(), 1, label()), None);
    let err = h.record("two".to_string(), 2, label());
    assert!(
        matches!(
            &err,
            Some(HistoryError {
                op: HistoryOp::Save,
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(
        texts(&h),
        vec!["two".to_string(), "one".to_string()],
        "kept in memory"
    );
    assert_eq!(h.view().notice, Some(HistoryNotice::SaveFailed));
    assert_eq!(
        store.stored_entries().len(),
        1,
        "premise: the failed save wrote nothing"
    );

    // The next change writes the whole list again and clears the notice.
    assert_eq!(h.record("three".to_string(), 3, label()), None);
    assert_eq!(h.view().notice, None);
    assert_synced(&h, &store);
    assert_eq!(store.stored_entries().len(), 3);
}

// ---- C8: errors carry no text -----------------------------------------------------------------

#[test]
fn c8_display_and_debug_of_every_produced_error_hold_no_entry_text() {
    let mut errors: Vec<HistoryError> = Vec::new();

    // Corrupt: the file holds the sentinel.
    for (_, content) in corrupt_documents() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("history.json"), &content).expect("seed");
        let (_h, warning) =
            History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
        errors.extend(warning);
    }
    // Io: a store directory that cannot exist (its parent is a file).
    let dir = TempDir::new();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("blocker file");
    let (mut h, _) = History::open(FileHistoryStore::new(blocker.join("data")), true, 20);
    errors.extend(h.record(SENTINEL.to_string(), 1, label()));
    errors.extend(h.clear());
    errors.extend(h.apply_settings(false, 20));
    // Fake-store failures of every operation, with the sentinel in memory.
    let store = FakeStore::new().failing_save(&[1]).failing_delete(&[1]);
    let (mut h, _) = History::open(store, true, 20);
    errors.extend(h.record(SENTINEL.to_string(), 1, label()));
    errors.extend(h.clear());

    assert!(
        errors.len() >= 8,
        "premise: the scenarios produced errors, got {}",
        errors.len()
    );
    for e in &errors {
        for rendered in [format!("{e}"), format!("{e:?}")] {
            assert!(!rendered.contains(SENTINEL), "{rendered}");
            assert!(!rendered.contains("not json"), "{rendered}");
        }
    }
}

// ---- C9: blank text ------------------------------------------------------------------------------

#[test]
fn c9_empty_and_whitespace_text_is_never_recorded() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 20);
    for blank in ["", " ", "\n\t  \r\n", "\u{a0}\u{2003}"] {
        assert_eq!(h.record(blank.to_string(), 1, label()), None, "{blank:?}");
    }
    assert_eq!(h.view().entries, vec![]);
    assert_eq!(store.saves(), 0);
    assert_eq!(h.record(" x ".to_string(), 2, label()), None);
    assert_eq!(h.view().entries.len(), 1);
}

// ---- C10: ids and order ----------------------------------------------------------------------------

#[test]
fn c10_ids_strictly_increase_across_clear_and_off_on_and_order_ignores_the_clock() {
    let store = FakeStore::new();
    let (mut h, _) = History::open(store.clone(), true, 20);
    // Descending timestamps (the clock went back): insertion order still decides.
    h.record("a".to_string(), 900, label());
    h.record("b".to_string(), 500, label());
    h.record("c".to_string(), 100, label());
    assert_eq!(
        texts(&h),
        vec!["c".to_string(), "b".to_string(), "a".to_string()]
    );
    let before = ids(&h);
    assert!(before.windows(2).all(|w| w[0] > w[1]), "{before:?}");
    let last = before[0];

    assert_eq!(h.clear(), None);
    h.record("d".to_string(), 1, label());
    let after_clear = ids(&h)[0];
    assert!(after_clear > last, "{after_clear} after clear, was {last}");

    assert_eq!(h.apply_settings(false, 20), None);
    assert_eq!(h.apply_settings(true, 20), None);
    h.record("e".to_string(), 1, label());
    assert!(ids(&h)[0] > after_clear, "ids not reused across off/on");
}

#[test]
fn ids_continue_after_a_restart_from_the_stored_next_id() {
    let dir = TempDir::new();
    {
        let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
        record_n(&mut h, 3);
    }
    let (mut h, warning) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
    assert_eq!(warning, None);
    assert_eq!(ids(&h), vec![3, 2, 1]);
    h.record("t04".to_string(), 4, label());
    assert_eq!(ids(&h), vec![4, 3, 2, 1]);
}

// ---- text_of and the view ------------------------------------------------------------------------------

#[test]
fn text_of_returns_the_full_text_of_a_listed_id_and_none_for_any_other() {
    let (mut h, _) = History::open(
        FileHistoryStore::new(TempDir::new().path().to_path_buf()),
        true,
        2,
    );
    let long = format!("{SENTINEL} {}", "word ".repeat(2_000));
    h.record(long.clone(), 1, label());
    h.record("two".to_string(), 2, label());
    h.record("three".to_string(), 3, label()); // trims id 1
    let listed = ids(&h);
    assert_eq!(h.text_of(listed[0]), Some("three"));
    assert_eq!(h.text_of(listed[1]), Some("two"));
    assert_eq!(h.text_of(1), None, "trimmed out");
    assert_eq!(h.text_of(999), None);
    let (mut h2, _) = History::open(FakeStore::new(), true, 5);
    h2.record(long.clone(), 1, label());
    assert_eq!(
        h2.text_of(ids(&h2)[0]),
        Some(long.as_str()),
        "full text, not shortened"
    );
}

#[test]
fn view_serializes_to_the_ipc_shape() {
    let store = FakeStore::new().failing_save(&[1]);
    let (mut h, _) = History::open(store, true, 20);
    h.record(
        "hello".to_string(),
        42,
        EngineLabel {
            kind: EngineKind::Server,
            model: String::new(),
        },
    );
    let v: Value = serde_json::to_value(h.view()).expect("serialize the view");
    assert_eq!(
        v,
        json!({
            "enabled": true,
            "entries": [{"id": 1, "text": "hello", "ready_at_ms": 42,
                         "engine": {"kind": "server", "model": ""}}],
            "notice": "save_failed"
        })
    );
    let kinds: Vec<Value> = [EngineKind::Api, EngineKind::Local, EngineKind::Server]
        .iter()
        .map(|k| serde_json::to_value(k).expect("kind"))
        .collect();
    assert_eq!(kinds, vec![json!("api"), json!("local"), json!("server")]);
    assert_eq!(
        serde_json::to_value(HistoryNotice::DeleteFailed).expect("notice"),
        json!("delete_failed")
    );
    let ok = History::open(FakeStore::new(), true, 20).0.view();
    let v: Value = serde_json::to_value(ok).expect("serialize");
    assert_eq!(v.get("notice"), Some(&Value::Null));
}

// ---- FileHistoryStore (C2, C11) ---------------------------------------------------------------------------------

fn doc_of(texts: &[&str]) -> StoredHistory {
    let n = texts.len() as u64;
    StoredHistory {
        next_id: n + 1,
        entries: texts
            .iter()
            .enumerate()
            .map(|(i, t)| entry(n - i as u64, t))
            .collect(),
    }
}

#[test]
fn file_store_round_trips_and_writes_the_schema_1_document() {
    let dir = TempDir::new();
    let mut store = FileHistoryStore::new(dir.path().to_path_buf());
    assert_eq!(store.load(), Ok(None), "nothing stored");
    let doc = doc_of(&["newest", "oldest"]);
    store.save(&doc).expect("save");
    assert_eq!(store.load(), Ok(Some(doc)));
    assert_eq!(
        file_names(dir.path()),
        vec!["history.json".to_string()],
        "no tmp left behind"
    );

    let raw = std::fs::read(dir.path().join("history.json")).expect("file");
    let v: Value = serde_json::from_slice(&raw).expect("valid json");
    assert_eq!(v.get("schema"), Some(&json!(1)));
    assert_eq!(v.get("next_id"), Some(&json!(3)));
    assert_eq!(
        v.pointer("/entries/0"),
        Some(
            &json!({"id": 2, "text": "newest", "ready_at_ms": 1_790_000_000_002i64,
                     "engine": {"kind": "api", "model": "whisper-1"}})
        )
    );
}

#[test]
fn c11_an_interrupted_save_leaves_the_previous_document_loadable() {
    let dir = TempDir::new();
    let mut store = FileHistoryStore::new(dir.path().to_path_buf());
    let previous = doc_of(&["kept one", "kept zero"]);
    store.save(&previous).expect("save");
    // The state after a crash between writing the tmp file and the rename.
    std::fs::write(
        dir.path().join("history.json.tmp"),
        r#"{"schema":1,"next_id":9,"entries":[{"id":8,"text":"half writ"#,
    )
    .expect("tmp");
    let mut again = FileHistoryStore::new(dir.path().to_path_buf());
    assert_eq!(again.load(), Ok(Some(previous.clone())));
    // Open takes the same path and shows the previous entries.
    let (h, warning) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 20);
    assert_eq!(warning, None);
    assert_eq!(h.view().entries, previous.entries);
}

#[test]
fn file_store_delete_removes_both_files_and_is_ok_when_nothing_exists() {
    let dir = TempDir::new();
    let mut store = FileHistoryStore::new(dir.path().to_path_buf());
    assert_eq!(store.delete(), Ok(()), "nothing existed");
    store.save(&doc_of(&["x"])).expect("save");
    std::fs::write(dir.path().join("history.json.tmp"), b"partial").expect("tmp");
    assert_eq!(store.delete(), Ok(()));
    assert_eq!(file_names(dir.path()), Vec::<String>::new());
    assert_eq!(store.delete(), Ok(()), "twice");
}

#[test]
fn file_store_survives_a_restart_with_its_entries() {
    let dir = TempDir::new();
    {
        let (mut h, _) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 3);
        record_n(&mut h, 5);
    }
    let (h, warning) = History::open(FileHistoryStore::new(dir.path().to_path_buf()), true, 3);
    assert_eq!(warning, None);
    assert_eq!(
        texts(&h),
        vec!["t05".to_string(), "t04".to_string(), "t03".to_string()]
    );
}
