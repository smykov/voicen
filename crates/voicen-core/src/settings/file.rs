//! The settings file (research R-2; spec 004 FR-009, FR-010;
//! contracts/core-traits.md#settingsfile). Only `SettingsService` uses it.

use std::io;
use std::path::PathBuf;

/// The settings file name inside the data directory (the one place it exists).
pub const SETTINGS_FILE: &str = "settings.json";

pub trait SettingsFile: Send + Sync {
    /// The file's bytes; `None` = no file.
    fn read(&self) -> io::Result<Option<Vec<u8>>>;
    /// Replace the file with `bytes`: tmp + sync + rename.
    fn write_atomic(&self, bytes: &[u8]) -> io::Result<()>;
    /// Move the file to `settings.json.bad-<suffix>` (or `-1` … `-9` when taken)
    /// without overwriting any existing file; returns the backup file name.
    fn move_aside(&self, suffix: &str) -> io::Result<String>;
}

/// [`SettingsFile`] over a directory with `std::fs`.
pub struct FsSettingsFile {
    dir: PathBuf,
}

impl FsSettingsFile {
    pub fn new(dir: PathBuf) -> FsSettingsFile {
        FsSettingsFile { dir }
    }
}

// T-032 skeleton (test-writer): every method fails with `Unsupported` until the
// developer implements R-2; the red tests in `settings::file::tests` and the
// service tests on a temp dir fail on this error.
fn not_implemented(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("T-032: FsSettingsFile::{what} not implemented"),
    )
}

impl SettingsFile for FsSettingsFile {
    fn read(&self) -> io::Result<Option<Vec<u8>>> {
        let _ = &self.dir;
        Err(not_implemented("read"))
    }

    fn write_atomic(&self, _bytes: &[u8]) -> io::Result<()> {
        Err(not_implemented("write_atomic"))
    }

    fn move_aside(&self, _suffix: &str) -> io::Result<String> {
        Err(not_implemented("move_aside"))
    }
}

/// One recorded call on [`FakeSettingsFile`].
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCall {
    Read,
    WriteAtomic,
    MoveAside(String),
}

#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
struct FakeFileState {
    bytes: Option<Vec<u8>>,
    backups: std::collections::BTreeMap<String, Vec<u8>>,
    calls: Vec<FileCall>,
    read_error: Option<io::ErrorKind>,
    write_error: Option<io::ErrorKind>,
    move_aside_error: Option<io::ErrorKind>,
}

