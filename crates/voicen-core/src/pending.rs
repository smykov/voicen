//! The pending recording's audio on disk (FR-032, T-209): `pending-<id>.wav` files
//! directly in one folder, written atomically, deleted only by their own name pattern
//! and never under a calls root (decision #251, invariant I3).

use crate::audio::AudioBuffer;
use crate::platform::{PendingId, TempAudioStore};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The calls roots known now, the current one first, then the earlier ones; `None`
/// when they are unknown (settings `Unavailable`), which fails closed.
pub type CallRootsFn = Arc<dyn Fn() -> Option<Vec<PathBuf>> + Send + Sync>;

/// Writes `bytes` to the file at `path` (create or truncate). A test fails it midway.
pub type WriteFn = Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync>;

/// STUB (T-209 red-tests stage): the compile surface the tests use. The developer
/// replaces every body; nothing here is behaviour.
#[derive(Clone)]
pub struct FsTempAudioStore {
    _dir: PathBuf,
    _roots: CallRootsFn,
    _writer: WriteFn,
}

impl FsTempAudioStore {
    pub fn new(dir: PathBuf, roots: CallRootsFn) -> FsTempAudioStore {
        let writer: WriteFn = Arc::new(|_, _| unimplemented!("T-209"));
        FsTempAudioStore::with_writer(dir, roots, writer)
    }

    /// The same store with an injected writer (tests).
    pub fn with_writer(dir: PathBuf, roots: CallRootsFn, writer: WriteFn) -> FsTempAudioStore {
        FsTempAudioStore {
            _dir: dir,
            _roots: roots,
            _writer: writer,
        }
    }
}

impl TempAudioStore for FsTempAudioStore {
    fn put_pending(&self, _id: PendingId, _audio: &AudioBuffer) -> io::Result<()> {
        unimplemented!("T-209")
    }

    fn get_pending(&self, _id: PendingId) -> io::Result<AudioBuffer> {
        unimplemented!("T-209")
    }

    fn delete_pending(&self, _id: PendingId) -> io::Result<()> {
        unimplemented!("T-209")
    }

