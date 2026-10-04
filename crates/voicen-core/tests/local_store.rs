//! T-016: `ModelStore` derives "downloaded" from the disk only (spec 002
//! data-model "LocalModelState", contracts/core-traits.md "ModelStore", T-016
//! analysis approach (7), decision #49).
//!
//! "Downloaded" = the final file exists with the catalog size; the hash is checked
//! only by the downloader. A wrong-size file is not downloaded and is never deleted
//! by the store; `*.part` files are deleted at start. No network here.

mod common;

use std::path::PathBuf;

use common::{catalog, dir_entries, entry, model_bytes, url_for, SIZE};
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::store::{LocalModelState, ModelStore};
use voicen_core::models::DownloadedModels;
use voicen_core::secrets::{KeyEdits, KeyPresence};
use voicen_core::settings::validate::{validate, KeyEditsWithPresence};
use voicen_core::settings::{defaults, EngineKind, ErrorCode, FieldId};
use voicen_core::test_support::TempDir;

const BASE: &str = "ggml-base.bin";
const TINY: &str = "ggml-tiny.bin";

/// `tiny` then `base`, both the fake model's size; the URLs are never used.
fn two_entries() -> Vec<CatalogEntry> {
    vec![
        entry(ModelId::Tiny, TINY, &url_for(9, TINY)),
        entry(ModelId::Base, BASE, &url_for(9, BASE)),
    ]
}

struct Dir {
    _tmp: TempDir,
    path: PathBuf,
}

fn models_dir() -> Dir {
    let tmp = TempDir::new();
    let path = tmp.path().join("models");
    std::fs::create_dir(&path).expect("create models dir");
    Dir { _tmp: tmp, path }
}

fn store(dir: &Dir, entries: Vec<CatalogEntry>) -> ModelStore {
    ModelStore::new(dir.path.clone(), catalog(entries))
}

