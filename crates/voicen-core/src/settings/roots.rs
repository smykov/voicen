//! The call storage root (spec 007 FR-002, FR-039; research R19; decisions #196,
//! #198): the one write probe of a root, as a port the settings core owns.
//!
//! [`CallRoots::probe`] is called by `SettingsService::save` when the effective root
//! changes, and (T-125/T-126) at every Record start; there is no second probe. A
//! refusal is a closed [`UnwritableReason`], never a path or OS text.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The file the probe creates and deletes in a root (research R19).
pub const WRITE_PROBE_FILE: &str = ".voicen-write-probe";

/// Why a storage root cannot be written; on the wire as the snake_case word
/// (`FieldError.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnwritableReason {
    /// The folder does not exist (a chosen root is never created).
    Missing,
    /// The path, or a part of it, is not a folder.
    NotAFolder,
    /// The path is not absolute.
    Relative,
    AccessDenied,
    DiskFull,
    /// Any other io error.
    Other,
}

/// The storage-root port: the default root and the one write probe.
pub trait CallRoots: Send + Sync {
    /// The root in force when `calls.storage_root` is `None` (`<data_dir>/calls`).
    fn default_root(&self) -> PathBuf;

    /// `Ok` when `root` is an absolute, existing folder in which
    /// `.voicen-write-probe` can be created and deleted.
    fn probe(&self, root: &Path) -> Result<(), UnwritableReason>;
}

/// [`CallRoots`] on `std::fs`.
///
/// Probe order: a relative path is `relative` (the disk is not touched); the default
/// root is created (`create_dir_all`, the app's own folder); any other root must
/// exist (`missing`) and be a folder (`not_a_folder`) and is never created; then the
/// probe file is created or truncated (not `create_new`, so a probe left by a crash
/// does not block) and removed. Each io error goes through [`reason_of`].
#[derive(Debug, Clone)]
pub struct FsCallRoots {
    default_root: PathBuf,
}

impl FsCallRoots {
    pub fn new(default_root: PathBuf) -> FsCallRoots {
        FsCallRoots { default_root }
    }
}

impl CallRoots for FsCallRoots {
    fn default_root(&self) -> PathBuf {
        self.default_root.clone()
    }

    fn probe(&self, root: &Path) -> Result<(), UnwritableReason> {
        if !root.is_absolute() {
            return Err(UnwritableReason::Relative);
        }
        if root == self.default_root {
            fs::create_dir_all(root).map_err(|e| reason_of(e.kind()))?;
        }
        let meta = fs::metadata(root).map_err(|e| reason_of(e.kind()))?;
        if !meta.is_dir() {
            return Err(UnwritableReason::NotAFolder);
        }
        let probe = root.join(WRITE_PROBE_FILE);
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&probe)
            .map_err(|e| reason_of(e.kind()))?;
        fs::remove_file(&probe).map_err(|e| reason_of(e.kind()))
    }
}