    fn delete_all(&self) -> io::Result<()> {
        unimplemented!("T-209")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn audio(n: i16) -> AudioBuffer {
        AudioBuffer::from_16k_mono((0..64).map(|i| i * 7 - n).collect())
    }

    fn roots(list: Vec<PathBuf>) -> CallRootsFn {
        Arc::new(move || Some(list.clone()))
    }

    fn unknown_roots() -> CallRootsFn {
        Arc::new(|| None)
    }

    /// A store over `<base>/tmp/audio` with one unrelated calls root.
    struct World {
        base: TempDir,
    }

    impl World {
        fn new() -> World {
            World {
                base: TempDir::new(),
            }
        }

        fn folder(&self) -> PathBuf {
            self.base.path().join("tmp").join("audio")
        }

        fn calls(&self) -> PathBuf {
            self.base.path().join("calls")
        }

        fn store(&self) -> FsTempAudioStore {
            FsTempAudioStore::new(self.folder(), roots(vec![self.calls()]))
        }

        fn store_with(&self, list: Vec<PathBuf>) -> FsTempAudioStore {
            FsTempAudioStore::new(self.folder(), roots(list))
        }

        /// The folder with the file `pending-1.wav` in it.
        fn planted(&self) -> PathBuf {
            fs::create_dir_all(self.folder()).expect("folder");
            let f = self.folder().join("pending-1.wav");
            fs::write(&f, b"stale").expect("plant");
            f
        }
    }

    /// Every entry of `dir` (names), sorted; empty when the folder is missing.
    fn names(dir: &Path) -> Vec<String> {
        let Ok(rd) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut v: Vec<String> = rd
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Entries whose name looks like the store's own files, `.part` included.
    fn pattern_files(dir: &Path) -> Vec<String> {
        names(dir)
            .into_iter()
            .filter(|n| n.starts_with("pending-"))
            .collect()
    }

    // ---- put / get / delete_pending ------------------------------------------

    #[test]
    fn put_then_get_round_trips_the_samples() {
        // Bite: a header/sample misread, a put that writes nothing, or a file kept
        // under another name than pending-<id>.wav.
        let w = World::new();
        let store = w.store();
        let a = audio(3);
        store.put_pending(PendingId(7), &a).expect("put");
        assert_eq!(names(&w.folder()), vec!["pending-7.wav".to_string()]);
        let back = store.get_pending(PendingId(7)).expect("get");
        assert_eq!(back.samples(), a.samples());
    }

    #[test]
    fn get_of_an_unknown_id_is_an_error() {
        // Bite: an empty buffer returned for a missing file.
        let w = World::new();
        let store = w.store();
        store.put_pending(PendingId(1), &audio(1)).expect("put");
        assert!(store.get_pending(PendingId(2)).is_err());
    }

    #[test]
    fn delete_pending_removes_only_its_own_id() {
        // Bite: delete_pending that clears the folder.
        let w = World::new();
        let store = w.store();
        store.put_pending(PendingId(1), &audio(1)).expect("put 1");
        store.put_pending(PendingId(2), &audio(2)).expect("put 2");
        fs::write(w.folder().join("notes.txt"), b"keep").expect("notes");
        store.delete_pending(PendingId(1)).expect("delete");
        assert_eq!(
            names(&w.folder()),
            vec!["notes.txt".to_string(), "pending-2.wav".to_string()]
        );
    }

    // ---- put: whole file or none ---------------------------------------------

    #[test]
    fn a_write_that_fails_midway_leaves_no_file_of_the_pattern() {
        // Bite: no cleanup of the .part on a write error. The writer writes the first
        // half of the bytes and fails on its first call only (F-024): a later call
        // must work, so the Err cannot come from a second failure.
        let w = World::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let writer: WriteFn = Arc::new(move |path, bytes| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                fs::write(path, &bytes[..bytes.len() / 2])?;
                return Err(io::Error::other("disk full"));
            }
            fs::write(path, bytes)
        });
        let store = FsTempAudioStore::with_writer(w.folder(), roots(vec![w.calls()]), writer);

        let err = store.put_pending(PendingId(4), &audio(4));
        assert!(err.is_err(), "the midway failure must reach the caller");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the writer ran once");
        assert_eq!(pattern_files(&w.folder()), Vec::<String>::new());

        store
            .put_pending(PendingId(5), &audio(5))
            .expect("the next put works");
        assert_eq!(names(&w.folder()), vec!["pending-5.wav".to_string()]);
    }

    #[test]
    fn a_failed_put_leaves_the_previous_file_of_the_same_id_whole() {
        // Bite: writing straight to pending-<id>.wav (a half-written target).
        let w = World::new();
        let ok = Arc::new(Mutex::new(true));
        let gate = ok.clone();
        let writer: WriteFn = Arc::new(move |path, bytes| {
            if *gate.lock().expect("gate") {
                return fs::write(path, bytes);
            }
            fs::write(path, &bytes[..10])?;
            Err(io::Error::other("disk full"))
        });
        let store = FsTempAudioStore::with_writer(w.folder(), roots(vec![w.calls()]), writer);
        let first = audio(1);
        store.put_pending(PendingId(3), &first).expect("first put");
        *ok.lock().expect("gate") = false;
        assert!(store.put_pending(PendingId(3), &audio(2)).is_err());
        let back = store.get_pending(PendingId(3)).expect("old file whole");
        assert_eq!(back.samples(), first.samples());
        assert_eq!(names(&w.folder()), vec!["pending-3.wav".to_string()]);
    }

    #[test]
    fn a_target_name_that_is_a_directory_fails_the_rename_and_removes_the_part() {
        // Bite: the .part left behind when the rename fails (or the error swallowed).
        let w = World::new();
        fs::create_dir_all(w.folder().join("pending-5.wav")).expect("dir at the target");
        let store = w.store();
        assert!(store.put_pending(PendingId(5), &audio(5)).is_err());
        assert_eq!(names(&w.folder()), vec!["pending-5.wav".to_string()]);
        assert!(w.folder().join("pending-5.wav").is_dir());
    }

    #[test]
    fn a_parent_that_is_a_regular_file_fails_the_put_and_leaves_no_file() {
        // Bite: create_dir_all error swallowed, or a file written elsewhere.
        let w = World::new();
        let tmp = w.base.path().join("tmp");
        fs::write(&tmp, b"not a folder").expect("file at tmp");
        let store = w.store();
        assert!(store.put_pending(PendingId(1), &audio(1)).is_err());
        assert_eq!(fs::read(&tmp).expect("tmp"), b"not a folder");
        assert_eq!(names(w.base.path()), vec!["tmp".to_string()]);
    }