fn write(dir: &Dir, name: &str, bytes: &[u8]) {
    std::fs::write(dir.path.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
}

fn validation_accepts_base(models: &dyn DownloadedModels) -> bool {
    let mut s = defaults(Some("en-US"));
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = Some("base".to_string());
    let edits = KeyEdits::default();
    let keys = KeyEditsWithPresence {
        edits: &edits,
        presence: KeyPresence::default(),
    };
    !validate(&s, &keys, models).iter().any(|e| {
        e.field == FieldId::EngineBuiltinLocalModelId && e.code == ErrorCode::ModelNotDownloaded
    })
}

#[test]
fn cleanup_at_start_deletes_every_part_file_and_nothing_else() {
    // Spec FR-008 / data-model "(start) … *.part deleted". Bite: only the catalog
    // models' `.part` deleted (a renamed catalog leaves orphans), a final file or an
    // unrelated file deleted, a wrong-size final file deleted.
    let dir = models_dir();
    write(&dir, "ggml-base.bin.part", &model_bytes()[..1000]);
    write(&dir, "ggml-old-model.bin.part", b"orphan");
    write(&dir, TINY, &model_bytes());
    write(&dir, BASE, b"wrong size");
    write(&dir, "notes.txt", b"keep");
    let s = store(&dir, two_entries());

    s.cleanup_at_start().expect("cleanup");

    assert_eq!(
        dir_entries(&dir.path),
        vec![BASE.to_string(), TINY.to_string(), "notes.txt".to_string()]
    );
    assert_eq!(
        std::fs::read(dir.path.join(BASE)).expect("base"),
        b"wrong size"
    );
    assert!(std::fs::read(dir.path.join(TINY)).expect("tiny") == model_bytes());
}

#[test]
fn cleanup_at_start_on_a_missing_dir_is_ok() {
    // First run: the models dir does not exist yet. Bite: an error (the shell would
    // report a failed start) or a panic on NotFound.
    let tmp = TempDir::new();
    let s = ModelStore::new(tmp.path().join("models"), catalog(two_entries()));
    assert!(s.cleanup_at_start().is_ok());
    assert!(!s.is_downloaded("base"));
    assert_eq!(s.list(), Vec::<String>::new());
}

#[test]
fn wrong_size_final_file_is_not_downloaded_and_stays_on_disk() {
    // data-model edge case. Bite: an existence-only check, a size check with
    // tolerance, the store deleting the file.
    for (label, len) in [
        ("one byte short", SIZE - 1),
        ("one byte long", SIZE + 1),
        ("empty", 0),
    ] {
        let dir = models_dir();
        let bytes = vec![7u8; len as usize];
        write(&dir, BASE, &bytes);
        let s = store(&dir, two_entries());
        s.cleanup_at_start().expect("cleanup");

        assert!(!s.is_downloaded("base"), "{label}: is_downloaded");
        assert_eq!(s.path_if_downloaded(ModelId::Base), None, "{label}: path");
        assert_eq!(s.list(), Vec::<String>::new(), "{label}: list");
        assert_eq!(
            s.states(),
            vec![
                (ModelId::Tiny, LocalModelState::NotDownloaded),
                (ModelId::Base, LocalModelState::NotDownloaded)
            ],
            "{label}: states"
        );
        assert!(
            !validation_accepts_base(&s),
            "{label}: validation accepted it"
        );
        assert!(
            std::fs::read(dir.path.join(BASE)).expect("file stays") == bytes,
            "{label}: the file changed"
        );
    }
}

#[test]
fn is_downloaded_follows_the_disk_without_caching() {
    // One store instance: file appears -> downloaded, removed -> not. The content is
    // not re-hashed: a catalog-size file is downloaded (the hash is the downloader's
    // gate). Bite: a state cached at construction or at the first call, a hash at
    // read time.
    let dir = models_dir();
    let s = store(&dir, two_entries());
    assert!(!s.is_downloaded("base"), "before");
    assert!(!validation_accepts_base(&s), "validation before");

    write(&dir, BASE, &vec![0u8; SIZE as usize]);
    assert!(s.is_downloaded("base"), "after the file appears");
    assert_eq!(
        s.path_if_downloaded(ModelId::Base),
        Some(dir.path.join(BASE))
    );
    assert!(
        validation_accepts_base(&s),
        "ModelStore as DownloadedModels: validation refuses builtin_local with a downloaded model"
    );

    std::fs::remove_file(dir.path.join(BASE)).expect("remove");
    assert!(!s.is_downloaded("base"), "after the file is removed");
    assert_eq!(s.path_if_downloaded(ModelId::Base), None);
}

#[test]
fn part_file_alone_is_not_downloaded() {
    // A complete-size `.part` (cut before the rename) is not the model. Bite: a
    // prefix or glob match on the file name.
    let dir = models_dir();
    write(&dir, "ggml-base.bin.part", &model_bytes());
    let s = store(&dir, two_entries());
    assert!(!s.is_downloaded("base"));
    assert_eq!(s.path_if_downloaded(ModelId::Base), None);
}

#[test]
fn unknown_or_uncataloged_id_is_not_downloaded() {
    // Bite: a panic on an unknown id, a case-insensitive or prefix match, a model
    // outside this store's catalog reported because its file happens to exist.
    let dir = models_dir();
    write(&dir, BASE, &model_bytes());
    write(&dir, "ggml-small.bin", &model_bytes());
    let s = store(&dir, vec![entry(ModelId::Base, BASE, &url_for(9, BASE))]);
    assert!(s.is_downloaded("base"));
    for id in [
        "",
        "nope",
        "Base",
        "BASE",
        "base ",
        "ggml-base.bin",
        "small",
        "tiny",
    ] {
        assert!(!s.is_downloaded(id), "is_downloaded({id:?})");
    }
    assert_eq!(s.list(), vec!["base".to_string()]);
}

#[test]
fn states_and_list_are_in_catalog_order() {
    // contracts/ipc.md: local_models_list returns catalog order; list() feeds the
    // settings picker. Bite: directory order, only downloaded models in states(),
    // ids not the stable strings.
    let dir = models_dir();
    let s = store(&dir, two_entries());
    write(&dir, BASE, &model_bytes());
    assert_eq!(
        s.states(),
        vec![
            (ModelId::Tiny, LocalModelState::NotDownloaded),
            (ModelId::Base, LocalModelState::Downloaded)
        ]
    );
    assert_eq!(s.list(), vec!["base".to_string()]);

    write(&dir, TINY, &model_bytes());
    assert_eq!(
        s.states(),
        vec![
            (ModelId::Tiny, LocalModelState::Downloaded),
            (ModelId::Base, LocalModelState::Downloaded)
        ]
    );
    assert_eq!(s.list(), vec!["tiny".to_string(), "base".to_string()]);
}
