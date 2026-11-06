//! Call folders on disk (spec 007 FR-015, FR-033, FR-039; T-125): [`CallStore`] creates
//! `<root>/<CallId>`, writes `meta.json` atomically and lists the calls of every root.
//!
//! - [`CallStore::create`] probes the current root ([`CallRoots::probe`], the one probe)
//!   before anything else, then makes the folder with `create_dir` (never an existing
//!   folder; clash suffixes `-1` … `-99`, past that the start is refused as root `other`),
//!   writes `meta.json` and opens both tracks through [`OpusOgg::create`]. Any failure
//!   after the folder exists removes the folder, so a refusal leaves nothing new on disk.
//! - [`write_meta`] is the one `meta.json` writer: `meta.json.tmp` written and synced,
//!   then renamed over `meta.json`; never in place.
//! - [`CallStore::list`] reads the current root ([`effective_root`]) and every earlier
//!   root. A name outside the [`CallId`] grammar is not a call and is not listed; a call
//!   folder whose `meta.json` is missing, unparsable, of another schema or carries an
//!   unknown key, or whose track file is missing, lists as [`ListState::Damaged`]. Entries
//!   are newest first by (start stamp, numeric suffix), not by `CallId`'s string order.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::codec::{OpusOgg, SyncWrite};
use super::events::{
    CallId, CallState, DeviceCause, EndpointId, FailReason, PassMode, StopReason, StorageKind,
    Track,
};
use super::transcript::{
    export_json, export_markdown, EngineRef, SegmentV1, Speaker, TranscriptV1,
};
use super::StorageOp;
use crate::clock::MonotonicClock;
use crate::diag::format::LocalTime;
use crate::i18n::{text, UiLanguage, CALLS_TITLE_DEFAULT, SPEAKER_ME, SPEAKER_OTHER};
use crate::settings::roots::{effective_root, reason_of, CallRoots, UnwritableReason};
use crate::settings::CallSettings;

/// The `meta.json` schema this build writes and reads.
pub const META_SCHEMA: u32 = 1;
/// The file name of a call's metadata.
pub const META_FILE: &str = "meta.json";
const MIC_FILE: &str = "mic.opus";
const SYSTEM_FILE: &str = "system.opus";
/// The sample rate of both tracks (contracts/core-call.md).
const TRACK_RATE: u32 = 16_000;
/// The highest clash suffix (`-99`, #242 D5).
const MAX_SUFFIX: u32 = 99;
/// Ogg stream serials; each track file holds one logical stream.
const MIC_SERIAL: u32 = 1;
const SYSTEM_SERIAL: u32 = 2;
const MIC_SPEAKER: &str = "me";
const SYSTEM_SPEAKER: &str = "s1";

/// The one mapping of a failed track step to the log's storage kind (`Encode` -> `Write`, #242 D1).
pub fn storage_kind(op: StorageOp) -> StorageKind {
    match op {
        StorageOp::Write | StorageOp::Encode => StorageKind::Write,
        StorageOp::Flush => StorageKind::Flush,
        StorageOp::Sync => StorageKind::Sync,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub reason: FailReason,
    pub track: Option<Track>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub record: bool,
    pub subtitles: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackMeta {
    pub track: Track,
    pub file: String,
    pub rate: u32,
    pub start_offset_ms: u64,
    pub endpoint: EndpointId,
    pub speaker: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceChange {
    pub at_ms: u64,
    pub track: Track,
    pub cause: DeviceCause,
    pub to: Option<EndpointId>,
    pub gap_ms: u64,
}

/// A run of blocks a full track channel dropped (`meta.json` `drops[]`; T-126, decision
/// #244): where on the track (`at_ms`), which track, how many blocks and the time they
/// spanned. The same facts as the `call_track_dropped` log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackDrop {
    pub at_ms: u64,
    pub track: Track,
    pub blocks: u32,
    pub ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engines {
    pub transcription: Option<EngineRef>,
    pub diarization: Option<EngineRef>,
}

/// A track's final-pass result (data-model "FinalPass and the queue": `pending` |
/// `done{segments}` | `failed{reason}`). A new call writes `pending`; the final pass
/// (`call::final_pass`) writes `done` with the track's merged segments or `failed` with the
/// track's reason, and `list` reads all three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackResult {
    Pending,
    Done { segments: Vec<SegmentV1> },
    Failed { reason: FailReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Results {
    pub mic: TrackResult,
    pub system: TrackResult,
}

/// `meta.json`, schema 1; the key order is the data-model.md "Call" field order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallMeta {
    pub schema: u32,
    pub id: CallId,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub state: CallState,
    pub failure: Option<Failure>,
    pub stop_reason: Option<StopReason>,
    pub mode: Mode,
    pub tracks: [TrackMeta; 2],
    pub device_changes: Vec<DeviceChange>,
    /// Channel drops (#244); required, `[]` at create.
    pub drops: Vec<TrackDrop>,
    pub engines: Engines,
    pub app_version: String,
    pub speakers: Vec<Speaker>,
    pub results: Results,
    pub pass_mode: Option<PassMode>,
}

/// The one `meta.json` writer: `meta.json.tmp` written and synced, then renamed over `meta.json`.
pub fn write_meta(dir: &Path, meta: &CallMeta) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    write_atomic(dir, META_FILE, &bytes)
}

/// Writes `bytes` as `<dir>/<name>` whole: `<name>.tmp` created, written and synced, then
/// renamed over `<name>`. On failure the previous `<name>` is untouched and a tmp file this
/// call created is removed (best effort); a tmp path it could not create is left as found.
///
/// The call folder is ours, so the tmp is opened with `File::create`: a `<name>.tmp` left by
/// a crash is replaced, not an error. The rest is the one tail [`fill_and_rename`].
pub(crate) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let target = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    let file = File::create(&tmp)?;
    fill_and_rename(file, &tmp, &target, bytes)
}

/// The one tmp+sync+rename tail of [`write_atomic`] and [`export_to`]: writes `bytes` to
/// `file` (the open tmp at `tmp`, created by the caller), syncs its data, closes it and
/// renames `tmp` over `target`. On any failure `tmp` is removed (best effort: the caller
/// created it) and the first error is returned; `target` is untouched.
fn fill_and_rename<W: SyncWrite>(
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

/// What a start needs to name and describe the call.
#[derive(Debug, Clone)]
pub struct NewCall {
    pub started: SystemTime,
    pub seconds_east: i32,
    pub mic: EndpointId,
    pub system: EndpointId,
    pub lang: UiLanguage,
    pub app_version: String,
}

/// Why a start could not create the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateError {
    Root(UnwritableReason),
    Storage(StorageKind),
}

/// A created call: its folder, its `meta.json` and the two open track encoders.
pub struct ActiveCallFiles<W: SyncWrite = File> {
    pub mic: OpusOgg<W>,
    pub system: OpusOgg<W>,
    meta: CallMeta,
    dir: PathBuf,
}

impl<W: SyncWrite> ActiveCallFiles<W> {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn meta(&self) -> &CallMeta {
        &self.meta
    }

    /// Writes `meta` through [`write_meta`] into this call's folder.
    /// [`ActiveCallFiles::meta`] becomes `meta` only when the write succeeded.
    pub fn write_meta(&mut self, meta: &CallMeta) -> io::Result<()> {
        write_meta(&self.dir, meta)?;
        self.meta = meta.clone();
        Ok(())
    }
}

/// How a listed call shows in the Calls window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    Recording,
    Queued,
    Transcribing,
    Ready,
    Failed(FailReason),
    Recovered,
    Damaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEntry {
    pub id: CallId,
    pub root: PathBuf,
    pub state: ListState,
    /// `None` for a damaged folder.
    pub meta: Option<CallMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallList {
    pub entries: Vec<CallEntry>,
    /// One line per root that could not be read.
    pub unavailable: Vec<PathBuf>,
}

pub struct CallStore {
    roots: Arc<dyn CallRoots>,
    clock: Arc<dyn MonotonicClock>,
}

impl CallStore {
    pub fn new(roots: Arc<dyn CallRoots>, clock: Arc<dyn MonotonicClock>) -> CallStore {
        CallStore { roots, clock }
    }

    /// [`CallStore::create_with`] over real files.
    pub fn create(
        &self,
        calls: &CallSettings,
        new: &NewCall,
    ) -> Result<ActiveCallFiles<File>, CreateError> {
        self.create_with(calls, new, |path| File::create(path))
    }

    /// Creates the call; `open` opens a track file (a test hands in a fault-injecting writer).
    ///
    /// Order: the probe of the current root, the folder (`create_dir`, suffixed on a
    /// clash), `meta.json` (state `recording`), `mic.opus`, `system.opus`. A refused probe
    /// touches nothing; any later failure removes the folder it made.
    pub fn create_with<W: SyncWrite>(
        &self,
        calls: &CallSettings,
        new: &NewCall,
        mut open: impl FnMut(&Path) -> io::Result<W>,
    ) -> Result<ActiveCallFiles<W>, CreateError> {
        let root = effective_root(calls, &self.roots.default_root());
        self.roots.probe(&root).map_err(CreateError::Root)?;
        let local = LocalTime::of(new.started, new.seconds_east);
        let (id, dir) = make_call_dir(&root, &folder_stamp(&local))?;
        let meta = initial_meta(id, &local, new);
        let made = write_meta(&dir, &meta)
            .map_err(|_| CreateError::Storage(StorageKind::Meta))
            .and_then(|()| self.open_track(&mut open, &dir.join(MIC_FILE), MIC_SERIAL))
            .and_then(|mic| {
                let system = self.open_track(&mut open, &dir.join(SYSTEM_FILE), SYSTEM_SERIAL)?;
                Ok((mic, system))
            });
        match made {
            Ok((mic, system)) => Ok(ActiveCallFiles {
                mic,
                system,
                meta,
                dir,
            }),
            Err(e) => {
                // Every track writer is dropped (closed) by now. Best effort: the folder
                // is the one this call created; the error returned is the step's own.
                let _ = fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    fn open_track<W: SyncWrite>(
        &self,
        open: &mut impl FnMut(&Path) -> io::Result<W>,
        path: &Path,
        serial: u32,
    ) -> Result<OpusOgg<W>, CreateError> {
        let inner = open(path).map_err(|_| CreateError::Storage(StorageKind::Write))?;
        OpusOgg::create(inner, serial, Arc::clone(&self.clock))
            .map_err(|e| CreateError::Storage(storage_kind(e.op)))
    }

    /// Every call of the current root and of every earlier root, newest first; a root
    /// that cannot be read is one `unavailable` line. A missing default root (created
    /// only at the first start) lists as empty. Never creates anything.
    pub fn list(&self, calls: &CallSettings) -> CallList {
        let default_root = self.roots.default_root();
        let current = effective_root(calls, &default_root);
        let mut entries = Vec::new();
        let mut unavailable = Vec::new();
        let roots = std::iter::once(&current).chain(calls.earlier_roots.iter());
        for root in roots {
            let read = match fs::read_dir(root) {
                Ok(read) => read,
                Err(e) => {
                    let unborn_default = calls.storage_root.is_none()
                        && *root == current
                        && e.kind() == io::ErrorKind::NotFound;
                    if !unborn_default {
                        unavailable.push(root.clone());
                    }
                    continue;
                }
            };
            let mut readable = true;
            for item in read {
                let Ok(item) = item else {
                    readable = false;
                    continue;
                };
                let Some(id) = item.file_name().to_str().and_then(CallId::parse) else {
                    continue;
                };
                entries.push(load_entry(root, id));
            }
            if !readable {
                unavailable.push(root.clone());
            }
        }
        // Stable: equal ids keep root order (current first, then earlier roots in order).
        entries.sort_by(|a, b| newest_first(&a.id, &b.id));
        CallList {
            entries,
            unavailable,
        }
    }
}

// ---- The Calls IPC wire, its path guard and export (T-141, contracts/ipc.md) -------------

/// The file name of a call's transcript.
/// The transcript file of a call folder, written by the final pass.
pub(crate) const TRANSCRIPT_FILE: &str = "transcript.json";
/// Its Markdown rendering, written by the final pass before `transcript.json`.
pub(crate) const TRANSCRIPT_MD_FILE: &str = "transcript.md";
/// The suffix of an export's tmp file next to its target; `-1` … `-9` on a clash.
const EXPORT_TMP_SUFFIX: &str = ".voicen-tmp";
/// The highest clash suffix of an export's tmp file.
const MAX_EXPORT_TMP_SUFFIX: u32 = 9;

/// The opaque `root` string of the wire for a root path: its UTF-8 text. A path that is not
/// valid Unicode gets a lossy text, which [`resolve_call`] never accepts (it lists, but
/// cannot be opened).
pub fn wire_root(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Wire `state` of a [`CallSummary`] (camelCase), one value per [`ListState`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WireState {
    Recording,
    Queued,
    Transcribing,
    Ready,
    Failed,
    Recovered,
    Damaged,
}

/// Wire `failReason` (camelCase), one value per [`FailReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WireFailReason {
    Key,
    Network,
    Timeout,
    Provider,
    Storage,
    EngineNotSetUp,
}

/// `CallSummary` of contracts/ipc.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSummary {
    pub id: String,
    pub root: String,
    pub title: String,
    pub started_at: String,
    pub duration_ms: Option<u64>,
    pub state: WireState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<WireFailReason>,
}

/// `CallList` of contracts/ipc.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallListWire {
    pub calls: Vec<CallSummary>,
    pub unavailable_roots: Vec<String>,
}

/// `Speaker` of contracts/ipc.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireSpeaker {
    pub id: String,
    pub name: String,
}

/// `Line` of contracts/ipc.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub start_ms: i64,
    pub end_ms: i64,
    pub track: String,
    pub speaker: String,
    pub lang: Option<String>,
    pub text: String,
}

/// `CallDetail` of contracts/ipc.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallDetail {
    pub summary: CallSummary,
    pub speakers: Vec<WireSpeaker>,
    pub lines: Vec<Line>,
}

/// `CallError.code` of contracts/ipc.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CallErrorCode {
    NotFound,
    InUse,
    EmptyName,
    RecordingCall,
    IoFailed,
}

