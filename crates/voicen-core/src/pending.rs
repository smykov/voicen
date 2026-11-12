//! The pending recording's audio on disk (FR-032, T-209): `pending-<id>.wav` files
//! directly in one folder, written atomically, deleted only by their own name pattern
//! and never under a calls root (decision #251, invariant I3).

use crate::audio::{wav, AudioBuffer, SAMPLE_RATE};
use crate::platform::{PendingId, TempAudioStore};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;

/// The calls roots known now, the current one first, then the earlier ones; `None`
/// when they are unknown (settings `Unavailable`), which fails closed.
pub type CallRootsFn = Arc<dyn Fn() -> Option<Vec<PathBuf>> + Send + Sync>;

/// Writes `bytes` to the file at `path` (create or truncate). A test fails it midway.
pub type WriteFn = Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync>;

const PREFIX: &str = "pending-";
const WAV: &str = ".wav";
const PART: &str = ".wav.part";
/// The size of the header [`wav::encode`] writes.
const HEADER_LEN: usize = 44;

/// The pending-audio store over one folder (invariant I3).
///
/// - It writes, reads and deletes only regular files named `pending-<u64>.wav` or
///   `pending-<u64>.wav.part` directly in its folder: never recursively, never
///   through a link.
/// - Every write and delete first passes [`guard`]: the calls roots are known and
///   the folder neither equals, lies inside nor contains any of them. Otherwise
///   `put_pending` returns `Err` and the deletes delete nothing (and return `Err`).
/// - The roots closure is called at every write and delete, so a root changed
///   during the session counts at exit.
/// - `put_pending` writes `pending-<id>.wav.part`, then renames it onto
///   `pending-<id>.wav`; on any `Err` the `.part` is removed, so either the whole
///   file or none of the new one is left.
#[derive(Clone)]
pub struct FsTempAudioStore {
    dir: PathBuf,
    roots: CallRootsFn,
    writer: WriteFn,
}

impl FsTempAudioStore {
    /// A store over `dir` (created at the first put), guarded by `roots`.
    pub fn new(dir: PathBuf, roots: CallRootsFn) -> FsTempAudioStore {
        let writer: WriteFn = Arc::new(write_synced);
        FsTempAudioStore::with_writer(dir, roots, writer)
    }

    /// The same store with an injected writer (tests).
    pub fn with_writer(dir: PathBuf, roots: CallRootsFn, writer: WriteFn) -> FsTempAudioStore {
        FsTempAudioStore { dir, roots, writer }
    }

    fn file(&self, id: PendingId) -> PathBuf {
        self.dir.join(format!("{PREFIX}{}{WAV}", id.0))
    }

    fn part(&self, id: PendingId) -> PathBuf {
        self.dir.join(format!("{PREFIX}{}{PART}", id.0))
    }

    /// `Ok` only when [`guard`] passes for the roots read now.
    fn check(&self) -> io::Result<()> {
        if guard(&self.dir, (self.roots)().as_deref()) {
            Ok(())
        } else {
            Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "pending audio folder refused by the calls-roots guard",
            ))
        }
    }

    fn write_then_rename(&self, id: PendingId, bytes: &[u8]) -> io::Result<()> {
        let part = self.part(id);
        let result = (self.writer)(&part, bytes).and_then(|()| fs::rename(&part, self.file(id)));
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }
}

impl TempAudioStore for FsTempAudioStore {
    fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> io::Result<()> {
        self.check()?;
        fs::create_dir_all(&self.dir)?;
        self.write_then_rename(id, &wav::encode(audio))
    }

    fn get_pending(&self, id: PendingId) -> io::Result<AudioBuffer> {
        let bytes = fs::read(self.file(id))?;
        decode(&bytes)
            .map(AudioBuffer::from_16k_mono)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "not a pending audio file"))
    }

    fn delete_pending(&self, id: PendingId) -> io::Result<()> {
        self.check()?;
        remove_regular(&self.file(id))
    }

    fn delete_all(&self) -> io::Result<()> {
        self.check()?;
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut first_err = None;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            if !is_own_name(&entry.file_name()) {
                continue;
            }
            // The entry's own type, not followed: a link or a directory never counts.
            match entry.file_type() {
                Ok(t) if t.is_file() => {}
                Ok(_) => continue,
                Err(e) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        first_err.map_or(Ok(()), Err)
    }
}

