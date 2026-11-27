//! File-system helpers shared by call storage, the settings file and the log (T-251). Neutral:
//! it imports no other core module, so `settings` and `diag` use it without depending on
//! `call`.
//!
//! - [`fill_and_rename`] is core's one tmp+sync+rename tail: every whole-file replace
//!   (`meta.json`, the transcripts, an export, `settings.json`) ends in it.
//! - [`blocked_by_file`] is the one "a regular file is in the way" rule of a path that could
//!   not be opened or created.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// A writer whose written data can be made durable: `File::sync_data` in production, a
/// counting fake in tests.
pub trait SyncWrite: Write {
    fn sync_data(&mut self) -> io::Result<()>;
}

impl SyncWrite for std::fs::File {
    fn sync_data(&mut self) -> io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// A boxed writer syncs through what it holds (the session's track opener hands one out).
impl<W: SyncWrite + ?Sized> SyncWrite for Box<W> {
    fn sync_data(&mut self) -> io::Result<()> {
        (**self).sync_data()
    }
}

/// The one tmp+sync+rename tail: writes `bytes` to `file` (the open tmp at `tmp`, created by
/// the caller), syncs its data, closes it and renames `tmp` over `target`. On any failure
/// `tmp` is removed (best effort: the caller created it) and the first error is returned;
/// `target` is untouched. A tmp the caller could not open never reaches this function, so it
/// is left as found.
pub fn fill_and_rename<W: SyncWrite>(
    mut file: W,
    tmp: &Path,
    target: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    let synced = file.write_all(bytes).and_then(|()| file.sync_data());
    drop(file);
    let written = synced.and_then(|()| fs::rename(tmp, target));
    if written.is_err() {
        // Best effort: the tmp file is ours; the error returned is the write's own.
        let _ = fs::remove_file(tmp);
    }
    written
}

/// `true` when the nearest existing part of `path` (itself or an ancestor) is not a
/// directory: a regular file is in the way, wherever it sits on the path. Unix reports such a
/// path as "not a directory"; Windows as "path not found" (or "already exists" for a create),
/// so a caller maps its error through this to give the same answer on every OS. Reads
/// metadata only (one `metadata` per path part, nearest first); never creates anything.
pub fn blocked_by_file(path: &Path) -> bool {
    path.ancestors()
        .find_map(|part| fs::metadata(part).ok())
        .is_some_and(|meta| !meta.is_dir())
}
