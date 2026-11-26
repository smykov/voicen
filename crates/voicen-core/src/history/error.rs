//! The history's one error type (contracts/core-history.md C8; FR-011): the operation
//! and an `io::ErrorKind` or `Corrupt`, never a path, an OS message or entry text.

use std::fmt;
use std::io;

/// The store operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryOp {
    Load,
    Save,
    Delete,
}

impl HistoryOp {
    /// A closed literal per operation.
    pub fn as_str(self) -> &'static str {
        match self {
            HistoryOp::Load => "load",
            HistoryOp::Save => "save",
            HistoryOp::Delete => "delete",
        }
    }
}

/// Why it failed: the I/O error's kind only, or a document that is not a valid
/// schema-1 history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryErrorKind {
    Io(io::ErrorKind),
    Corrupt,
}

/// A failed store operation. Holds no text (no path, no OS message, no entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryError {
    pub op: HistoryOp,
    pub kind: HistoryErrorKind,
}

impl HistoryError {
    /// `op` failed with the kind of `err` (its message and path are dropped).
    pub fn io(op: HistoryOp, err: &io::Error) -> HistoryError {
        HistoryError {
            op,
            kind: HistoryErrorKind::Io(err.kind()),
        }
    }

    /// The loaded document is not a valid schema-1 history.
    pub fn corrupt() -> HistoryError {
        HistoryError {
            op: HistoryOp::Load,
            kind: HistoryErrorKind::Corrupt,
        }
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            HistoryErrorKind::Io(kind) => {
                write!(f, "history {} failed: {kind}", self.op.as_str())
            }
            HistoryErrorKind::Corrupt => {
                write!(f, "history {} failed: corrupt document", self.op.as_str())
            }
        }
    }
}

impl std::error::Error for HistoryError {}