/// The default writer: create or truncate, write, flush to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Removes `path` when it is a regular file (not followed); a missing file is `Ok`.
fn remove_regular(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => match fs::remove_file(path) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// `pending-<u64>.wav` or `pending-<u64>.wav.part`, the id ASCII digits only.
fn is_own_name(name: &OsStr) -> bool {
    let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(PREFIX)) else {
        return false;
    };
    let Some(id) = rest.strip_suffix(PART).or_else(|| rest.strip_suffix(WAV)) else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && id.parse::<u64>().is_ok()
}

/// The samples of a file [`wav::encode`] wrote (PCM 16-bit, mono, 16 kHz, the
/// canonical 44-byte header); `None` for anything else.
fn decode(bytes: &[u8]) -> Option<Vec<i16>> {
    let u16_at = |i: usize| -> Option<u16> {
        Some(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?))
    };
    let u32_at = |i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?))
    };
    let header_ok = bytes.get(0..4)? == b"RIFF"
        && bytes.get(8..16)? == b"WAVEfmt "
        && u32_at(16)? == 16
        && u16_at(20)? == 1
        && u16_at(22)? == 1
        && u32_at(24)? == SAMPLE_RATE
        && u16_at(34)? == 16
        && bytes.get(36..40)? == b"data";
    if !header_ok {
        return None;
    }
    let data_len = usize::try_from(u32_at(40)?).ok()?;
    let data = bytes.get(HEADER_LEN..HEADER_LEN.checked_add(data_len)?)?;
    let (pairs, odd) = data.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    Some(pairs.iter().map(|&c| i16::from_le_bytes(c)).collect())
}

/// The one calls-roots guard (decision #251): true only when `roots` is known and
/// `dir` neither equals, lies inside nor contains any root. Each path is compared
/// both canonicalised (its longest existing ancestor resolved, links and junctions
/// included) and lexically, component-wise and ASCII case-insensitively; any
/// relation in either form blocks, so the guard fails closed.
fn guard(dir: &Path, roots: Option<&[PathBuf]>) -> bool {
    let Some(roots) = roots else {
        return false;
    };
    let dir_forms = forms(dir);
    roots.iter().all(|root| {
        let root_forms = forms(root);
        !dir_forms
            .iter()
            .any(|d| root_forms.iter().any(|r| related(d, r)))
    })
}

/// The canonical form (when one can be made) and the lexical form of `path`.
fn forms(path: &Path) -> Vec<Vec<String>> {
    let mut out = vec![key(&lexical(path))];
    if let Some(c) = canonical(path) {
        out.push(key(&c));
    }
    out
}

/// `path` with its longest existing ancestor canonicalised and the missing rest
/// appended lexically; `None` when no ancestor can be canonicalised.
fn canonical(path: &Path) -> Option<PathBuf> {
    let path = lexical(path);
    let mut existing = path.as_path();
    let mut rest: Vec<&OsStr> = Vec::new();
    loop {
        if let Ok(c) = fs::canonicalize(existing) {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        rest.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// `path` with `.` dropped and `..` applied lexically.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The comparison key: one ASCII-lower-cased string per component, a verbatim
/// disk prefix (`\\?\C:`) folded into the plain one (`c:`).
fn key(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::VerbatimDisk(d) | Prefix::Disk(d) => {
                    format!("{}:", char::from(d).to_ascii_lowercase())
                }
                _ => p.as_os_str().to_string_lossy().to_ascii_lowercase(),
            },
            Component::RootDir => "/".to_string(),
            other => other.as_os_str().to_string_lossy().to_ascii_lowercase(),
        })
        .collect()
}

/// Equal, or one a component-wise prefix of the other.
fn related(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
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