/// `CallError` of contracts/ipc.md; `reason` is the snake_case [`UnwritableReason`] word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallError {
    pub code: CallErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<UnwritableReason>,
}

impl CallError {
    /// `ioFailed` with `reason = reason_of(kind)`.
    pub fn io_failed(kind: io::ErrorKind) -> CallError {
        CallError {
            code: CallErrorCode::IoFailed,
            file: None,
            reason: Some(reason_of(kind)),
        }
    }

    /// `notFound`.
    pub fn not_found() -> CallError {
        CallError {
            code: CallErrorCode::NotFound,
            file: None,
            reason: None,
        }
    }
}

/// The export formats of `calls_export` / `calls_copy_markdown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Md,
    Json,
}

/// The guard: `root` must be `wire_root` of the current or an earlier root of `calls`
/// (`default_root` is the current one when none is chosen) and `id` a [`CallId`]. Pure: no
/// filesystem call. Anything else is `notFound`; no path is built from wire text.
pub fn resolve_call(
    calls: &CallSettings,
    default_root: &Path,
    root: &str,
    id: &str,
) -> Result<PathBuf, CallError> {
    guard(calls, default_root, root, id).map(|(root, id)| root.join(id.as_str()))
}

/// The guard itself: the matching settings root (never the wire text) and the parsed id.
/// A root that is not valid Unicode has no `to_str` and matches nothing.
fn guard(
    calls: &CallSettings,
    default_root: &Path,
    root: &str,
    id: &str,
) -> Result<(PathBuf, CallId), CallError> {
    let current = effective_root(calls, default_root);
    let known = std::iter::once(&current)
        .chain(calls.earlier_roots.iter())
        .find(|p| p.to_str() == Some(root))
        .ok_or_else(CallError::not_found)?;
    let id = CallId::parse(id).ok_or_else(CallError::not_found)?;
    Ok((known.clone(), id))
}

/// The one mapping of a listed state to the wire `state` and `failReason`.
fn wire_state(state: ListState) -> (WireState, Option<WireFailReason>) {
    match state {
        ListState::Recording => (WireState::Recording, None),
        ListState::Queued => (WireState::Queued, None),
        ListState::Transcribing => (WireState::Transcribing, None),
        ListState::Ready => (WireState::Ready, None),
        ListState::Failed(reason) => (WireState::Failed, Some(wire_fail_reason(reason))),
        ListState::Recovered => (WireState::Recovered, None),
        ListState::Damaged => (WireState::Damaged, None),
    }
}

fn wire_fail_reason(reason: FailReason) -> WireFailReason {
    match reason {
        FailReason::Key => WireFailReason::Key,
        FailReason::Network => WireFailReason::Network,
        FailReason::Timeout => WireFailReason::Timeout,
        FailReason::Provider => WireFailReason::Provider,
        FailReason::Storage => WireFailReason::Storage,
        FailReason::EngineNotSetUp => WireFailReason::EngineNotSetUp,
    }
}

/// The wire summary of a listed entry; a damaged one gets its default title and a
/// `startedAt` from its stamp, in `lang`, and `durationMs: null`.
pub fn summary_of(entry: &CallEntry, lang: UiLanguage) -> CallSummary {
    let (state, fail_reason) = wire_state(entry.state);
    let (title, started_at, duration_ms) = match &entry.meta {
        Some(meta) => (
            meta.title.clone(),
            meta.started_at.clone(),
            meta.duration_ms,
        ),
        None => {
            // The grammar fixes `YYYY-MM-DD_HHMMSS` as the first 17 bytes of every id.
            let stamp = entry.id.as_str();
            let part = |from: usize, to: usize| stamp.get(from..to).unwrap_or_default();
            let title = default_title(lang, part(8, 10), part(5, 7), part(11, 13), part(13, 15));
            let started_at = format!(
                "{}T{}:{}:{}",
                part(0, 10),
                part(11, 13),
                part(13, 15),
                part(15, 17)
            );
            (title, started_at, None)
        }
    };
    CallSummary {
        id: entry.id.as_str().to_string(),
        root: wire_root(&entry.root),
        title,
        started_at,
        duration_ms,
        state,
        fail_reason,
    }
}

/// The wire list: one summary per entry (same order), one `unavailableRoots` text per root.
pub fn list_wire(list: &CallList, lang: UiLanguage) -> CallListWire {
    CallListWire {
        calls: list.entries.iter().map(|e| summary_of(e, lang)).collect(),
        unavailable_roots: list.unavailable.iter().map(|p| wire_root(p)).collect(),
    }
}

/// The wire detail of an entry and its transcript (`None` -> no lines); `speakers` come
/// from the entry's meta.
pub fn detail_of(
    entry: &CallEntry,
    transcript: Option<&TranscriptV1>,
    lang: UiLanguage,
) -> CallDetail {
    let speakers = entry
        .meta
        .iter()
        .flat_map(|meta| meta.speakers.iter())
        .map(|s| WireSpeaker {
            id: s.id.clone(),
            name: s.name.clone(),
        })
        .collect();
    let lines = transcript
        .iter()
        .flat_map(|t| t.segments.iter())
        .map(|s| Line {
            start_ms: s.start_ms,
            end_ms: s.end_ms,
            track: s.track.clone(),
            speaker: s.speaker.clone(),
            lang: s.lang.clone(),
            text: s.text.clone(),
        })
        .collect();
    CallDetail {
        summary: summary_of(entry, lang),
        speakers,
        lines,
    }
}

/// `ioFailed` for a call folder or transcript that is there but cannot be used (reason `other`).
fn unusable() -> CallError {
    CallError::io_failed(io::ErrorKind::InvalidData)
}

/// The call's `transcript.json`, parsed and validated; `None` when there is none.
fn read_transcript(dir: &Path) -> Result<Option<TranscriptV1>, CallError> {
    let text = match fs::read_to_string(dir.join(TRANSCRIPT_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CallError::io_failed(e.kind())),
    };
    let transcript = TranscriptV1::from_json(&text).map_err(|_| unusable())?;
    transcript.validate().map_err(|_| unusable())?;
    Ok(Some(transcript))
}

impl CallStore {
    /// Guard, then the call's meta and transcript; missing folder `notFound`, damaged or
    /// broken transcript `ioFailed`, no transcript.json empty `lines`.
    pub fn get(
        &self,
        calls: &CallSettings,
        root: &str,
        id: &str,
        lang: UiLanguage,
    ) -> Result<CallDetail, CallError> {
        let (entry, dir) = self.open_call(calls, root, id)?;
        let transcript = read_transcript(&dir)?;
        Ok(detail_of(&entry, transcript.as_ref(), lang))
    }

    /// The bytes copy and export share: Markdown or JSON of the call's valid transcript
    /// with the title and names of meta.json. No transcript is `ioFailed{missing}`.
    pub fn export_bytes(
        &self,
        calls: &CallSettings,
        root: &str,
        id: &str,
        format: Format,
    ) -> Result<Vec<u8>, CallError> {
        let (entry, dir) = self.open_call(calls, root, id)?;
        let meta = entry.meta.as_ref().ok_or_else(unusable)?;
        let transcript =
            read_transcript(&dir)?.ok_or_else(|| CallError::io_failed(io::ErrorKind::NotFound))?;
        let text = match format {
            Format::Md => export_markdown(&transcript, &meta.title, &meta.speakers),
            Format::Json => export_json(&transcript, &meta.title, &meta.speakers),
        };
        Ok(text.into_bytes())
    }

    /// The guard, then the listed entry of the call folder ([`load_entry`], as the list
    /// reads it): a folder that is not there is `notFound`, a damaged one `ioFailed`.
    fn open_call(
        &self,
        calls: &CallSettings,
        root: &str,
        id: &str,
    ) -> Result<(CallEntry, PathBuf), CallError> {
        let (root, id) = guard(calls, &self.roots.default_root(), root, id)?;
        let dir = root.join(id.as_str());
        if !dir.is_dir() {
            return Err(CallError::not_found());
        }
        let entry = load_entry(&root, id);
        if entry.meta.is_none() {
            return Err(unusable());
        }
        Ok((entry, dir))
    }
}

/// Writes `bytes` to `target` through a fresh tmp file reserved next to it with `create_new`
/// and the one tmp+sync+rename tail; leaves nothing of this call on failure. A file that is
/// already there under a tmp name is never opened, truncated or removed. A target without a
/// parent or a file name is `ioFailed{other}`; every io error is `ioFailed{reason_of(kind)}`.
pub fn export_to(target: &Path, bytes: &[u8]) -> Result<(), CallError> {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(CallError::io_failed(io::ErrorKind::InvalidInput));
    };
    let (file, tmp) = reserve_export_tmp(dir, name).map_err(|e| tmp_open_failed(dir, &e))?;
    fill_and_rename(file, &tmp, target, bytes).map_err(|e| CallError::io_failed(e.kind()))
}

/// The `ioFailed` of a tmp that could not be opened in `dir`. A `dir` that is a regular file
/// is `not_a_folder` on every OS: Unix reports a path below a file as "not a directory",
/// Windows as "path not found" (D4). Runs only after the open failed; it reads metadata and
/// never creates or opens anything.
fn tmp_open_failed(dir: &Path, e: &io::Error) -> CallError {
    if e.kind() == io::ErrorKind::NotFound && fs::metadata(dir).is_ok_and(|m| !m.is_dir()) {
        return CallError::io_failed(io::ErrorKind::NotADirectory);
    }
    CallError::io_failed(e.kind())
}