    // ---- delete_all: own pattern, not recursive ------------------------------

    #[test]
    fn delete_all_removes_only_regular_files_of_its_own_pattern() {
        // Bite: remove_dir_all, a wildcard on "pending", a missing .part, a recursion
        // into a subfolder, or a directory named like the pattern removed.
        let w = World::new();
        let dir = w.folder();
        fs::create_dir_all(dir.join("pending-3.wav")).expect("pattern-named dir");
        fs::write(dir.join("pending-3.wav").join("inner.txt"), b"x").expect("inner");
        fs::create_dir_all(dir.join("sub")).expect("sub");
        fs::write(dir.join("sub").join("pending-4.wav"), b"x").expect("nested");
        fs::write(dir.join("pending-1.wav"), b"x").expect("1");
        fs::write(dir.join("pending-2.wav.part"), b"x").expect("2.part");
        fs::write(dir.join("notes.txt"), b"x").expect("notes");
        fs::write(dir.join("pending-x.wav"), b"x").expect("x");
        fs::write(dir.join("pending-1.wav.bak"), b"x").expect("bak");
        fs::write(dir.join("pending-.wav"), b"x").expect("empty id");

        w.store().delete_all().expect("delete_all");

        assert_eq!(
            names(&dir),
            vec![
                "notes.txt".to_string(),
                "pending-.wav".to_string(),
                "pending-1.wav.bak".to_string(),
                "pending-3.wav".to_string(),
                "pending-x.wav".to_string(),
                "sub".to_string(),
            ]
        );
        assert!(dir.join("pending-3.wav").join("inner.txt").is_file());
        assert!(dir.join("sub").join("pending-4.wav").is_file());
    }

