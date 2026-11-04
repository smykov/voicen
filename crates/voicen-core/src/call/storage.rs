//! Call folders on disk (spec 007 FR-015, FR-033, FR-039; T-125): [`CallStore`] creates
//! `<root>/<CallId>`, writes `meta.json` atomically and lists the calls of every root.
//!
//! RED STAGE: the public surface only; every body is `todo!()` until the developer fills it.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::codec::{OpusOgg, SyncWrite};
use super::events::{CallId, CallState, EndpointId, FailReason, StopReason, StorageKind, Track};
use super::transcript::{EngineRef, Speaker};
use super::StorageOp;
use crate::clock::MonotonicClock;
use crate::i18n::UiLanguage;
use crate::settings::roots::{CallRoots, UnwritableReason};
use crate::settings::CallSettings;

/// The one mapping of a failed track step to the log's storage kind (`Encode` -> `Write`, #242 D1).
pub fn storage_kind(op: StorageOp) -> StorageKind {
    let _ = op;
    todo!("T-125: storage_kind")
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
    pub cause: String,
    pub to: Option<EndpointId>,
    pub gap_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engines {
    pub transcription: Option<EngineRef>,
    pub diarization: Option<EngineRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackResult {
    Pending,
    Ready,
    Failed,
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
    pub engines: Engines,
    pub app_version: String,
    pub speakers: Vec<Speaker>,
    pub results: Results,
    pub pass_mode: Option<String>,
}

/// The one `meta.json` writer: `meta.json.tmp` written and synced, then renamed over `meta.json`.
pub fn write_meta(dir: &Path, meta: &CallMeta) -> io::Result<()> {
    let _ = (dir, meta);
    todo!("T-125: write_meta")
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
    pub fn write_meta(&mut self, meta: &CallMeta) -> io::Result<()> {
        let _ = meta;
        todo!("T-125: ActiveCallFiles::write_meta")
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
    pub fn create_with<W: SyncWrite>(
        &self,
        calls: &CallSettings,
        new: &NewCall,
        open: impl FnMut(&Path) -> io::Result<W>,
    ) -> Result<ActiveCallFiles<W>, CreateError> {
        let _ = (&self.roots, &self.clock, calls, new, open);
        todo!("T-125: CallStore::create_with")
    }

    pub fn list(&self, calls: &CallSettings) -> CallList {
        let _ = (&self.roots, calls);
        todo!("T-125: CallStore::list")
    }
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
}