/// In-memory [`SettingsFile`] with a call log and injectable failures.
///
/// - Every trait call is recorded in [`calls`](Self::calls), failed ones included.
/// - `fail_read` / `fail_write` / `fail_move_aside` make every later call of that
///   method fail with the given kind, without changing any bytes, until
///   [`clear_failures`](Self::clear_failures).
/// - `move_aside` uses the same names as the real file (`settings.json.bad-<suffix>`,
///   then `-1` … `-9`) and never replaces a backup.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeSettingsFile {
    state: std::sync::Mutex<FakeFileState>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeSettingsFile {
    /// No file.
    pub fn new() -> FakeSettingsFile {
        FakeSettingsFile::default()
    }

    /// A file holding `bytes` (not recorded as a call).
    pub fn with_bytes(bytes: &[u8]) -> FakeSettingsFile {
        let fake = FakeSettingsFile::default();
        fake.lock().bytes = Some(bytes.to_vec());
        fake
    }

    /// The file's bytes now (`None` = no file); not recorded.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        self.lock().bytes.clone()
    }

    /// Every backup made by `move_aside`: (file name, bytes), by name.
    pub fn backups(&self) -> Vec<(String, Vec<u8>)> {
        self.lock()
            .backups
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect()
    }

    pub fn calls(&self) -> Vec<FileCall> {
        self.lock().calls.clone()
    }

    pub fn fail_read(&self, kind: io::ErrorKind) {
        self.lock().read_error = Some(kind);
    }

    pub fn fail_write(&self, kind: io::ErrorKind) {
        self.lock().write_error = Some(kind);
    }

    pub fn fail_move_aside(&self, kind: io::ErrorKind) {
        self.lock().move_aside_error = Some(kind);
    }

    pub fn clear_failures(&self) {
        let mut state = self.lock();
        state.read_error = None;
        state.write_error = None;
        state.move_aside_error = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeFileState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-fakes"))]
fn injected(kind: io::ErrorKind) -> io::Error {
    io::Error::new(kind, "injected by FakeSettingsFile")
}

#[cfg(any(test, feature = "test-fakes"))]
impl SettingsFile for FakeSettingsFile {
    fn read(&self) -> io::Result<Option<Vec<u8>>> {
        let mut state = self.lock();
        state.calls.push(FileCall::Read);
        match state.read_error {
            Some(kind) => Err(injected(kind)),
            None => Ok(state.bytes.clone()),
        }
    }

    fn write_atomic(&self, bytes: &[u8]) -> io::Result<()> {
        let mut state = self.lock();
        state.calls.push(FileCall::WriteAtomic);
        if let Some(kind) = state.write_error {
            return Err(injected(kind));
        }
        state.bytes = Some(bytes.to_vec());
        Ok(())
    }

    fn move_aside(&self, suffix: &str) -> io::Result<String> {
        let mut state = self.lock();
        state.calls.push(FileCall::MoveAside(suffix.to_string()));
        if let Some(kind) = state.move_aside_error {
            return Err(injected(kind));
        }
        if state.bytes.is_none() {
            return Err(injected(io::ErrorKind::NotFound));
        }
        let base = format!("{SETTINGS_FILE}.bad-{suffix}");
        let name = std::iter::once(base.clone())
            .chain((1..=9).map(|n| format!("{base}-{n}")))
            .find(|name| !state.backups.contains_key(name))
            .ok_or_else(|| injected(io::ErrorKind::AlreadyExists))?;
        let bytes = state.bytes.take().unwrap_or_default();
        state.backups.insert(name.clone(), bytes);
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::path::Path;

    const TMP_FILE: &str = "settings.json.tmp";

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("list dir")
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

    #[test]
    fn write_atomic_replaces_and_leaves_no_tmp() {
        // Bite: write_atomic writing settings.json in place without a rename, a
        // .tmp left behind, the old content kept, or no create_dir_all.
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());

        file.write_atomic(b"{\"first\":1}")
            .expect("first write_atomic");
        assert_eq!(
            fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json"),
            b"{\"first\":1}"
        );
        file.write_atomic(b"{\"second\":2}")
            .expect("second write_atomic");
        assert_eq!(
            fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json"),
            b"{\"second\":2}"
        );
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);
        assert_eq!(
            file.read().expect("read").as_deref(),
            Some(&b"{\"second\":2}"[..])
        );

        // A data directory that does not exist yet is created.
        let nested = dir.path().join("nested").join("deeper");
        let file = FsSettingsFile::new(nested.clone());
        file.write_atomic(b"{}")
            .expect("write_atomic into a new dir");
        assert_eq!(fs::read(nested.join(SETTINGS_FILE)).expect("file"), b"{}");
        assert_eq!(entries(&nested), vec![SETTINGS_FILE.to_string()]);
    }

    #[test]
    fn read_without_a_file_is_none() {
        // Bite: NotFound reported as an error (that would make a first start
        // Unavailable instead of FirstRun).
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        assert_eq!(file.read().expect("no file is not an error"), None);
        // A data directory that does not exist yet is "no file" too.
        let file = FsSettingsFile::new(dir.path().join("missing"));
        assert_eq!(file.read().expect("no dir is not an error"), None);
    }

    #[test]
    fn leftover_tmp_is_deleted_and_never_read() {
        // Bite: reading the .tmp (as content or as a fallback), or leaving it.
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        fs::write(dir.path().join(SETTINGS_FILE), b"{\"in\":\"settings\"}").expect("seed");
        fs::write(dir.path().join(TMP_FILE), b"{\"in\":\"tmp\"}").expect("seed tmp");

        assert_eq!(
            file.read().expect("read").as_deref(),
            Some(&b"{\"in\":\"settings\"}"[..])
        );
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);

        // Only a .tmp (a crash before the first rename): no file, and the .tmp goes.
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        fs::write(dir.path().join(TMP_FILE), b"{\"in\":\"tmp\"}").expect("seed tmp");
        assert_eq!(file.read().expect("read"), None);
        assert!(entries(dir.path()).is_empty(), "{:?}", entries(dir.path()));
    }

    #[test]
    fn read_error_other_than_not_found_is_err() {
        // Bite: any read error mapped to None (a first run would then overwrite an
        // unreadable-but-present file with defaults).
        let dir = TempDir::new();
        fs::create_dir(dir.path().join(SETTINGS_FILE)).expect("dir at settings.json");
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        let err = file
            .read()
            .expect_err("a directory at settings.json is an error");
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
        // The OS error is passed through, not replaced by a made-up one.
        assert!(err.raw_os_error().is_some(), "not an OS error: {err}");
    }

    #[test]
    fn failed_write_keeps_previous_file() {
        // Bite: writing settings.json directly (the old bytes would be lost or
        // truncated when the write fails).
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        file.write_atomic(b"{\"old\":true}")
            .expect("first write_atomic");

        // The tmp path cannot be created as a file.
        fs::create_dir(dir.path().join(TMP_FILE)).expect("dir at settings.json.tmp");
        file.write_atomic(b"{\"new\":true}")
            .expect_err("write_atomic must fail when the tmp cannot be created");
        assert_eq!(
            fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json kept"),
            b"{\"old\":true}"
        );
    }

    #[test]
    fn move_aside_with_every_name_taken_fails_and_keeps_file() {
        // Bite: a plain rename into a taken name (overwrites an earlier backup), a
        // copy instead of a move, or more/fewer than the 10 candidate names.
        let dir = TempDir::new();
        let file = FsSettingsFile::new(dir.path().to_path_buf());
        let base = format!("{SETTINGS_FILE}.bad-S");
        let mut taken: Vec<String> = vec![base.clone()];
        taken.extend((1..=8).map(|n| format!("{base}-{n}")));
        for name in &taken {
            fs::write(dir.path().join(name), format!("marker {name}")).expect("seed backup");
        }
        fs::write(dir.path().join(SETTINGS_FILE), b"{bad 1").expect("seed");

        // The last free candidate is -9.
        let name = file.move_aside("S").expect("-9 is free");
        assert_eq!(name, format!("{base}-9"));
        assert_eq!(fs::read(dir.path().join(&name)).expect("backup"), b"{bad 1");
        assert!(
            !dir.path().join(SETTINGS_FILE).exists(),
            "moved, not copied"
        );

        // All 10 names taken: Err(AlreadyExists), nothing moved or overwritten.
        fs::write(dir.path().join(SETTINGS_FILE), b"{bad 2").expect("seed");
        let err = file.move_aside("S").expect_err("all names taken");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(dir.path().join(SETTINGS_FILE)).expect("settings.json kept"),
            b"{bad 2"
        );
        for name in &taken {
            assert_eq!(
                fs::read_to_string(dir.path().join(name)).expect("backup kept"),
                format!("marker {name}")
            );
        }
        assert_eq!(
            fs::read(dir.path().join(&name)).expect("-9 kept"),
            b"{bad 1"
        );
    }
}