    #[test]
    fn delete_all_of_a_missing_folder_is_ok() {
        // Bite: NotFound reported as a failure (every first start would warn).
        let w = World::new();
        w.store().delete_all().expect("nothing to delete");
        assert!(
            !w.folder().exists(),
            "delete_all must not create the folder"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_all_does_not_follow_a_link_named_like_the_pattern() {
        // Bite: metadata() instead of the entry's own file type: the link's target
        // outside the folder deleted.
        let w = World::new();
        fs::create_dir_all(w.folder()).expect("folder");
        let outside = w.base.path().join("outside.txt");
        fs::write(&outside, b"keep").expect("outside");
        std::os::unix::fs::symlink(&outside, w.folder().join("pending-4.wav")).expect("link");
        w.store().delete_all().expect("delete_all");
        assert_eq!(fs::read(&outside).expect("target"), b"keep");
    }

    // ---- the calls-roots guard -----------------------------------------------

    /// With `list` as the roots: nothing is deleted, put is refused.
    fn assert_refused(w: &World, list: Vec<PathBuf>, why: &str) {
        let planted = w.planted();
        let store = w.store_with(list);
        let _ = store.delete_all();
        let _ = store.delete_pending(PendingId(1));
        assert!(planted.is_file(), "{why}: pending-1.wav was deleted");
        assert!(
            store.put_pending(PendingId(2), &audio(2)).is_err(),
            "{why}: put must be refused"
        );
        assert_eq!(
            pattern_files(&w.folder()),
            vec!["pending-1.wav".to_string()],
            "{why}: a file was written"
        );
    }

    #[test]
    fn a_root_equal_to_the_folder_blocks_the_cleanup() {
        // Bite: no guard at all, or a guard that only tests "folder inside root".
        let w = World::new();
        assert_refused(&w, vec![w.folder()], "root == folder");
    }

    #[test]
    fn a_root_that_is_an_ancestor_of_the_folder_blocks_the_cleanup() {
        // Bite: a guard that only tests "root inside folder" or equality.
        let w = World::new();
        assert_refused(&w, vec![w.base.path().to_path_buf()], "root above folder");
        let w = World::new();
        assert_refused(&w, vec![w.base.path().join("tmp")], "root = parent");
    }

    #[test]
    fn a_root_inside_the_folder_blocks_the_cleanup() {
        // Bite: a guard without the "contains" case (a non-recursive cleanup alone
        // passes this one, so it is the third test, not the only one: F-024).
        let w = World::new();
        let inside = w.folder().join("calls");
        fs::create_dir_all(&inside).expect("root inside");
        assert_refused(&w, vec![inside], "root inside folder");
    }

    #[test]
    fn an_earlier_root_is_guarded_like_the_current_one() {
        // Bite: only the first root checked.
        let w = World::new();
        let other = w.base.path().join("elsewhere");
        assert_refused(&w, vec![other.clone(), w.folder()], "earlier == folder");
        let w = World::new();
        let other = w.base.path().join("elsewhere");
        assert_refused(
            &w,
            vec![other, w.base.path().to_path_buf()],
            "earlier above folder",
        );
    }

    #[test]
    fn a_root_differing_only_in_ascii_case_blocks_the_cleanup() {
        // Bite: a case-sensitive lexical comparison (Windows paths differ in case).
        // The upper-cased root does not exist on a case-sensitive disk, so this
        // takes the lexical branch.
        let w = World::new();
        let up = PathBuf::from(w.folder().to_string_lossy().to_ascii_uppercase());
        assert_refused(&w, vec![up], "root differs in case");
        let w = World::new();
        let up = PathBuf::from(w.base.path().to_string_lossy().to_ascii_uppercase());
        assert_refused(&w, vec![up], "ancestor differs in case");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_the_folder_or_its_ancestor_as_a_root_blocks_the_cleanup() {
        // Bite: lexical-only comparison (the canonical branch dropped).
        let w = World::new();
        fs::create_dir_all(w.folder()).expect("folder");
        let link = w.base.path().join("calls-link");
        std::os::unix::fs::symlink(w.folder(), &link).expect("link to folder");
        assert_refused(&w, vec![link], "link to folder");

        let w = World::new();
        fs::create_dir_all(w.folder()).expect("folder");
        let link = w.base.path().join("calls-up");
        std::os::unix::fs::symlink(w.base.path(), &link).expect("link to ancestor");
        assert_refused(&w, vec![link], "link to ancestor");
    }

    #[test]
    fn unknown_roots_fail_closed() {
        // Bite: None treated as "no roots", so everything is allowed.
        let w = World::new();
        let planted = w.planted();
        let store = FsTempAudioStore::new(w.folder(), unknown_roots());
        let _ = store.delete_all();
        let _ = store.delete_pending(PendingId(1));
        assert!(planted.is_file(), "deleted while the roots are unknown");
        assert!(store.put_pending(PendingId(2), &audio(2)).is_err());
        assert_eq!(
            pattern_files(&w.folder()),
            vec!["pending-1.wav".to_string()]
        );
    }

    #[test]
    fn unrelated_roots_do_not_block_the_cleanup() {
        // Bite: a guard that refuses everything (sibling roots, a root whose name
        // only shares a string prefix with the folder, no roots at all).
        let w = World::new();
        let near = PathBuf::from(format!("{}-2", w.folder().display()));
        for list in [
            vec![w.calls()],
            vec![w.calls(), near.clone()],
            vec![near],
            Vec::new(),
        ] {
            let planted = w.planted();
            let store = w.store_with(list.clone());
            store.put_pending(PendingId(9), &audio(9)).expect("put");
            store.delete_pending(PendingId(9)).expect("delete 9");
            assert!(planted.is_file());
            store.delete_all().expect("delete_all");
            assert!(!planted.exists(), "roots {list:?} blocked the cleanup");
        }
    }

    #[test]
    fn the_roots_are_read_at_every_call() {
        // Bite: roots captured at construction: a root changed during the session
        // must count at exit.
        let w = World::new();
        let planted = w.planted();
        let current = Arc::new(Mutex::new(vec![w.calls()]));
        let src = current.clone();
        let roots: CallRootsFn = Arc::new(move || Some(src.lock().expect("roots").clone()));
        let store = FsTempAudioStore::new(w.folder(), roots);
        *current.lock().expect("roots") = vec![w.base.path().to_path_buf()];
        let _ = store.delete_all();
        assert!(planted.is_file(), "the changed root was not honoured");
        *current.lock().expect("roots") = vec![w.calls()];
        store.delete_all().expect("delete_all");
        assert!(!planted.exists());
    }
}