/// Creates `<dir>/<name>.voicen-tmp`, else `-1` … `-9`, with `create_new`: never a file that
/// is already there. All taken is `AlreadyExists`.
fn reserve_export_tmp(dir: &Path, name: &std::ffi::OsStr) -> io::Result<(File, PathBuf)> {
    for n in 0..=MAX_EXPORT_TMP_SUFFIX {
        let mut tmp_name = name.to_os_string();
        tmp_name.push(EXPORT_TMP_SUFFIX);
        if n > 0 {
            tmp_name.push(format!("-{n}"));
        }
        let tmp = dir.join(tmp_name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// `YYYY-MM-DD_HHMMSS` of the local start.
fn folder_stamp(t: &LocalTime) -> String {
    format!(
        "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// `YYYY-MM-DDTHH:MM:SS±HH:MM` of the local start (whole seconds).
pub(crate) fn started_at(t: &LocalTime) -> String {
    let sign = if t.offset < 0 { '-' } else { '+' };
    let abs = t.offset.unsigned_abs();
    let mut out = String::with_capacity(25);
    let _ = write!(
        out,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        t.year,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second,
        abs / 3_600,
        abs % 3_600 / 60
    );
    out
}

/// Creates `<root>/<stamp>`, else `<stamp>-1` … `<stamp>-99`, never an existing folder.
fn make_call_dir(root: &Path, stamp: &str) -> Result<(CallId, PathBuf), CreateError> {
    let names =
        std::iter::once(stamp.to_string()).chain((1..=MAX_SUFFIX).map(|n| format!("{stamp}-{n}")));
    for name in names {
        let Some(id) = CallId::parse(&name) else {
            return Err(CreateError::Root(UnwritableReason::Other));
        };
        let dir = root.join(&name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CreateError::Root(reason_of(e.kind()))),
        }
    }
    Err(CreateError::Root(UnwritableReason::Other))
}

fn two(n: u64) -> String {
    format!("{n:02}")
}

/// `calls.title_default` in `lang`: the title of a new call, and of a damaged one on the wire.
fn default_title(lang: UiLanguage, day: &str, month: &str, hour: &str, minute: &str) -> String {
    text(
        lang,
        CALLS_TITLE_DEFAULT,
        &[
            ("day", day),
            ("month", month),
            ("hour", hour),
            ("minute", minute),
        ],
    )
}

fn initial_meta(id: CallId, local: &LocalTime, new: &NewCall) -> CallMeta {
    let title = default_title(
        new.lang,
        &two(local.day),
        &two(local.month),
        &two(local.hour),
        &two(local.minute),
    );
    let track = |track, file: &str, endpoint: &EndpointId, speaker: &str| TrackMeta {
        track,
        file: file.to_string(),
        rate: TRACK_RATE,
        start_offset_ms: 0,
        endpoint: endpoint.clone(),
        speaker: speaker.to_string(),
    };
    CallMeta {
        schema: META_SCHEMA,
        id,
        title,
        started_at: started_at(local),
        ended_at: None,
        duration_ms: None,
        state: CallState::Recording,
        failure: None,
        stop_reason: None,
        mode: Mode {
            record: true,
            subtitles: false,
        },
        tracks: [
            track(Track::Mic, MIC_FILE, &new.mic, MIC_SPEAKER),
            track(Track::System, SYSTEM_FILE, &new.system, SYSTEM_SPEAKER),
        ],
        device_changes: Vec::new(),
        drops: Vec::new(),
        engines: Engines {
            transcription: None,
            diarization: None,
        },
        app_version: new.app_version.clone(),
        speakers: vec![
            Speaker {
                id: MIC_SPEAKER.to_string(),
                name: text(new.lang, SPEAKER_ME, &[]),
            },
            Speaker {
                id: SYSTEM_SPEAKER.to_string(),
                name: text(new.lang, SPEAKER_OTHER, &[]),
            },
        ],
        results: Results {
            mic: TrackResult::Pending,
            system: TrackResult::Pending,
        },
        pass_mode: None,
    }
}

/// The listed entry of the call folder `<root>/<id>`: its meta, or damaged.
fn load_entry(root: &Path, id: CallId) -> CallEntry {
    let dir = root.join(id.as_str());
    match read_meta(&dir, &id) {
        Some((meta, state)) => CallEntry {
            id,
            root: root.to_path_buf(),
            state,
            meta: Some(meta),
        },
        None => CallEntry {
            id,
            root: root.to_path_buf(),
            state: ListState::Damaged,
            meta: None,
        },
    }
}

/// `None` (damaged) for a missing or unparsable `meta.json` (an unknown key included), an
/// unknown schema, an id that is not the folder's, tracks other than `mic.opus` /
/// `system.opus`, a missing track file, or `failed` without its reason.
fn read_meta(dir: &Path, id: &CallId) -> Option<(CallMeta, ListState)> {
    let bytes = fs::read(dir.join(META_FILE)).ok()?;
    let meta: CallMeta = serde_json::from_slice(&bytes).ok()?;
    if meta.schema != META_SCHEMA || meta.id != *id {
        return None;
    }
    let [mic, system] = &meta.tracks;
    let tracks_ok = mic.track == Track::Mic
        && mic.file == MIC_FILE
        && system.track == Track::System
        && system.file == SYSTEM_FILE
        && dir.join(MIC_FILE).is_file()
        && dir.join(SYSTEM_FILE).is_file();
    if !tracks_ok {
        return None;
    }
    let state = match meta.state {
        CallState::Recording => ListState::Recording,
        CallState::ReadyToTranscribe => ListState::Queued,
        CallState::Transcribing => ListState::Transcribing,
        CallState::Ready => ListState::Ready,
        CallState::Failed => ListState::Failed(meta.failure.as_ref()?.reason),
        CallState::Recovered => ListState::Recovered,
    };
    Some((meta, state))
}

/// Newest first: the start stamp descending, then the clash suffix descending by its
/// numeric value (`-10` before `-2`, any suffix before none).
fn newest_first(a: &CallId, b: &CallId) -> Ordering {
    sort_key(b).cmp(&sort_key(a))
}

/// `(stamp, suffix digits without leading zeros by length, then text)`; the grammar
/// fixes the stamp at 17 bytes and the suffix at `-<digits>`.
fn sort_key(id: &CallId) -> (&str, usize, &str) {
    let text = id.as_str();
    let (stamp, suffix) = text.split_at(text.len().min(17));
    let digits = suffix.trim_start_matches('-').trim_start_matches('0');
    (stamp, digits.len(), digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{days_from_civil, SystemMonotonic};
    use crate::settings::defaults;
    use crate::settings::roots::FsCallRoots;
    use crate::test_support::TempDir;
    use serde_json::Value;
    use std::fs;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::{Duration, UNIX_EPOCH};

    // ---- helpers ------------------------------------------------------------------

    /// `FsCallRoots` that records every probe and can be told to refuse.
    struct ProbeLog {
        inner: FsCallRoots,
        probes: Mutex<Vec<PathBuf>>,
        refuse: Option<UnwritableReason>,
    }

    impl ProbeLog {
        fn probed(&self) -> Vec<PathBuf> {
            self.probes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl CallRoots for ProbeLog {
        fn default_root(&self) -> PathBuf {
            self.inner.default_root()
        }
        fn probe(&self, root: &Path) -> Result<(), UnwritableReason> {
            self.probes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(root.to_path_buf());
            match self.refuse {
                Some(reason) => Err(reason),
                None => self.inner.probe(root),
            }
        }
    }

    struct Fixture {
        dir: TempDir,
        default_root: PathBuf,
        roots: Arc<ProbeLog>,
        store: CallStore,
    }

    fn fixture_with(refuse: Option<UnwritableReason>) -> Fixture {
        let dir = TempDir::new();
        let default_root = dir.path().join("data").join("calls");
        let roots = Arc::new(ProbeLog {
            inner: FsCallRoots::new(default_root.clone()),
            probes: Mutex::new(Vec::new()),
            refuse,
        });
        let store = CallStore::new(roots.clone(), Arc::new(SystemMonotonic));
        Fixture {
            dir,
            default_root,
            roots,
            store,
        }
    }

    fn fixture() -> Fixture {
        fixture_with(None)
    }

    /// Settings whose current root is `root` (created by the caller) and earlier roots.
    fn calls_in(root: Option<&Path>, earlier: &[&Path]) -> CallSettings {
        let mut calls = defaults(None).calls;
        calls.storage_root = root.map(Path::to_path_buf);
        calls.earlier_roots = earlier.iter().map(|p| p.to_path_buf()).collect();
        calls
    }

    fn utc(y: u64, m: u64, d: u64, h: u64, mi: u64, s: u64) -> SystemTime {
        let days = days_from_civil(y, m, d).expect("date");
        UNIX_EPOCH + Duration::from_secs(days * 86_400 + h * 3_600 + mi * 60 + s)
    }

    fn new_call(started: SystemTime, seconds_east: i32, lang: UiLanguage) -> NewCall {
        NewCall {
            started,
            seconds_east,
            mic: EndpointId::new("{0.0.1.00000000}.{aaaa-0001}"),
            system: EndpointId::new("{0.0.0.00000000}.{bbbb-0002}"),
            lang,
            app_version: "9.9.9".to_string(),
        }
    }

    /// 2026-10-30 14:05:09 at UTC+03:00.
    fn moscow_call() -> NewCall {
        new_call(utc(2026, 10, 30, 11, 5, 9), 10_800, UiLanguage::En)
    }

    const MOSCOW_ID: &str = "2026-10-30_140509";

    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(dir)
            .expect("list dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).expect("read")).expect("json")
    }

    fn sample_meta(id: &str, state: CallState) -> CallMeta {
        CallMeta {
            schema: 1,
            id: CallId::parse(id).expect("call id"),
            title: "Call 30.10 10:10".to_string(),
            started_at: "2026-10-30T10:10:10+00:00".to_string(),
            ended_at: None,
            duration_ms: None,
            state,
            failure: None,
            stop_reason: None,
            mode: Mode {
                record: true,
                subtitles: false,
            },
            tracks: [
                TrackMeta {
                    track: Track::Mic,
                    file: "mic.opus".to_string(),
                    rate: 16_000,
                    start_offset_ms: 0,
                    endpoint: EndpointId::new("{0.0.1.00000000}.{aaaa-0001}"),
                    speaker: "me".to_string(),
                },
                TrackMeta {
                    track: Track::System,
                    file: "system.opus".to_string(),
                    rate: 16_000,
                    start_offset_ms: 0,
                    endpoint: EndpointId::new("{0.0.0.00000000}.{bbbb-0002}"),
                    speaker: "s1".to_string(),
                },
            ],
            device_changes: Vec::new(),
            drops: Vec::new(),
            engines: Engines {
                transcription: None,
                diarization: None,
            },
            app_version: "9.9.9".to_string(),
            speakers: vec![
                Speaker {
                    id: "me".to_string(),
                    name: "Me".to_string(),
                },
                Speaker {
                    id: "s1".to_string(),
                    name: "Other party".to_string(),
                },
            ],
            results: Results {
                mic: TrackResult::Pending,
                system: TrackResult::Pending,
            },
            pass_mode: None,
        }
    }

    /// A call folder as another run would have left it.
    fn plant_value(root: &Path, id: &str, meta: Option<&Value>, mic: bool, system: bool) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).expect("folder");
        if let Some(meta) = meta {
            fs::write(dir.join("meta.json"), meta.to_string()).expect("meta");
        }
        if mic {
            fs::write(dir.join("mic.opus"), b"x").expect("mic");
        }
        if system {
            fs::write(dir.join("system.opus"), b"x").expect("system");
        }
    }

    fn meta_value(id: &str, state: CallState) -> Value {
        serde_json::to_value(sample_meta(id, state)).expect("value")
    }

    fn plant(root: &Path, id: &str, state: CallState) {
        plant_value(root, id, Some(&meta_value(id, state)), true, true);
    }

    fn ids(list: &CallList) -> Vec<String> {
        list.entries.iter().map(|e| e.id.to_string()).collect()
    }

    /// The keys of the outermost JSON object, in file order.
    fn top_level_keys(text: &str) -> Vec<String> {
        let b = text.as_bytes();
        let (mut keys, mut depth, mut i) = (Vec::new(), 0usize, 0usize);
        while i < b.len() {
            match b[i] {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b'"' => {
                    let start = i + 1;
                    i += 1;
                    while b[i] != b'"' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    let mut j = i + 1;
                    while b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if depth == 1 && b[j] == b':' {
                        keys.push(String::from_utf8_lossy(&b[start..i]).into_owned());
                    }
                }
                _ => {}
            }
            i += 1;
        }
        keys
    }

    /// A track writer over a real file that fails exactly one call (F-024): the first write
    /// or the first flush, once; every other call, before and after, succeeds.
    struct FaultSink {
        file: File,
        fail_write: bool,
        fail_flush: bool,
    }

    impl FaultSink {
        fn healthy(path: &Path) -> io::Result<FaultSink> {
            Ok(FaultSink {
                file: File::create(path)?,
                fail_write: false,
                fail_flush: false,
            })
        }
    }

    impl Write for FaultSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.fail_write) {
                return Err(io::Error::other("fake write fault"));
            }
            self.file.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            if std::mem::take(&mut self.fail_flush) {
                return Err(io::Error::other("fake flush fault"));
            }
            self.file.flush()
        }
    }

    impl SyncWrite for FaultSink {
        fn sync_data(&mut self) -> io::Result<()> {
            self.file.sync_data()
        }
    }

    // ---- create -------------------------------------------------------------------

    #[test]
    fn create_names_folder_from_local_start_and_suffixes_on_clash_never_overwrites() {
        // Bite: UTC instead of local time; `create_dir_all` / reuse of an existing folder;
        // a clash that overwrites; no suffix counting (-1, -2).
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let sentinel = root.join(MOSCOW_ID).join("keep.txt");
        fs::create_dir(root.join(MOSCOW_ID)).expect("existing");
        fs::write(&sentinel, b"user data").expect("sentinel");

        let first = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("first create: {e:?}"));
        assert_eq!(first.dir(), root.join(format!("{MOSCOW_ID}-1")));
        let second = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("second create: {e:?}"));
        assert_eq!(second.dir(), root.join(format!("{MOSCOW_ID}-2")));
        assert_eq!(second.meta().id.as_str(), format!("{MOSCOW_ID}-2"));
        assert_eq!(fs::read(&sentinel).expect("kept"), b"user data");
        assert_eq!(
            names(&root.join(MOSCOW_ID)),
            vec!["keep.txt".to_string()],
            "the existing folder was written into"
        );
    }

    #[test]
    fn create_names_a_clash_free_start_without_suffix_and_rolls_the_local_date() {
        // 2026-01-01 01:30:00Z at UTC-03:30 is 2025-12-31 22:00:00 local. Bite: UTC date,
        // positive-only offsets, a suffix on the first call.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let call = new_call(utc(2026, 1, 1, 1, 30, 0), -12_600, UiLanguage::En);
        let made = f
            .store
            .create(&calls, &call)
            .unwrap_or_else(|e| panic!("create: {e:?}"));
        assert_eq!(made.dir(), root.join("2025-12-31_220000"));
        let meta = read_json(&root.join("2025-12-31_220000").join("meta.json"));
        assert_eq!(meta["id"], "2025-12-31_220000");
        assert_eq!(meta["started_at"], "2025-12-31T22:00:00-03:30");
        assert_eq!(meta["title"], "Call 31.12 22:00");
    }

    #[test]
    fn create_past_suffix_99_is_refused_as_root_other_and_leaves_nothing() {
        // D5. Bite: unbounded suffixes; a refusal that creates the -100 folder; a different
        // reason.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        fs::create_dir(root.join(MOSCOW_ID)).expect("base");
        for n in 1..=99 {
            fs::create_dir(root.join(format!("{MOSCOW_ID}-{n}"))).expect("suffix");
        }
        let before = names(&root);
        let calls = calls_in(Some(&root), &[]);
        let got = f.store.create(&calls, &moscow_call());
        assert_eq!(got.err(), Some(CreateError::Root(UnwritableReason::Other)));
        assert_eq!(names(&root), before, "the refusal changed the root");
        for name in &before {
            assert_eq!(names(&root.join(name)), Vec::<String>::new(), "{name}");
        }
    }

    #[test]
    fn meta_json_keys_follow_data_model_order_and_start_recording() {
        // Bite: keys in another order (a map, a reordered struct), a state other than
        // `recording`, results not pending, engines/pass_mode not null, a leftover tmp.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let made = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("create: {e:?}"));
        let dir = made.dir().to_path_buf();
        assert_eq!(
            names(&dir),
            vec![
                "meta.json".to_string(),
                "mic.opus".to_string(),
                "system.opus".to_string()
            ]
        );
        let text = fs::read_to_string(dir.join("meta.json")).expect("meta");
        assert_eq!(
            top_level_keys(&text),
            [
                "schema",
                "id",
                "title",
                "started_at",
                "ended_at",
                "duration_ms",
                "state",
                "failure",
                "stop_reason",
                "mode",
                "tracks",
                "device_changes",
                "drops",
                "engines",
                "app_version",
                "speakers",
                "results",
                "pass_mode"
            ]
        );
        let v: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["schema"], 1);
        assert_eq!(v["id"], MOSCOW_ID);
        assert_eq!(v["started_at"], "2026-10-30T14:05:09+03:00");
        assert_eq!(v["state"], "recording");
        assert_eq!(v["ended_at"], Value::Null);
        assert_eq!(v["duration_ms"], Value::Null);
        assert_eq!(v["failure"], Value::Null);
        assert_eq!(v["stop_reason"], Value::Null);
        assert_eq!(v["mode"]["record"], true);
        assert_eq!(v["mode"]["subtitles"], false);
        assert_eq!(v["device_changes"], serde_json::json!([]));
        // T-126 (#244): `drops` follows `device_changes` and starts as an empty array.
        assert_eq!(v["drops"], serde_json::json!([]));
        assert_eq!(v["engines"]["transcription"], Value::Null);
        assert_eq!(v["engines"]["diarization"], Value::Null);
        assert_eq!(v["app_version"], "9.9.9");
        assert_eq!(v["results"]["mic"], "pending");
        assert_eq!(v["results"]["system"], "pending");
        assert_eq!(v["pass_mode"], Value::Null);
        assert_eq!(v["tracks"][0]["track"], "mic");
        assert_eq!(v["tracks"][0]["file"], "mic.opus");
        assert_eq!(v["tracks"][0]["rate"], 16_000);
        assert_eq!(v["tracks"][0]["start_offset_ms"], 0);
        assert_eq!(v["tracks"][0]["endpoint"], "{0.0.1.00000000}.{aaaa-0001}");
        assert_eq!(v["tracks"][0]["speaker"], "me");
        assert_eq!(v["tracks"][1]["track"], "system");
        assert_eq!(v["tracks"][1]["file"], "system.opus");
        assert_eq!(v["tracks"][1]["endpoint"], "{0.0.0.00000000}.{bbbb-0002}");
        assert_eq!(v["tracks"][1]["speaker"], "s1");
    }

    #[test]
    fn default_title_and_speakers_en_ru() {
        // Bite: title in the wrong language, unpadded values, local time not used, speaker
        // names hard-coded in one language.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        // 2026-03-05 09:07:00 local (UTC+00:00).
        let started = utc(2026, 3, 5, 9, 7, 0);
        let en = f
            .store
            .create(&calls, &new_call(started, 0, UiLanguage::En))
            .unwrap_or_else(|e| panic!("en: {e:?}"));
        let v = read_json(&en.dir().join("meta.json"));
        assert_eq!(v["title"], "Call 05.03 09:07");
        assert_eq!(v["speakers"][0]["id"], "me");
        assert_eq!(v["speakers"][0]["name"], "Me");
        assert_eq!(v["speakers"][1]["id"], "s1");
        assert_eq!(v["speakers"][1]["name"], "Other party");

        let ru = f
            .store
            .create(&calls, &new_call(started, 0, UiLanguage::Ru))
            .unwrap_or_else(|e| panic!("ru: {e:?}"));
        let v = read_json(&ru.dir().join("meta.json"));
        let title = v["title"].as_str().expect("title").to_string();
        assert!(!title.contains('{'), "unrendered placeholder: {title:?}");
        assert_ne!(title, "Call 05.03 09:07", "the title is not Russian");
        let digits: String = title.chars().filter(char::is_ascii_digit).collect();
        assert_eq!(digits, "05030907", "{title:?}");
        assert_eq!(v["speakers"][0]["id"], "me");
        assert_eq!(v["speakers"][0]["name"], "Я");
        assert_eq!(v["speakers"][1]["id"], "s1");
        assert_eq!(v["speakers"][1]["name"], "Собеседник");
    }

    #[test]
    fn create_uses_the_effective_root_and_never_creates_a_chosen_one() {
        // Bite: the default root used when a root is chosen; `create_dir_all` for a missing
        // chosen root; no default root when none is chosen.
        let f = fixture();
        let chosen = f.dir.path().join("chosen");
        let calls = calls_in(Some(&chosen), &[]);
        let got = f.store.create(&calls, &moscow_call());
        assert_eq!(
            got.err(),
            Some(CreateError::Root(UnwritableReason::Missing))
        );
        assert!(!chosen.exists(), "a chosen root was created");
        assert!(!f.default_root.exists(), "the default root was touched");

        fs::create_dir(&chosen).expect("chosen");
        let made = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("create: {e:?}"));
        assert_eq!(made.dir(), chosen.join(MOSCOW_ID));
        assert!(!f.default_root.exists(), "the default root was created");

        let calls = calls_in(None, &[]);
        let made = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("default: {e:?}"));
        assert_eq!(made.dir(), f.default_root.join(MOSCOW_ID));
    }

    #[test]
    fn create_runs_the_probe_and_refusal_leaves_nothing() {
        // FR-039: the probe at each start, on the current root, before anything else. Bite:
        // no probe call; a probe on another root; a refusal that still creates a folder,
        // opens a track, or maps to another reason.
        let f = fixture_with(Some(UnwritableReason::AccessDenied));
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        fs::create_dir(root.join("user-folder")).expect("user folder");
        let calls = calls_in(Some(&root), &[]);
        let mut opened = 0;
        let got = f.store.create_with(&calls, &moscow_call(), |path| {
            opened += 1;
            FaultSink::healthy(path)
        });
        assert_eq!(
            got.err(),
            Some(CreateError::Root(UnwritableReason::AccessDenied))
        );
        assert_eq!(opened, 0, "a track was opened after a refused probe");
        assert_eq!(f.roots.probed(), vec![root.clone()]);
        assert_eq!(names(&root), vec!["user-folder".to_string()]);

        // Healthy roots: one probe per start.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        for _ in 0..2 {
            assert!(f.store.create(&calls, &moscow_call()).is_ok());
        }
        assert_eq!(f.roots.probed(), vec![root.clone(), root]);
    }

    #[test]
    fn a_failed_track_write_removes_what_create_made() {
        // The mic header write fails once. Bite: the error swallowed at the origin
        // (`let _ =`), a folder left behind, an error of another kind.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        fs::create_dir(root.join("user-folder")).expect("user folder");
        let calls = calls_in(Some(&root), &[]);
        let got = f.store.create_with(&calls, &moscow_call(), |path| {
            let mut sink = FaultSink::healthy(path)?;
            sink.fail_write = path.ends_with("mic.opus");
            Ok(sink)
        });
        assert_eq!(got.err(), Some(CreateError::Storage(StorageKind::Write)));
        assert_eq!(names(&root), vec!["user-folder".to_string()]);
    }

    #[test]
    fn a_failed_system_track_flush_removes_both_tracks_and_the_folder() {
        // The system header flush fails once, after the mic file was written. Bite: the
        // flush error swallowed, only the failing file removed, the folder kept.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let got = f.store.create_with(&calls, &moscow_call(), |path| {
            let mut sink = FaultSink::healthy(path)?;
            sink.fail_flush = path.ends_with("system.opus");
            Ok(sink)
        });
        assert_eq!(got.err(), Some(CreateError::Storage(StorageKind::Flush)));
        assert_eq!(names(&root), Vec::<String>::new());
    }

    #[test]
    fn a_track_file_that_cannot_be_opened_leaves_nothing() {
        // The opener itself fails once. Bite: the open error swallowed or unwrapped, the
        // meta.json folder left behind.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let got = f.store.create_with(&calls, &moscow_call(), |path| {
            if path.ends_with("system.opus") {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                FaultSink::healthy(path)
            }
        });
        assert!(matches!(got.err(), Some(CreateError::Storage(_))));
        assert_eq!(names(&root), Vec::<String>::new());
    }

    // ---- write_meta ---------------------------------------------------------------

    #[test]
    fn write_meta_replaces_meta_json_and_leaves_no_tmp() {
        // Bite: a tmp left behind, content not replaced, content not the serialized meta.
        let dir = TempDir::new();
        let id = "2026-10-30_101010";
        write_meta(dir.path(), &sample_meta(id, CallState::Recording)).expect("first");
        let mut next = sample_meta(id, CallState::ReadyToTranscribe);
        next.duration_ms = Some(61_000);
        write_meta(dir.path(), &next).expect("second");
        assert_eq!(names(dir.path()), vec!["meta.json".to_string()]);
        let back: CallMeta =
            serde_json::from_slice(&fs::read(dir.path().join("meta.json")).expect("read"))
                .expect("parse");
        assert_eq!(back, next);
    }

    #[test]
    fn write_meta_failure_keeps_previous_meta_json_and_no_tmp() {
        // `meta.json.tmp` is a folder, so writing it fails (a file-shaped fault; the gate
        // runs as root). Bite: an in-place write (succeeds and replaces the file), the error
        // swallowed (Ok), the previous bytes touched.
        let dir = TempDir::new();
        let id = "2026-10-30_101010";
        write_meta(dir.path(), &sample_meta(id, CallState::Recording)).expect("first");
        let before = fs::read(dir.path().join("meta.json")).expect("read");
        fs::create_dir(dir.path().join("meta.json.tmp")).expect("tmp as folder");
        let got = write_meta(dir.path(), &sample_meta(id, CallState::Ready));
        assert!(got.is_err(), "{got:?}");
        assert_eq!(
            fs::read(dir.path().join("meta.json")).expect("read"),
            before
        );
        assert_eq!(
            names(dir.path()),
            vec!["meta.json".to_string(), "meta.json.tmp".to_string()]
        );
        assert!(dir.path().join("meta.json.tmp").is_dir());
    }

    #[test]
    fn write_meta_reports_a_failed_rename() {
        // `meta.json` is a folder, so the tmp is written but the rename fails. Bite: the
        // rename error swallowed at its origin (the write and sync still propagate).
        let dir = TempDir::new();
        fs::create_dir(dir.path().join("meta.json")).expect("meta as folder");
        let got = write_meta(
            dir.path(),
            &sample_meta("2026-10-30_101010", CallState::Recording),
        );
        assert!(got.is_err(), "{got:?}");
        assert!(dir.path().join("meta.json").is_dir());
    }

    #[test]
    fn active_call_write_meta_goes_through_the_atomic_writer() {
        // A state change of a running call replaces meta.json whole. Bite: ActiveCallFiles
        // writing in place (the tmp-folder fault is then ignored).
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let calls = calls_in(Some(&root), &[]);
        let mut made = f
            .store
            .create(&calls, &moscow_call())
            .unwrap_or_else(|e| panic!("create: {e:?}"));
        let mut next = made.meta().clone();
        next.state = CallState::ReadyToTranscribe;
        made.write_meta(&next).expect("write");
        let state = read_json(&made.dir().join("meta.json"))["state"].clone();
        assert_eq!(state, "ready-to-transcribe");
        fs::create_dir(made.dir().join("meta.json.tmp")).expect("fault");
        next.state = CallState::Ready;
        assert!(made.write_meta(&next).is_err());
        let state = read_json(&made.dir().join("meta.json"))["state"].clone();
        assert_eq!(state, "ready-to-transcribe");
    }

    // ---- list ---------------------------------------------------------------------

    #[test]
    fn list_current_and_earlier_newest_first_with_numeric_suffix_order() {
        // Bite: CallId string order (-10 before -2), oldest first, earlier roots dropped,
        // ties not current-root-first, a duplicate id of two roots collapsed.
        let f = fixture();
        let current = f.dir.path().join("current");
        let earlier = f.dir.path().join("earlier");
        fs::create_dir(&current).expect("current");
        fs::create_dir(&earlier).expect("earlier");
        for id in [
            "2026-10-30_101010-2",
            "2026-10-30_101010-10",
            "2026-10-29_090000",
        ] {
            plant(&current, id, CallState::Ready);
        }
        for id in [
            "2026-10-31_080000",
            "2026-10-30_101010",
            "2026-10-30_101010-2",
        ] {
            plant(&earlier, id, CallState::Ready);
        }
        let calls = calls_in(Some(&current), &[&earlier]);
        let list = f.store.list(&calls);
        assert!(list.unavailable.is_empty());
        let got: Vec<(String, PathBuf)> = list
            .entries
            .iter()
            .map(|e| (e.id.to_string(), e.root.clone()))
            .collect();
        let want = vec![
            ("2026-10-31_080000".to_string(), earlier.clone()),
            ("2026-10-30_101010-10".to_string(), current.clone()),
            ("2026-10-30_101010-2".to_string(), current.clone()),
            ("2026-10-30_101010-2".to_string(), earlier.clone()),
            ("2026-10-30_101010".to_string(), earlier.clone()),
            ("2026-10-29_090000".to_string(), current.clone()),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn list_skips_names_that_are_not_a_call_id() {
        // D2. Bite: listing every directory entry, the probe file or a user folder as damaged.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        plant(&root, "2026-10-30_101010", CallState::Ready);
        fs::write(root.join(".voicen-write-probe"), b"").expect("probe");
        fs::create_dir(root.join("my-notes")).expect("user folder");
        fs::write(root.join("stray.txt"), b"x").expect("stray");
        fs::write(root.join("meta.json.tmp"), b"x").expect("tmp");
        fs::create_dir(root.join("2026-13-40_999999")).expect("bad date");
        fs::create_dir(root.join("2026-10-30_101010-")).expect("empty suffix");
        let list = f.store.list(&calls_in(Some(&root), &[]));
        assert_eq!(ids(&list), vec!["2026-10-30_101010".to_string()]);
        assert!(list.unavailable.is_empty());
    }

    #[test]
    fn ready_to_transcribe_lists_as_queued() {
        // Bite: `ReadyToTranscribe` shown as Ready/Recording; any other state mapped wrong.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let table = [
            (
                "2026-10-30_000001",
                CallState::Recording,
                ListState::Recording,
            ),
            (
                "2026-10-30_000002",
                CallState::ReadyToTranscribe,
                ListState::Queued,
            ),
            (
                "2026-10-30_000003",
                CallState::Transcribing,
                ListState::Transcribing,
            ),
            ("2026-10-30_000004", CallState::Ready, ListState::Ready),
            (
                "2026-10-30_000006",
                CallState::Recovered,
                ListState::Recovered,
            ),
        ];
        for (id, state, _) in table {
            plant(&root, id, state);
        }
        let mut failed = meta_value("2026-10-30_000005", CallState::Failed);
        failed["failure"] = serde_json::json!({"reason": "network", "track": "mic"});
        plant_value(&root, "2026-10-30_000005", Some(&failed), true, true);
        let list = f.store.list(&calls_in(Some(&root), &[]));
        for (id, _, want) in table {
            let entry = list.entries.iter().find(|e| e.id.as_str() == id).expect(id);
            assert_eq!(entry.state, want, "{id}");
            assert_eq!(entry.meta.as_ref().map(|m| m.id.as_str()), Some(id));
        }
        let entry = list
            .entries
            .iter()
            .find(|e| e.id.as_str() == "2026-10-30_000005")
            .expect("failed");
        assert_eq!(entry.state, ListState::Failed(FailReason::Network));
    }

    #[test]
    fn missing_default_root_lists_empty_without_unavailable() {
        // D3. Bite: an unavailable line for a default root not created yet; list creating it.
        let f = fixture();
        let list = f.store.list(&calls_in(None, &[]));
        assert!(list.entries.is_empty());
        assert!(list.unavailable.is_empty(), "{:?}", list.unavailable);
        assert!(!f.default_root.exists(), "list created the default root");
    }

    #[test]
    fn missing_chosen_or_earlier_root_gives_one_unavailable_line() {
        // D3. Bite: a missing chosen/earlier root silently skipped; a line per entry; the
        // readable roots hidden because one root failed; an unreadable root (a file) skipped.
        let f = fixture();
        let current = f.dir.path().join("current");
        fs::create_dir(&current).expect("current");
        plant(&current, "2026-10-30_101010", CallState::Ready);
        let gone = f.dir.path().join("gone");
        let list = f.store.list(&calls_in(Some(&current), &[&gone]));
        assert_eq!(list.unavailable, vec![gone.clone()]);
        assert_eq!(ids(&list), vec!["2026-10-30_101010".to_string()]);
        assert!(!gone.exists());

        let list = f.store.list(&calls_in(Some(&gone), &[&current]));
        assert_eq!(list.unavailable, vec![gone.clone()]);
        assert_eq!(ids(&list), vec!["2026-10-30_101010".to_string()]);

        let file_root = f.dir.path().join("a-file");
        fs::write(&file_root, b"x").expect("file");
        let list = f
            .store
            .list(&calls_in(Some(&current), &[&file_root, &gone]));
        assert_eq!(list.unavailable, vec![file_root, gone]);
    }

    #[test]
    fn damaged_for_missing_unparsable_unknown_schema_or_missing_track() {
        // FR-033: never hidden. Bite: a damaged folder skipped, listed as Ready/Recovered,
        // a panic on bad JSON, an unknown schema accepted, a missing track not checked.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        plant(&root, "2026-10-30_000001", CallState::Ready);

        plant_value(&root, "2026-10-30_000002", None, true, true);

        let bad = "2026-10-30_000003";
        plant_value(&root, bad, None, true, true);
        fs::write(root.join(bad).join("meta.json"), b"{ not json").expect("bad json");

        let mut v = meta_value("2026-10-30_000004", CallState::Ready);
        v["schema"] = serde_json::json!(2);
        plant_value(&root, "2026-10-30_000004", Some(&v), true, true);

        let v = meta_value("2026-10-30_000005", CallState::Ready);
        plant_value(&root, "2026-10-30_000005", Some(&v), true, false);
        let v = meta_value("2026-10-30_000006", CallState::Ready);
        plant_value(&root, "2026-10-30_000006", Some(&v), false, true);

        let list = f.store.list(&calls_in(Some(&root), &[]));
        assert_eq!(list.entries.len(), 6);
        for entry in &list.entries {
            let want = if entry.id.as_str() == "2026-10-30_000001" {
                ListState::Ready
            } else {
                ListState::Damaged
            };
            assert_eq!(entry.state, want, "{}", entry.id);
            if want == ListState::Damaged {
                assert!(entry.meta.is_none(), "{}", entry.id);
            }
        }
        assert!(list.unavailable.is_empty());
    }

    #[test]
    fn meta_json_with_unknown_key_lists_as_damaged() {
        // D4. Bite: an unknown key ignored (no deny_unknown_fields).
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let mut v = meta_value("2026-10-30_000001", CallState::Ready);
        v["surprise"] = serde_json::json!(true);
        plant_value(&root, "2026-10-30_000001", Some(&v), true, true);
        let list = f.store.list(&calls_in(Some(&root), &[]));
        assert_eq!(ids(&list), vec!["2026-10-30_000001".to_string()]);
        assert_eq!(list.entries[0].state, ListState::Damaged);
    }

    #[test]
    fn device_cause_and_pass_mode_serialize_as_the_log_literals() {
        // Review 1 #2: one vocabulary with diag/format.rs `call_device cause=` and
        // `final_finished mode=`. Bite: a free string, or another rename rule.
        let causes = [
            (DeviceCause::DefaultOutputChanged, "default_output_changed"),
            (DeviceCause::MicLost, "mic_lost"),
            (DeviceCause::MicDefaultChanged, "mic_default_changed"),
            (DeviceCause::TrackLost, "track_lost"),
            (DeviceCause::Reopened, "reopened"),
            (DeviceCause::NoDevice, "no_device"),
        ];
        for (cause, wire) in causes {
            assert_eq!(serde_json::to_value(cause).expect("cause"), wire);
            assert_eq!(
                serde_json::from_value::<DeviceCause>(Value::from(wire)).expect("parse"),
                cause
            );
        }
        for (mode, wire) in [(PassMode::Timed, "timed"), (PassMode::Timeless, "timeless")] {
            assert_eq!(serde_json::to_value(mode).expect("mode"), wire);
            assert_eq!(
                serde_json::from_value::<PassMode>(Value::from(wire)).expect("parse"),
                mode
            );
        }
        assert_eq!(
            serde_json::to_value(TrackResult::Pending).expect("result"),
            "pending"
        );
    }

    #[test]
    fn meta_json_outside_the_closed_sets_lists_as_damaged() {
        // Review 1 #1/#2: `results.<track>` is `pending` until the final-pass write lands
        // (`done{segments}` / `failed{reason}`, data-model); `cause` and `pass_mode` are
        // closed sets. Bite: a result, cause or pass mode accepted as free text.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let change = |cause: &str| {
            serde_json::json!([{
                "at_ms": 1_000, "track": "system", "cause": cause, "to": null, "gap_ms": 0
            }])
        };

        let mut v = meta_value("2026-10-30_000001", CallState::Ready);
        v["device_changes"] = change("reopened");
        v["pass_mode"] = serde_json::json!("timeless");
        plant_value(&root, "2026-10-30_000001", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000002", CallState::Ready);
        v["results"]["mic"] = serde_json::json!("ready");
        plant_value(&root, "2026-10-30_000002", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000003", CallState::Ready);
        v["results"]["system"] = serde_json::json!("failed");
        plant_value(&root, "2026-10-30_000003", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000004", CallState::Ready);
        v["device_changes"] = change("unplugged");
        plant_value(&root, "2026-10-30_000004", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000005", CallState::Ready);
        v["pass_mode"] = serde_json::json!("fast");
        plant_value(&root, "2026-10-30_000005", Some(&v), true, true);

        let list = f.store.list(&calls_in(Some(&root), &[]));
        let states: Vec<(String, ListState)> = list
            .entries
            .iter()
            .map(|e| (e.id.to_string(), e.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("2026-10-30_000005".to_string(), ListState::Damaged),
                ("2026-10-30_000004".to_string(), ListState::Damaged),
                ("2026-10-30_000003".to_string(), ListState::Damaged),
                ("2026-10-30_000002".to_string(), ListState::Damaged),
                ("2026-10-30_000001".to_string(), ListState::Ready),
            ]
        );
        let meta = list.entries[4].meta.as_ref().expect("meta");
        assert_eq!(meta.device_changes[0].cause, DeviceCause::Reopened);
        assert_eq!(meta.pass_mode, Some(PassMode::Timeless));
    }

    #[test]
    fn meta_json_drops_outside_the_closed_shape_lists_as_damaged() {
        // T-126 (#244): `drops[]` is `{at_ms, track, blocks, ms}` with `deny_unknown_fields`
        // and a closed `track`; a well-formed entry round-trips. Bite: an unknown key, a
        // free-text track, a missing or negative number or a missing `drops` array
        // accepted (a serde default or an untyped value instead of the closed struct).
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let entry = serde_json::json!({"at_ms": 1_500, "track": "mic", "blocks": 3, "ms": 60});

        let mut v = meta_value("2026-10-30_000001", CallState::Ready);
        v["drops"] = serde_json::json!([entry.clone()]);
        plant_value(&root, "2026-10-30_000001", Some(&v), true, true);

        let mut extra = entry.clone();
        extra["text"] = serde_json::json!("hello");
        let mut free_track = entry.clone();
        free_track["track"] = serde_json::json!("both");
        let mut negative = entry.clone();
        negative["blocks"] = serde_json::json!(-1);
        let mut short = entry.clone();
        short.as_object_mut().expect("object").remove("ms");
        for (n, bad) in [extra, free_track, negative, short].into_iter().enumerate() {
            let id = format!("2026-10-30_00001{n}");
            let mut v = meta_value(&id, CallState::Ready);
            v["drops"] = serde_json::json!([bad]);
            plant_value(&root, &id, Some(&v), true, true);
        }
        let id = "2026-10-30_000020";
        let mut v = meta_value(id, CallState::Ready);
        v.as_object_mut().expect("object").remove("drops");
        plant_value(&root, id, Some(&v), true, true);

        let list = f.store.list(&calls_in(Some(&root), &[]));
        let damaged: Vec<String> = list
            .entries
            .iter()
            .filter(|e| e.state == ListState::Damaged)
            .map(|e| e.id.to_string())
            .collect();
        assert_eq!(
            damaged.len(),
            5,
            "every entry outside the shape: {damaged:?}"
        );
        let good = list
            .entries
            .iter()
            .find(|e| e.id.to_string() == "2026-10-30_000001")
            .expect("good entry");
        assert_eq!(good.state, ListState::Ready);
        let drops = &good.meta.as_ref().expect("meta").drops;
        assert_eq!(drops.len(), 1);
        assert_eq!(
            (drops[0].at_ms, drops[0].track, drops[0].blocks, drops[0].ms),
            (1_500, Track::Mic, 3, 60)
        );
    }

    #[test]
    fn damaged_for_a_foreign_id_odd_tracks_a_nested_unknown_key_or_failed_without_failure() {
        // The implementation's further damaged conditions (failure-branch bite). Bite: the
        // meta id not compared with the folder name; track names/kinds not checked; an
        // unknown key inside a track accepted; `failed` without its reason given a default.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        plant(&root, "2026-10-30_000001", CallState::Ready);

        let v = meta_value("2026-10-31_101010", CallState::Ready);
        plant_value(&root, "2026-10-30_000002", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000003", CallState::Ready);
        v["tracks"][0]["file"] = serde_json::json!("other.opus");
        plant_value(&root, "2026-10-30_000003", Some(&v), true, true);
        fs::write(root.join("2026-10-30_000003").join("other.opus"), b"x").expect("other");

        let mut v = meta_value("2026-10-30_000004", CallState::Ready);
        v["tracks"][0]["track"] = serde_json::json!("system");
        v["tracks"][1]["track"] = serde_json::json!("mic");
        plant_value(&root, "2026-10-30_000004", Some(&v), true, true);

        let mut v = meta_value("2026-10-30_000005", CallState::Ready);
        v["tracks"][0]["extra"] = serde_json::json!(1);
        plant_value(&root, "2026-10-30_000005", Some(&v), true, true);

        let v = meta_value("2026-10-30_000006", CallState::Failed);
        assert_eq!(v["failure"], Value::Null);
        plant_value(&root, "2026-10-30_000006", Some(&v), true, true);

        let list = f.store.list(&calls_in(Some(&root), &[]));
        assert_eq!(list.entries.len(), 6);
        for entry in &list.entries {
            if entry.id.as_str() == "2026-10-30_000001" {
                assert_eq!(entry.state, ListState::Ready);
            } else {
                assert_eq!(entry.state, ListState::Damaged, "{}", entry.id);
                assert!(entry.meta.is_none(), "{}", entry.id);
            }
        }
        assert!(list.unavailable.is_empty());
    }

    #[test]
    fn unreadable_default_root_and_missing_earlier_root_give_unavailable_lines() {
        // D3: only a *missing* default current root lists as empty. Bite: any read_dir error
        // of the default root hidden (here it is a file); a missing earlier root hidden when
        // no root is chosen.
        let f = fixture();
        fs::create_dir_all(f.default_root.parent().expect("parent")).expect("data");
        fs::write(&f.default_root, b"x").expect("default root as a file");
        let gone = f.dir.path().join("gone");
        let list = f.store.list(&calls_in(None, &[&gone]));
        assert!(list.entries.is_empty());
        assert_eq!(list.unavailable, vec![f.default_root.clone(), gone]);
    }

    #[test]
    fn create_dir_failure_other_than_a_clash_refuses_with_its_reason() {
        // A probe that passes over a root that is gone by the time the folder is made.
        // Bite: any create_dir error treated as a clash (the suffixes tried, then `other`);
        // the error kind not mapped through reason_of; the root created.
        let dir = TempDir::new();
        let store = CallStore::new(
            Arc::new(crate::settings::roots::FakeCallRoots::new()),
            Arc::new(SystemMonotonic),
        );
        let gone = dir.path().join("gone");
        let got = store.create(&calls_in(Some(&gone), &[]), &moscow_call());
        assert_eq!(
            got.err(),
            Some(CreateError::Root(UnwritableReason::Missing))
        );
        assert!(!gone.exists(), "the root was created");
    }

    #[cfg(unix)]
    #[test]
    fn meta_json_write_failure_inside_create_is_reported_and_leaves_nothing() {
        // Review 1 #3. The root is 4070 bytes: `<root>/<id>` (+18) fits PATH_MAX (4096 with
        // the NUL), so `create_dir` succeeds, while `<root>/<id>/meta.json.tmp` (+32) does
        // not, so the first `meta.json` write fails with ENAMETOOLONG. Bite: that error
        // swallowed or mapped to another kind (the next step, the mic track open, would
        // then fail as `Write`), and the new folder left behind.
        const ROOT_LEN: usize = 4070;
        let dir = TempDir::new();
        let mut root = dir.path().to_path_buf();
        while root.as_os_str().len() < ROOT_LEN {
            let left = ROOT_LEN - root.as_os_str().len();
            root.push("d".repeat((left - 1).min(200)));
        }
        assert_eq!(root.as_os_str().len(), ROOT_LEN);
        fs::create_dir_all(&root).expect("deep root");
        let store = CallStore::new(
            Arc::new(crate::settings::roots::FakeCallRoots::new()),
            Arc::new(SystemMonotonic),
        );
        let got = store.create(&calls_in(Some(&root), &[]), &moscow_call());
        assert_eq!(got.err(), Some(CreateError::Storage(StorageKind::Meta)));
        assert_eq!(names(&root), Vec::<String>::new());
    }

    #[test]
    fn write_meta_failed_rename_removes_its_tmp() {
        // `meta.json` is a folder, so the tmp is written and synced but the rename fails.
        // Bite: the tmp this write created left behind.
        let dir = TempDir::new();
        fs::create_dir(dir.path().join("meta.json")).expect("meta as folder");
        let got = write_meta(
            dir.path(),
            &sample_meta("2026-10-30_101010", CallState::Recording),
        );
        assert!(got.is_err(), "{got:?}");
        assert_eq!(names(dir.path()), vec!["meta.json".to_string()]);
    }

    // ---- storage_kind -------------------------------------------------------------

    #[test]
    fn storage_kind_maps_every_op_encode_to_write() {
        // D1. Bite: Encode left without a mapping to Write, a pair swapped.
        let table = [
            (StorageOp::Write, StorageKind::Write),
            (StorageOp::Flush, StorageKind::Flush),
            (StorageOp::Sync, StorageKind::Sync),
            (StorageOp::Encode, StorageKind::Write),
        ];
        for (op, kind) in table {
            assert_eq!(storage_kind(op), kind, "{op}");
        }
    }

    // ---- T-141: wire, guard, get, export -------------------------------------------

    use crate::call::transcript::{export_json, export_markdown, TranscriptV1};

    const WIRE_FIXTURE: &str = include_str!("../../../../e2e/fixtures/calls-wire.json");
    const READY_ID: &str = "2026-10-29_100000";

    /// `transcript.json` of the ready call: its own title and names differ from meta.json's.
    const TRANSCRIPT_TEXT: &str = concat!(
        r#"{"schema":1,"call_id":"2026-10-29_100000","title":"Own title","#,
        r#""started_at":"2026-10-29T10:00:00+03:00","duration_ms":3600000,"#,
        r#""speakers":[{"id":"me","name":"X"},{"id":"s1","name":"Y"}],"#,
        r#""segments":[{"start_ms":0,"end_ms":2000,"track":"mic","speaker":"me","lang":"en","text":"Hello there","source":"final"},"#,
        r#"{"start_ms":1500,"end_ms":4000,"track":"system","speaker":"s1","lang":null,"text":"Hi","source":"final"}],"#,
        r#""engines":{"transcription":"api:whisper-1","diarization":null}}"#
    );

    fn transcript() -> TranscriptV1 {
        match TranscriptV1::from_json(TRANSCRIPT_TEXT) {
            Ok(t) => t,
            Err(e) => panic!("fixture transcript does not parse: {e:?}"),
        }
    }

    fn not_found() -> CallError {
        CallError {
            code: CallErrorCode::NotFound,
            file: None,
            reason: None,
        }
    }

    fn io_failed(reason: UnwritableReason) -> CallError {
        CallError {
            code: CallErrorCode::IoFailed,
            file: None,
            reason: Some(reason),
        }
    }

    /// A listed entry without touching the disk; `meta.state` follows `state`.
    fn wire_entry(
        root: &str,
        id: &str,
        state: ListState,
        title: &str,
        started_at: &str,
        duration_ms: Option<u64>,
    ) -> CallEntry {
        let stored = match state {
            ListState::Recording => CallState::Recording,
            ListState::Queued => CallState::ReadyToTranscribe,
            ListState::Transcribing => CallState::Transcribing,
            ListState::Ready => CallState::Ready,
            ListState::Failed(_) => CallState::Failed,
            ListState::Recovered | ListState::Damaged => CallState::Recovered,
        };
        let mut meta = sample_meta(id, stored);
        meta.title = title.to_string();
        meta.started_at = started_at.to_string();
        meta.duration_ms = duration_ms;
        if let ListState::Failed(reason) = state {
            meta.failure = Some(Failure {
                reason,
                track: None,
            });
        }
        CallEntry {
            id: CallId::parse(id).expect("call id"),
            root: PathBuf::from(root),
            state,
            meta: (state != ListState::Damaged).then_some(meta),
        }
    }

    fn wire_list() -> CallList {
        let c = "C:\\Calls";
        let mut entries = vec![
            wire_entry(
                c,
                "2026-10-30_140509",
                ListState::Recording,
                "Call 30.10 14:05",
                "2026-10-30T14:05:09+03:00",
                None,
            ),
            wire_entry(
                c,
                "2026-10-30_130000",
                ListState::Queued,
                "Call 30.10 13:00",
                "2026-10-30T13:00:00+03:00",
                Some(1_800_000),
            ),
            wire_entry(
                c,
                "2026-10-30_120000",
                ListState::Transcribing,
                "Call 30.10 12:00",
                "2026-10-30T12:00:00+03:00",
                Some(2_400_000),
            ),
            wire_entry(
                c,
                READY_ID,
                ListState::Ready,
                "Weekly sync",
                "2026-10-29T10:00:00+03:00",
                Some(3_600_000),
            ),
        ];
        let reasons = [
            FailReason::Key,
            FailReason::Network,
            FailReason::Timeout,
            FailReason::Provider,
            FailReason::Storage,
            FailReason::EngineNotSetUp,
        ];
        for (n, reason) in reasons.into_iter().enumerate() {
            entries.push(wire_entry(
                c,
                &format!("2026-10-28_10000{n}"),
                ListState::Failed(reason),
                "Call 28.10 10:00",
                &format!("2026-10-28T10:00:0{n}+03:00"),
                Some(60_000),
            ));
        }
        entries.push(wire_entry(
            c,
            "2026-10-27_090000",
            ListState::Recovered,
            "Call 27.10 09:00",
            "2026-10-27T09:00:00+03:00",
            Some(120_000),
        ));
        entries.push(wire_entry(
            "D:\\Old calls",
            "2026-10-26_081507-2",
            ListState::Damaged,
            "",
            "",
            None,
        ));
        CallList {
            entries,
            unavailable: vec![PathBuf::from("E:\\Gone")],
        }
    }

    fn to_value<T: Serialize>(v: &T) -> Value {
        serde_json::to_value(v).expect("wire value")
    }

    #[test]
    fn calls_wire_fixture_matches_core() {
        // P-010: e2e/fixtures/calls-wire.json is exactly serde_json of the wire builders, every
        // ListState incl. queued, every FailReason, recovered, damaged, an unavailable root, a
        // detail with both tracks and a null lang, the CallError shapes. Bite: a field renamed,
        // a state or reason spelled another way (snake_case for camelCase), `failReason` not
        // skipped, a damaged title/startedAt rule changed, a key added or dropped.
        let fixture: Value = serde_json::from_str(WIRE_FIXTURE)
            .unwrap_or_else(|e| panic!("calls-wire.json is valid JSON: {e}"));
        let list = wire_list();
        let ready = list
            .entries
            .iter()
            .find(|e| e.id.as_str() == READY_ID)
            .expect("ready entry");
        let damaged = list.entries.last().expect("damaged entry");
        let mut errors = serde_json::Map::new();
        errors.insert("notFound".to_string(), to_value(&not_found()));
        errors.insert(
            "inUse".to_string(),
            to_value(&CallError {
                code: CallErrorCode::InUse,
                file: Some("mic.opus".to_string()),
                reason: None,
            }),
        );
        errors.insert(
            "ioFailed".to_string(),
            to_value(&io_failed(UnwritableReason::AccessDenied)),
        );
        let mut core = serde_json::Map::new();
        core.insert(
            "list".to_string(),
            to_value(&list_wire(&list, UiLanguage::En)),
        );
        core.insert(
            "damagedRu".to_string(),
            to_value(&summary_of(damaged, UiLanguage::Ru)),
        );
        core.insert(
            "detail".to_string(),
            to_value(&detail_of(ready, Some(&transcript()), UiLanguage::En)),
        );
        core.insert("errors".to_string(), Value::Object(errors));
        for (key, value) in &core {
            assert_eq!(
                fixture.get(key.as_str()),
                Some(value),
                "e2e/fixtures/calls-wire.json {key} differs from core; core says:\n{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|e| e.to_string())
            );
        }
        let fixture_keys: std::collections::BTreeSet<&str> = fixture
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "_format")
            .collect();
        let core_keys: std::collections::BTreeSet<&str> = core.keys().map(String::as_str).collect();
        assert_eq!(
            fixture_keys, core_keys,
            "the fixture holds exactly core's keys"
        );
        assert!(
            fixture
                .get("_format")
                .and_then(Value::as_str)
                .is_some_and(|f| f.contains("Never edit by hand")),
            "_format names the builders and says never edit by hand"
        );
    }

    #[test]
    fn a_root_that_is_not_current_or_remembered_is_not_found_without_fs() {
        // Acceptance: failure branch, `notFound` with no filesystem access (P-009). A real
        // folder holding a valid call that settings do not list is refused by all three
        // doors; a listed root that does not exist still resolves (no fs check).
        // Bite: a guard that canonicalizes/starts_with/exists instead of exact equality, one
        // that accepts any existing root, a door that skips the guard.
        let f = fixture();
        let listed = f.dir.path().join("listed");
        let stranger = f.dir.path().join("stranger");
        fs::create_dir(&stranger).expect("stranger");
        plant(&stranger, READY_ID, CallState::Ready);
        let calls = calls_in(Some(&listed), &[]);
        let root = wire_root(&stranger);
        assert_eq!(
            resolve_call(&calls, &f.default_root, &root, READY_ID),
            Err(not_found())
        );
        assert_eq!(
            f.store.get(&calls, &root, READY_ID, UiLanguage::En),
            Err(not_found())
        );
        assert_eq!(
            f.store.export_bytes(&calls, &root, READY_ID, Format::Md),
            Err(not_found())
        );
        // The listed root is not on disk: the guard is pure, so it still resolves.
        assert_eq!(
            resolve_call(&calls, &f.default_root, &wire_root(&listed), READY_ID),
            Ok(listed.join(READY_ID))
        );
        // Near misses of the listed root are other texts.
        let text = wire_root(&listed);
        for near in [
            String::new(),
            format!("{text}/"),
            format!("{text}/.."),
            format!("{text}x"),
            text.to_uppercase(),
            wire_root(listed.parent().expect("parent")),
        ] {
            assert_eq!(
                resolve_call(&calls, &f.default_root, &near, READY_ID),
                Err(not_found()),
                "{near:?}"
            );
        }
    }

    #[test]
    fn an_id_outside_the_grammar_is_not_found() {
        // Acceptance: failure branch, an `id` outside the call-id grammar is `notFound`.
        // Bite: the id joined without CallId::parse; a prefix/contains check; `..` allowed.
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        plant(&root, "2026-10-30_140509", CallState::Ready);
        let calls = calls_in(Some(&root), &[]);
        let text = wire_root(&root);
        let bad = [
            "..",
            ".",
            "",
            "a/b",
            "2026-10-30_140509/..",
            "../2026-10-30_140509",
            "2026-10-30_140509\\x",
            "2026-13-01_000000",
            "2026-10-30_140509 ",
            " 2026-10-30_140509",
            "2026-10-30_140509-",
            "2026-10-30_140509-x",
        ];
        for id in bad {
            assert_eq!(
                resolve_call(&calls, &f.default_root, &text, id),
                Err(not_found()),
                "{id:?}"
            );
            assert_eq!(
                f.store.get(&calls, &text, id, UiLanguage::En),
                Err(not_found()),
                "{id:?}"
            );
        }
        for id in ["2026-10-30_140509", "2026-10-30_140509-2"] {
            assert_eq!(
                resolve_call(&calls, &f.default_root, &text, id),
                Ok(root.join(id)),
                "{id}"
            );
        }
    }

    #[test]
    fn guard_accepts_the_current_and_every_earlier_root_and_nothing_else() {
        // Bite: only the current root accepted; only the first earlier root; the default root
        // accepted next to a chosen one; the text compared lossy/case-folded.
        let default = Path::new("/srv/default");
        let (cur, e1, e2) = (
            Path::new("/srv/cur"),
            Path::new("/srv/earlier one"),
            Path::new("/srv/earlier2"),
        );
        assert_eq!(wire_root(Path::new("/srv/earlier one")), "/srv/earlier one");
        let chosen = calls_in(Some(cur), &[e1, e2]);
        for p in [cur, e1, e2] {
            assert_eq!(
                resolve_call(&chosen, default, &wire_root(p), READY_ID),
                Ok(p.join(READY_ID)),
                "{p:?}"
            );
        }
        for other in [default, Path::new("/srv/other")] {
            assert_eq!(
                resolve_call(&chosen, default, &wire_root(other), READY_ID),
                Err(not_found()),
                "{other:?}"
            );
        }
        let unchosen = calls_in(None, &[e1]);
        for p in [default, e1] {
            assert_eq!(
                resolve_call(&unchosen, default, &wire_root(p), READY_ID),
                Ok(p.join(READY_ID)),
                "{p:?}"
            );
        }
        assert_eq!(
            resolve_call(&unchosen, default, &wire_root(cur), READY_ID),
            Err(not_found())
        );
    }

    /// A ready call folder with its transcript; returns the settings and the root.
    fn plant_ready(f: &Fixture) -> (CallSettings, PathBuf) {
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        let mut meta = sample_meta(READY_ID, CallState::Ready);
        meta.title = "Weekly sync".to_string();
        meta.started_at = "2026-10-29T10:00:00+03:00".to_string();
        meta.duration_ms = Some(3_600_000);
        meta.speakers[0].name = "Alice".to_string();
        meta.speakers[1].name = "Bob".to_string();
        plant_value(&root, READY_ID, Some(&to_value(&meta)), true, true);
        fs::write(root.join(READY_ID).join("transcript.json"), TRANSCRIPT_TEXT)
            .expect("transcript");
        (calls_in(Some(&root), &[]), root)
    }

    #[test]
    fn get_maps_meta_and_transcript_to_the_detail() {
        // Bite: title/speakers taken from transcript.json instead of meta.json; a line field
        // dropped, a lang null turned into a string; the summary not from the listed entry.
        let f = fixture();
        let (calls, root) = plant_ready(&f);
        let got = f
            .store
            .get(&calls, &wire_root(&root), READY_ID, UiLanguage::En);
        let want = CallDetail {
            summary: CallSummary {
                id: READY_ID.to_string(),
                root: wire_root(&root),
                title: "Weekly sync".to_string(),
                started_at: "2026-10-29T10:00:00+03:00".to_string(),
                duration_ms: Some(3_600_000),
                state: WireState::Ready,
                fail_reason: None,
            },
            speakers: vec![
                WireSpeaker {
                    id: "me".to_string(),
                    name: "Alice".to_string(),
                },
                WireSpeaker {
                    id: "s1".to_string(),
                    name: "Bob".to_string(),
                },
            ],
            lines: vec![
                Line {
                    start_ms: 0,
                    end_ms: 2000,
                    track: "mic".to_string(),
                    speaker: "me".to_string(),
                    lang: Some("en".to_string()),
                    text: "Hello there".to_string(),
                },
                Line {
                    start_ms: 1500,
                    end_ms: 4000,
                    track: "system".to_string(),
                    speaker: "s1".to_string(),
                    lang: None,
                    text: "Hi".to_string(),
                },
            ],
        };
        assert_eq!(got, Ok(want));
    }

    #[test]
    fn get_without_transcript_has_no_lines() {
        // D3. Bite: a missing transcript.json reported as ioFailed; the state or failReason
        // not carried (queued, failed with a reason).
        let f = fixture();
        let root = f.dir.path().join("calls");
        fs::create_dir(&root).expect("root");
        plant(&root, "2026-10-30_130000", CallState::ReadyToTranscribe);
        let mut failed = sample_meta("2026-10-30_120000", CallState::Failed);
        failed.failure = Some(Failure {
            reason: FailReason::Network,
            track: None,
        });
        plant_value(
            &root,
            "2026-10-30_120000",
            Some(&to_value(&failed)),
            true,
            true,
        );
        let calls = calls_in(Some(&root), &[]);
        let text = wire_root(&root);
        let queued = f
            .store
            .get(&calls, &text, "2026-10-30_130000", UiLanguage::En)
            .expect("queued");
        assert_eq!(queued.lines, Vec::<Line>::new());
        assert_eq!(queued.summary.state, WireState::Queued);
        assert_eq!(queued.summary.fail_reason, None);
        assert_eq!(queued.speakers.len(), 2);
        let failed = f
            .store
            .get(&calls, &text, "2026-10-30_120000", UiLanguage::En)
            .expect("failed");
        assert_eq!(failed.lines, Vec::<Line>::new());
        assert_eq!(failed.summary.state, WireState::Failed);
        assert_eq!(failed.summary.fail_reason, Some(WireFailReason::Network));
    }

    #[test]
    fn get_refuses_a_gone_damaged_or_broken_call_with_the_right_code() {
        // D2, D3. Bite: a damaged call opened as fine; a missing folder reported as ioFailed;
        // a broken or invalid transcript.json shown as no lines.
        let f = fixture();
        let (calls, root) = plant_ready(&f);
        let text = wire_root(&root);
        let code = |r: Result<CallDetail, CallError>| r.expect_err("refused").code;
        // Gone: the folder does not exist.
        assert_eq!(
            code(
                f.store
                    .get(&calls, &text, "2026-10-01_000000", UiLanguage::En)
            ),
            CallErrorCode::NotFound
        );
        // Damaged: the folder exists, meta.json does not.
        plant_value(&root, "2026-10-02_000000", None, true, true);
        assert_eq!(
            code(
                f.store
                    .get(&calls, &text, "2026-10-02_000000", UiLanguage::En)
            ),
            CallErrorCode::IoFailed
        );
        // Unparsable transcript.json.
        let path = root.join(READY_ID).join("transcript.json");
        fs::write(&path, "{").expect("write");
        assert_eq!(
            code(f.store.get(&calls, &text, READY_ID, UiLanguage::En)),
            CallErrorCode::IoFailed
        );
        // Parsable but invalid: a line ends after the call.
        fs::write(
            &path,
            TRANSCRIPT_TEXT.replace("\"end_ms\":4000", "\"end_ms\":4000000000"),
        )
        .expect("write");
        assert_eq!(
            code(f.store.get(&calls, &text, READY_ID, UiLanguage::En)),
            CallErrorCode::IoFailed
        );
    }

    #[test]
    fn export_bytes_serve_markdown_and_json_with_meta_names_and_refuse_the_rest() {
        // FR-036: copy and export share these bytes. Bite: the transcript's own title/names;
        // a second renderer; a missing transcript exported as empty bytes; invalid exported.
        let f = fixture();
        let (calls, root) = plant_ready(&f);
        let text = wire_root(&root);
        let t = transcript();
        let names = [
            Speaker {
                id: "me".to_string(),
                name: "Alice".to_string(),
            },
            Speaker {
                id: "s1".to_string(),
                name: "Bob".to_string(),
            },
        ];
        assert_eq!(
            f.store.export_bytes(&calls, &text, READY_ID, Format::Md),
            Ok(export_markdown(&t, "Weekly sync", &names).into_bytes())
        );
        assert_eq!(
            f.store.export_bytes(&calls, &text, READY_ID, Format::Json),
            Ok(export_json(&t, "Weekly sync", &names).into_bytes())
        );
        // No transcript.json: nothing to export.
        plant(&root, "2026-10-30_130000", CallState::ReadyToTranscribe);
        for format in [Format::Md, Format::Json] {
            assert_eq!(
                f.store
                    .export_bytes(&calls, &text, "2026-10-30_130000", format),
                Err(io_failed(UnwritableReason::Missing))
            );
        }
        fs::write(root.join(READY_ID).join("transcript.json"), "{").expect("write");
        assert_eq!(
            f.store
                .export_bytes(&calls, &text, READY_ID, Format::Json)
                .expect_err("broken")
                .code,
            CallErrorCode::IoFailed
        );
    }

    // ---- export_to / fill_and_rename ----------------------------------------------

    /// A tmp writer over a real file that fails exactly one call (F-024): the first write or
    /// the first sync, once, with `PermissionDenied`; every other call succeeds.
    struct OneFault {
        file: File,
        fail_write: bool,
        fail_sync: bool,
        synced: Arc<std::sync::atomic::AtomicBool>,
    }

    impl OneFault {
        fn open(
            tmp: &Path,
            fail_write: bool,
            fail_sync: bool,
        ) -> (OneFault, Arc<std::sync::atomic::AtomicBool>) {
            let synced = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let file = File::create(tmp).expect("tmp");
            (
                OneFault {
                    file,
                    fail_write,
                    fail_sync,
                    synced: Arc::clone(&synced),
                },
                synced,
            )
        }
    }

    impl Write for OneFault {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.fail_write) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fake write fault",
                ));
            }
            self.file.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl SyncWrite for OneFault {
        fn sync_data(&mut self) -> io::Result<()> {
            if std::mem::take(&mut self.fail_sync) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fake sync fault",
                ));
            }
            self.synced.store(true, std::sync::atomic::Ordering::SeqCst);
            self.file.sync_data()
        }
    }

    #[test]
    fn export_md_and_json_replace_the_target_via_rename_and_leave_no_tmp() {
        // Acceptance: export writes with temp + rename into the target's folder. Bite: the
        // bytes written straight into the target; the target not replaced when it exists; a
        // tmp left behind; the tmp not synced.
        let dir = TempDir::new();
        let md = dir.path().join("call.md");
        let json = dir.path().join("call.json");
        fs::write(&md, "old").expect("old");
        export_to(&md, b"# new\n").expect("md");
        export_to(&json, b"{\"a\":1}").expect("json");
        assert_eq!(fs::read(&md).expect("md"), b"# new\n");
        assert_eq!(fs::read(&json).expect("json"), b"{\"a\":1}");
        assert_eq!(
            names(dir.path()),
            vec!["call.json".to_string(), "call.md".to_string()]
        );
        // The shared tail: healthy run syncs before the rename and replaces the target.
        let tmp = dir.path().join("t.tmp");
        let (writer, synced) = OneFault::open(&tmp, false, false);
        fill_and_rename(writer, &tmp, &md, b"third").expect("tail");
        assert!(
            synced.load(std::sync::atomic::Ordering::SeqCst),
            "the data is synced"
        );
        assert_eq!(fs::read(&md).expect("md"), b"third");
        assert!(!tmp.exists());
    }

    #[test]
    fn export_into_a_missing_or_non_folder_target_leaves_nothing_and_reports_the_reason() {
        // Acceptance: failure branch, a target that cannot be written leaves no file and
        // returns ioFailed{reason}. Real-fs faults that root cannot bypass (chmod does not
        // bite as root). Bite: a missing folder created; `reason` dropped or always `other`;
        // a swallowed create error; a tmp left behind when the rename fails.
        let dir = TempDir::new();
        let before = names(dir.path());
        assert_eq!(
            export_to(&dir.path().join("nope").join("out.md"), b"x"),
            Err(io_failed(UnwritableReason::Missing))
        );
        assert_eq!(names(dir.path()), before, "no folder made");
        let afile = dir.path().join("afile");
        fs::write(&afile, "mine").expect("file");
        assert_eq!(
            export_to(&afile.join("out.md"), b"x"),
            Err(io_failed(UnwritableReason::NotAFolder))
        );
        assert_eq!(fs::read(&afile).expect("file"), b"mine");
        // A target that is a folder fails at the rename and leaves no tmp.
        let adir = dir.path().join("adir");
        fs::create_dir(&adir).expect("dir");
        let got = export_to(&adir, b"x");
        assert_eq!(got.expect_err("a folder").code, CallErrorCode::IoFailed);
        assert_eq!(fs::read_dir(&adir).expect("dir").count(), 0);
        assert_eq!(
            names(dir.path()),
            vec!["adir".to_string(), "afile".to_string()]
        );
        // No parent or no file name: refused as `other`, nothing touched.
        for bad in [Path::new(""), Path::new("/")] {
            assert_eq!(
                export_to(bad, b"x"),
                Err(io_failed(UnwritableReason::Other)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn export_write_or_sync_failure_removes_its_tmp_and_reports_the_reason() {
        // F-024: the fault writer fails exactly one call, the write or the sync, so a swallow
        // at that call cannot hide behind a later error. The previous target survives, the
        // tmp is gone, the first error comes back and maps to access_denied.
        // Bite: `let _ =` on the write or on the sync; the tmp not removed; the rename run
        // after a failed write; the target written in place.
        for (fail_write, fail_sync) in [(true, false), (false, true)] {
            let dir = TempDir::new();
            let target = dir.path().join("out.md");
            let tmp = dir.path().join("out.md.voicen-tmp");
            fs::write(&target, "keep").expect("old");
            let (writer, _) = OneFault::open(&tmp, fail_write, fail_sync);
            let err = fill_and_rename(writer, &tmp, &target, b"new").expect_err("fault");
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(fs::read(&target).expect("target"), b"keep");
            assert_eq!(
                names(dir.path()),
                vec!["out.md".to_string()],
                "write={fail_write}"
            );
            assert_eq!(
                CallError::io_failed(err.kind()),
                io_failed(UnwritableReason::AccessDenied)
            );
        }
    }

    #[test]
    fn export_never_touches_an_existing_tmp_name() {
        // Hypothesis 4: the user's own `<name>.voicen-tmp` in the chosen folder is never
        // opened, truncated, renamed or removed. Bite: File::create instead of create_new
        // (truncates it); a fixed tmp name; removing the tmp name on failure without owning it;
        // overwriting when all nine alternatives are taken.
        let dir = TempDir::new();
        let target = dir.path().join("out.md");
        let mine = dir.path().join("out.md.voicen-tmp");
        fs::write(&mine, "mine").expect("mine");
        export_to(&target, b"data").expect("export");
        assert_eq!(fs::read(&target).expect("target"), b"data");
        assert_eq!(fs::read(&mine).expect("mine"), b"mine");
        assert_eq!(
            names(dir.path()),
            vec!["out.md".to_string(), "out.md.voicen-tmp".to_string()]
        );
        // A failed rename (target is a folder) must not remove the user's tmp either.
        let adir = dir.path().join("adir");
        fs::create_dir(&adir).expect("dir");
        let user = dir.path().join("adir.voicen-tmp");
        fs::write(&user, "user").expect("user");
        assert!(export_to(&adir, b"x").is_err());
        assert_eq!(fs::read(&user).expect("user"), b"user");
        assert_eq!(
            names(dir.path()),
            vec![
                "adir".to_string(),
                "adir.voicen-tmp".to_string(),
                "out.md".to_string(),
                "out.md.voicen-tmp".to_string()
            ]
        );
        // All ten reservable names taken: refused, nothing opened, nothing created.
        let full = TempDir::new();
        let target = full.path().join("out.md");
        let mut taken = vec!["out.md.voicen-tmp".to_string()];
        taken.extend((1..=9).map(|n| format!("out.md.voicen-tmp-{n}")));
        for name in &taken {
            fs::write(full.path().join(name), name).expect("taken");
        }
        let got = export_to(&target, b"data");
        assert_eq!(got.expect_err("all taken").code, CallErrorCode::IoFailed);
        assert!(!target.exists());
        for name in &taken {
            assert_eq!(
                fs::read(full.path().join(name)).expect("taken"),
                name.as_bytes()
            );
        }
        assert_eq!(names(full.path()).len(), taken.len());
    }

    #[test]
    fn write_atomic_still_replaces_a_stale_meta_tmp() {
        // Characterization (passes today): the call folder is ours, so write_atomic opens its
        // tmp with File::create and a tmp left by a crash is replaced, not an error. The split
        // into an opener and the shared tail must keep this. Bite: write_atomic switched to
        // create_new.
        let dir = TempDir::new();
        fs::write(dir.path().join("meta.json"), "old").expect("old");
        fs::write(
            dir.path().join("meta.json.tmp"),
            "stale crash leftover, longer",
        )
        .expect("stale");
        write_atomic(dir.path(), "meta.json", b"fresh").expect("write");
        assert_eq!(
            fs::read(dir.path().join("meta.json")).expect("meta"),
            b"fresh"
        );
        assert_eq!(names(dir.path()), vec!["meta.json".to_string()]);
    }
}
