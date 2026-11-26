//! The disk seam of the history (contracts/core-history.md "Store trait") and its one
//! file implementation: `history.json` written whole through `history.json.tmp` and a
//! rename (research R3; C11).

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::{HistoryError, HistoryOp};
use super::model::{HistoryEntry, StoredHistory};

/// The stored document's file name in the data directory.
pub const HISTORY_FILE: &str = "history.json";

/// The partial write's file name, renamed over [`HISTORY_FILE`] when complete.
pub const HISTORY_TMP_FILE: &str = "history.json.tmp";

/// The one schema this build reads and writes.
const SCHEMA: u32 = 1;

/// Where the history document lives. Errors never carry text.
pub trait HistoryStore: Send {
    /// `Ok(None)` when nothing is stored. Corrupt data => `Err(kind = Corrupt)`.
    fn load(&mut self) -> Result<Option<StoredHistory>, HistoryError>;
    /// Atomic replace: after `Ok`, exactly `doc` is stored; after `Err`, the previous
    /// document is intact.
    fn save(&mut self, doc: &StoredHistory) -> Result<(), HistoryError>;
    /// Removes the document and any interrupted partial write. `Ok` if nothing existed.
    fn delete(&mut self) -> Result<(), HistoryError>;
}

/// `history.json` (+ `history.json.tmp`) in `dir`; `dir` comes from the shell's single
/// data-directory resolver (`paths::data_dir()`).
#[derive(Debug, Clone)]
pub struct FileHistoryStore {
    dir: PathBuf,
}

impl FileHistoryStore {
    pub fn new(dir: PathBuf) -> Self {
        FileHistoryStore { dir }
    }
}

/// The document on disk (data-model "Stored document").
#[derive(Serialize)]
struct DocOut<'a> {
    schema: u32,
    next_id: u64,
    entries: &'a [HistoryEntry],
}

#[derive(Deserialize)]
struct DocIn {
    schema: u32,
    next_id: u64,
    entries: Vec<HistoryEntry>,
}

/// The data-model rules of a loaded document: schema 1, every text non-blank, ids
/// strictly decreasing and below `next_id`. Any violation makes the whole document
/// corrupt (FR-010).
fn valid(doc: DocIn) -> Option<StoredHistory> {
    if doc.schema != SCHEMA {
        return None;
    }
    let mut above = doc.next_id;
    for entry in &doc.entries {
        if entry.text.trim().is_empty() || entry.id >= above {
            return None;
        }
        above = entry.id;
    }
    Some(StoredHistory {
        next_id: doc.next_id,
        entries: doc.entries,
    })
}

/// Removes `path`; a missing file is success.
fn remove_if_present(path: &std::path::Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

impl HistoryStore for FileHistoryStore {
    fn load(&mut self) -> Result<Option<StoredHistory>, HistoryError> {
        let bytes = match fs::read(self.dir.join(HISTORY_FILE)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(HistoryError::io(HistoryOp::Load, &err)),
        };
        // serde_json's error quotes no input, but it is dropped anyway (C8).
        let doc: DocIn = serde_json::from_slice(&bytes).map_err(|_| HistoryError::corrupt())?;
        valid(doc).map(Some).ok_or_else(HistoryError::corrupt)
    }

    fn save(&mut self, doc: &StoredHistory) -> Result<(), HistoryError> {
        let failed = |err: io::Error| HistoryError::io(HistoryOp::Save, &err);
        let bytes = serde_json::to_vec(&DocOut {
            schema: SCHEMA,
            next_id: doc.next_id,
            entries: &doc.entries,
        })
        .map_err(|err| failed(io::Error::from(err)))?;
        fs::create_dir_all(&self.dir).map_err(failed)?;
        // The one tmp + sync + rename writer of the crate: a stale tmp is replaced,
        // a failed write removes its tmp and leaves `history.json` as it was.
        crate::call::storage::write_atomic(&self.dir, HISTORY_FILE, &bytes).map_err(failed)
    }

    fn delete(&mut self) -> Result<(), HistoryError> {
        let failed = |err: io::Error| HistoryError::io(HistoryOp::Delete, &err);
        // Both are attempted; the document's own error comes first.
        let doc = remove_if_present(&self.dir.join(HISTORY_FILE));
        let tmp = remove_if_present(&self.dir.join(HISTORY_TMP_FILE));
        doc.and(tmp).map_err(failed)
    }
}
