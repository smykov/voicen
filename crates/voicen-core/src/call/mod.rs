//! Call mode (spec 007, contracts/core-call.md "Modules").

pub mod codec;
pub mod events;
pub mod transcript;

pub use events::{
    CallId, CallNotice, CallSource, CallState, CallWarning, DeviceCause, EndpointId, FailReason,
    FinalOutcome, FinalStage, InvalidRule, PassMode, RefusedPart, RefusedReason, SilentRule,
    StopReason, StorageKind, SwitchRefusal, Track, VadFallbackCause,
};

pub use transcript::{
    merge, render_markdown, ChunkInput, EngineRef, EngineSegment, EnginesV1, MergeInput, SegmentV1,
    Speaker, TrackInput, TranscriptParseError, TranscriptV1,
};

use std::fmt;
use std::io;

/// A call track could not be stored (FR-016): the operation that failed and the
/// `io::ErrorKind` it failed with. It carries no path and no audio, so it may be logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("call track {op} failed: {kind}")]
pub struct StorageError {
    pub op: StorageOp,
    pub kind: io::ErrorKind,
}

/// The step of writing a call track that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOp {
    /// Writing bytes to the file.
    Write,
    /// Flushing the written bytes (after every page).
    Flush,
    /// `sync_data`, every 5 s and at the end.
    Sync,
    /// libopus refused to set up the encoder or to encode a frame. Not an `io::Error`
    /// (its `kind` is `Other`); with valid settings and frames this does not happen.
    Encode,
}

impl fmt::Display for StorageOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StorageOp::Write => "write",
            StorageOp::Flush => "flush",
            StorageOp::Sync => "sync",
            StorageOp::Encode => "encode",
        })
    }
}

/// Why reading a call track stopped before a clean end of stream (research R7). A truncated
/// tail is not one of these: the reader ends with the recoverable data instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeEnd {
    /// No valid `OpusHead` on the first page: there is no audio to read.
    #[error("call track unreadable: no valid OpusHead")]
    Unreadable,
    /// A page failed its CRC and a valid page followed it: the readable prefix was returned.
    #[error("call track damaged: a bad page before the end")]
    Damaged,
    /// libopus refused to set up the decoder or to decode a packet of a CRC-valid page.
    #[error("call track packet could not be decoded")]
    Codec,
}