/// A [`CallRoots`] for tests that do not exercise the root: every probe passes and
/// the disk is never touched. The default root is a fixed absolute fake path.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone)]
pub struct FakeCallRoots {
    default_root: PathBuf,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeCallRoots {
    pub fn new() -> FakeCallRoots {
        let root = if cfg!(windows) {
            r"C:\voicen-fake-data\calls"
        } else {
            "/voicen-fake-data/calls"
        };
        FakeCallRoots {
            default_root: PathBuf::from(root),
        }
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl Default for FakeCallRoots {
    fn default() -> FakeCallRoots {
        FakeCallRoots::new()
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl CallRoots for FakeCallRoots {
    fn default_root(&self) -> PathBuf {
        self.default_root.clone()
    }

    fn probe(&self, _root: &Path) -> Result<(), UnwritableReason> {
        Ok(())
    }
}

/// The one mapping of a probe's io error to its reason (as
/// `diag::log::LogsUnwritableReason`).
pub fn reason_of(kind: io::ErrorKind) -> UnwritableReason {
    match kind {
        io::ErrorKind::NotFound => UnwritableReason::Missing,
        io::ErrorKind::PermissionDenied => UnwritableReason::AccessDenied,
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => UnwritableReason::DiskFull,
        io::ErrorKind::NotADirectory => UnwritableReason::NotAFolder,
        _ => UnwritableReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const PROBE: &str = ".voicen-write-probe";

    fn roots(dir: &TempDir) -> (FsCallRoots, PathBuf) {
        let default_root = dir.path().join("data").join("calls");
        (FsCallRoots::new(default_root.clone()), default_root)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(dir)
            .expect("list dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn fs_call_roots_is_a_shareable_port() {
        // The service holds it as `Arc<dyn CallRoots>` and T-125/T-126 call it from
        // the capture side. Bite: a trait without `Send + Sync`.
        fn shareable<T: ?Sized + Send + Sync>() {}
        shareable::<dyn CallRoots>();
        let dir = TempDir::new();
        let (fs_roots, default_root) = roots(&dir);
        let port: Arc<dyn CallRoots> = Arc::new(fs_roots);
        assert_eq!(port.default_root(), default_root);
    }

    #[test]
    fn probe_accepts_a_writable_folder_and_leaves_nothing_behind() {
        // Bite: a probe that leaves `.voicen-write-probe` (the delete step skipped),
        // or one that writes elsewhere.
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("chosen");
        fs::create_dir(&chosen).expect("create");
        assert_eq!(fs_roots.probe(&chosen), Ok(()));
        assert_eq!(names(&chosen), Vec::<String>::new());
        // Twice: a second probe is as good as the first.
        assert_eq!(fs_roots.probe(&chosen), Ok(()));
        assert_eq!(names(&chosen), Vec::<String>::new());
    }

    #[test]
    fn probe_keeps_the_content_of_the_folder() {
        // The probe never touches what the user keeps there (earlier calls).
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("chosen");
        fs::create_dir_all(chosen.join("call-1")).expect("create");
        fs::write(chosen.join("call-1").join("meta.json"), b"{}").expect("write");
        assert_eq!(fs_roots.probe(&chosen), Ok(()));
        assert_eq!(names(&chosen), vec!["call-1".to_string()]);
        assert_eq!(names(&chosen.join("call-1")), vec!["meta.json".to_string()]);
    }

    #[test]
    fn probe_refuses_a_missing_chosen_root_and_never_creates_it() {
        // FR-002: a missing root refuses Record with `missing`. Bite: `create_dir_all`
        // on every root (the missing folder is created and accepted).
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("not-there").join("calls");
        assert_eq!(fs_roots.probe(&chosen), Err(UnwritableReason::Missing));
        assert!(!dir.path().join("not-there").exists(), "created a parent");
        assert!(!chosen.exists(), "created the root");
    }

    #[test]
    fn probe_refuses_a_root_that_is_a_regular_file() {
        // Bite: only the probe-file step (it fails with a different reason), or a
        // metadata check that accepts any existing path.
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let file = dir.path().join("a-file");
        fs::write(&file, b"x").expect("write");
        assert_eq!(fs_roots.probe(&file), Err(UnwritableReason::NotAFolder));
        assert_eq!(
            fs::read(&file).expect("read"),
            b"x",
            "the file is untouched"
        );
    }

    #[test]
    fn probe_refuses_a_root_under_a_regular_file() {
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let file = dir.path().join("a-file");
        fs::write(&file, b"x").expect("write");
        let under = file.join("calls");
        let got = fs_roots.probe(&under);
        // Unix says "not a directory" for a path below a file; Windows says "path not
        // found". Either way the root is refused, never created.
        #[cfg(unix)]
        assert_eq!(got, Err(UnwritableReason::NotAFolder));
        #[cfg(not(unix))]
        assert!(
            matches!(
                got,
                Err(UnwritableReason::NotAFolder | UnwritableReason::Missing)
            ),
            "{got:?}"
        );
        assert!(file.is_file(), "the file is still a file");
    }

    #[test]
    fn probe_refuses_a_relative_root_without_touching_the_disk() {
        // A relative path would resolve against the working directory (R19 needs a
        // fixed folder). Bite: the relative check missing (the folder exists below
        // the cwd, or is created, and the probe passes).
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        for relative in [
            "voicen-t120-relative-root",
            "voicen-t120-a/voicen-t120-b",
            ".",
        ] {
            assert_eq!(
                fs_roots.probe(Path::new(relative)),
                Err(UnwritableReason::Relative),
                "{relative}"
            );
        }
        assert!(!Path::new("voicen-t120-relative-root").exists());
        assert!(!Path::new("voicen-t120-a").exists());
        assert!(!Path::new(PROBE).exists(), "a probe file in the cwd");
    }

    #[test]
    fn probe_creates_the_default_root_and_only_the_default_root() {
        // The default root is the app's own folder: the first Record on a fresh
        // install must not be refused as `missing`. Bite: no `create_dir_all` for the
        // default (the first call is refused), or one for every root.
        let dir = TempDir::new();
        let (fs_roots, default_root) = roots(&dir);
        assert!(!default_root.exists());
        assert_eq!(fs_roots.probe(&default_root), Ok(()));
        assert!(default_root.is_dir(), "the default root was not created");
        assert_eq!(names(&default_root), Vec::<String>::new());
        // A sibling is not the default: not created.
        let sibling = dir.path().join("data").join("calls-other");
        assert_eq!(fs_roots.probe(&sibling), Err(UnwritableReason::Missing));
        assert!(!sibling.exists());
    }

    #[test]
    fn probe_fails_when_the_probe_file_cannot_be_created() {
        // The create branch of Acceptance 3 on a real disk (permission tricks do not
        // fail under root in the Docker gate): `.voicen-write-probe` is a folder, so
        // creating the file fails. Bite: a probe that only checks `is_dir` and
        // read-only flags, or one that swallows the create error.
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("chosen");
        fs::create_dir_all(chosen.join(PROBE)).expect("create");
        let got = fs_roots.probe(&chosen);
        assert!(got.is_err(), "{got:?}");
        assert!(chosen.join(PROBE).is_dir(), "the probe path was replaced");
    }

    #[cfg(unix)]
    #[test]
    fn a_create_error_is_refused_even_when_the_delete_would_succeed() {
        // `.voicen-write-probe` is a symlink to itself: opening it fails (ELOOP) while
        // unlinking a symlink succeeds, so only the create error can refuse. (The
        // folder case above fails both steps and cannot tell them apart.) Bite: the
        // create error swallowed (`.ok()`), leaving the delete as the only check.
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("chosen");
        fs::create_dir(&chosen).expect("create");
        std::os::unix::fs::symlink(PROBE, chosen.join(PROBE)).expect("symlink");
        let got = fs_roots.probe(&chosen);
        assert!(got.is_err(), "{got:?}");
    }

    #[test]
    fn a_stale_probe_file_does_not_block_the_probe() {
        // A crash between create and delete leaves the file behind; it must not turn
        // every later save into a refusal. Bite: `create_new(true)`.
        let dir = TempDir::new();
        let (fs_roots, _) = roots(&dir);
        let chosen = dir.path().join("chosen");
        fs::create_dir(&chosen).expect("create");
        fs::write(chosen.join(PROBE), b"left by a crash").expect("write");
        assert_eq!(fs_roots.probe(&chosen), Ok(()));
        assert_eq!(names(&chosen), Vec::<String>::new(), "the stale file stays");
    }

    #[test]
    fn reason_of_maps_the_io_kind_to_the_closed_reason() {
        // One mapping for every io error of the probe (as `LogsUnwritableReason`).
        // Bite: PermissionDenied or StorageFull sent to `other`, NotFound not `missing`,
        // the table's default not `other`.
        use io::ErrorKind as K;
        let table = [
            (K::NotFound, UnwritableReason::Missing),
            (K::PermissionDenied, UnwritableReason::AccessDenied),
            (K::StorageFull, UnwritableReason::DiskFull),
            (K::QuotaExceeded, UnwritableReason::DiskFull),
            (K::NotADirectory, UnwritableReason::NotAFolder),
            (K::IsADirectory, UnwritableReason::Other),
            (K::ReadOnlyFilesystem, UnwritableReason::Other),
            (K::AlreadyExists, UnwritableReason::Other),
            (K::TimedOut, UnwritableReason::Other),
            (K::Interrupted, UnwritableReason::Other),
            (K::Other, UnwritableReason::Other),
        ];
        for (kind, reason) in table {
            assert_eq!(reason_of(kind), reason, "{kind:?}");
        }
    }

    #[test]
    fn unwritable_reason_wire_form_is_snake_case_and_closed() {
        // The `reason` of the wire (decision #196 (2)); G41's texts are keyed by it.
        // Bite: a variant renamed, camel case, a payload (path or OS text).
        let table = [
            (UnwritableReason::Missing, "missing"),
            (UnwritableReason::NotAFolder, "not_a_folder"),
            (UnwritableReason::Relative, "relative"),
            (UnwritableReason::AccessDenied, "access_denied"),
            (UnwritableReason::DiskFull, "disk_full"),
            (UnwritableReason::Other, "other"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (reason, wire) in table {
            assert_eq!(
                serde_json::to_value(reason).expect("serializes"),
                serde_json::json!(wire)
            );
            assert!(seen.insert(reason), "{reason:?} listed twice");
        }
        // The Debug text of a reason names no path (it is logged by the diag layer).
        assert!(!format!("{:?}", UnwritableReason::Missing).contains('/'));
    }
}
