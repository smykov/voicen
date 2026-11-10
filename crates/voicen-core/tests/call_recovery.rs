//! T-144 recovery tests: `CallStore::recover` at the start of the app (FR-020 moved to
//! T-007; SC-005, SC-006).
//!
//! The API these tests pin (the developer builds it; it lives in `call/storage.rs`):
//!
//! ```text
//! CallStore::recover(&self, &CallSettings, &Log) -> Recovery
//! Recovery { pub recovered: Vec<CallEntry>, pub queue: Vec<CallEntry> }   // also voicen_core::call::Recovery
//!     // recovered: the calls that were `recording` and not the active call, now `recovered`;
//!     // queue: every `ready-to-transcribe` call (a `transcribing` one is set back to it first),
//!     // oldest first (start stamp, then the numeric id suffix), over the current and every
//!     // earlier root. Logs `call_recovered calls=<recovered.len()>`.
//! CallStore::release(&self, dir: &Path)   // clears the active slot `create` set
//! ```
//!
//! Recovery is read-only for the audio: only `meta.json` changes, through `write_meta`:
//! state `recovered`, `duration_ms` = the shorter of the two tracks (start offset plus the
//! scanned length; an unreadable track counts 0), `ended_at` = `started_at` + duration.
//! A crash is a call whose `ActiveCallFiles` are dropped without `finish`.

mod common;
mod diag_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};
use voicen_core::call::codec::{scan, TrackEncoder};
use voicen_core::call::storage::{write_meta, Failure, META_FILE};
use voicen_core::call::{
    CallMeta, CallState, CallStore, EndpointId, FailReason, ListState, NewCall, Track,
};
use voicen_core::clock::{MonotonicClock, SystemMonotonic};
use voicen_core::diag::{Log, LogConfig};
use voicen_core::i18n::UiLanguage;
use voicen_core::settings::roots::FsCallRoots;
use voicen_core::settings::{defaults, CallSettings};
use voicen_core::test_support::TempDir;

/// A stepping monotonic clock: the encoder syncs by the time `advance` moves it.
struct Step {
    now: Mutex<Instant>,
}

impl Step {
    fn new() -> Arc<Step> {
        Arc::new(Step {
            now: Mutex::new(SystemMonotonic.now()),
        })
    }
    fn advance(&self, d: Duration) {
        *self.now.lock().expect("clock") += d;
    }
}

impl MonotonicClock for Step {
    fn now(&self) -> Instant {
        *self.now.lock().expect("clock")
    }
}

struct Env {
    tmp: TempDir,
    root: PathBuf,
    logs: PathBuf,
    store: CallStore,
    log: Arc<Log>,
    step: Arc<Step>,
}

fn env() -> Env {
    let tmp = TempDir::new();
    let root = tmp.path().join("calls");
    fs::create_dir_all(&root).expect("root");
    let logs = tmp.path().join("logs");
    fs::create_dir_all(&logs).expect("logs");
    let store = CallStore::new(
        Arc::new(FsCallRoots::new(tmp.path().join("data").join("calls"))),
        Arc::new(SystemMonotonic),
    );
    let (log, _, _) = diag_support::open_log(
        &logs,
        diag_support::at(diag_support::NOON_UTC, 0),
        0,
        LogConfig::default(),
    );
    Env {
        tmp,
        root,
        logs,
        store,
        log,
        step: Step::new(),
    }
}

fn calls_in(root: &Path, earlier: &[&Path]) -> CallSettings {
    let mut calls = defaults(None).calls;
    calls.storage_root = Some(root.to_path_buf());
    calls.earlier_roots = earlier.iter().map(|p| p.to_path_buf()).collect();
    calls
}

/// 2026-10-30 11:05:09 UTC plus `plus` seconds.
fn stamp(plus: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_793_358_309 + plus)
}

fn new_call(started: SystemTime) -> NewCall {
    NewCall {
        started,
        seconds_east: 10_800,
        mic: EndpointId::new("{0.0.1.00000000}.{aaaa-0001}"),
        system: EndpointId::new("{0.0.0.00000000}.{bbbb-0002}"),
        lang: UiLanguage::En,
        app_version: "9.9.9".to_string(),
    }
}

fn tone(seconds: usize) -> Vec<f32> {
    (0..16_000 * seconds)
        .map(|i| (i as f32 * 0.05).sin() * 0.3)
        .collect()
}

/// A call folder under `root` whose two tracks hold `mic_s` / `system_s` seconds, then
/// "crashed": the encoders are dropped without `finish`, the call is released, and
/// `meta.json` stays `recording`.
fn crashed_call(
    e: &Env,
    root: &Path,
    started: SystemTime,
    mic_s: usize,
    system_s: usize,
) -> PathBuf {
    let calls = calls_in(root, &[]);
    let mut files = e.store.create(&calls, &new_call(started)).expect("create");
    let dir = files.dir().to_path_buf();
    let chunk = tone(1);
    for sec in 0..mic_s.max(system_s) {
        e.step.advance(Duration::from_secs(1));
        if sec < mic_s {
            files.mic.push(&chunk).expect("mic push");
        }
        if sec < system_s {
            files.system.push(&chunk).expect("system push");
        }
    }
    drop(files);
    e.store.release(&dir);
    dir
}

fn meta_of(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join(META_FILE)).expect("meta.json")).expect("json")
}

fn set_state(dir: &Path, state: CallState) {
    let mut meta: CallMeta =
        serde_json::from_slice(&fs::read(dir.join(META_FILE)).expect("meta")).expect("meta");
    meta.state = state;
    write_meta(dir, &meta).expect("write_meta");
}

/// A `failed` call as a final pass leaves it: the state with its `failure` reason.
fn set_failed(dir: &Path, reason: FailReason, track: Option<Track>) {
    let mut meta: CallMeta =
        serde_json::from_slice(&fs::read(dir.join(META_FILE)).expect("meta")).expect("meta");
    meta.state = CallState::Failed;
    meta.failure = Some(Failure { reason, track });
    write_meta(dir, &meta).expect("write_meta");
}

fn sha(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).expect("file")).to_vec()
}

fn secs_of(stamp: &str) -> i64 {
    // `YYYY-MM-DDTHH:MM:SS±HH:MM` -> seconds since 1970 of the local wall time.
    let n = |a: usize, b: usize| stamp[a..b].parse::<i64>().expect("digits");
    let (y, m, d) = (n(0, 4), n(5, 7), n(8, 10));
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + n(11, 13) * 3_600 + n(14, 16) * 60 + n(17, 19)
}

fn recovered_lines(e: &Env) -> Vec<String> {
    diag_support::all_lines(&e.logs)
        .into_iter()
        .map(|raw| diag_support::closed(&raw))
        .filter(|l| l.head == "call_recovered")
        .map(|l| l.get("calls").expect("calls").to_string())
        .collect()
}

#[test]
fn recover_marks_an_unowned_recording_recovered_from_the_shorter_track_and_leaves_opus_bytes_unchanged(
) {
    // Acceptance 1 (failure branch) + I-a: mic about 6 s and system about 9 s, both cut
    // mid-file, the mic starting 1.5 s late. The call becomes `recovered`, its duration is
    // the shorter track (offset + scanned length), `ended_at` = `started_at` + duration,
    // the other meta fields are unchanged, both `.opus` files are byte for byte the same
    // (SHA-256), `call_recovered calls=1` is logged and the call is not queued. Bite: the
    // longer track, no offset, state left `recording`, `ended_at` unset, an audio file
    // rewritten or truncated, nothing logged, the recovered call queued.
    let e = env();
    let dir = crashed_call(&e, &e.root, stamp(0), 6, 9);
    // The mic joined 1.5 s after the start.
    let mut meta: CallMeta =
        serde_json::from_slice(&fs::read(dir.join(META_FILE)).expect("meta")).expect("meta");
    meta.tracks[0].start_offset_ms = 1_500;
    write_meta(&dir, &meta).expect("write_meta");
    let before = meta_of(&dir);
    assert_eq!(before["state"], "recording");
    let hashes = [sha(&dir.join("mic.opus")), sha(&dir.join("system.opus"))];
    let mic = scan(fs::File::open(dir.join("mic.opus")).expect("mic")).expect("scan");
    let sys = scan(fs::File::open(dir.join("system.opus")).expect("system")).expect("scan");
    let want_mic = 1_500 + mic.duration_48k / 48;
    let want_sys = sys.duration_48k / 48;
    let want = want_mic.min(want_sys);
    assert!(
        want_mic < want_sys,
        "premise: the mic is the shorter: {want_mic} {want_sys}"
    );
    assert!(want >= 5_000, "premise: the tracks hold audio: {want}");

    let calls = calls_in(&e.root, &[]);
    let out = e.store.recover(&calls, &e.log);
    assert_eq!(out.recovered.len(), 1);
    assert_eq!(out.recovered[0].state, ListState::Recovered);
    assert_eq!(out.recovered[0].root, e.root);
    assert!(out.queue.is_empty(), "a recovered call is not queued");

    let after = meta_of(&dir);
    assert_eq!(after["state"], "recovered");
    let duration = after["duration_ms"].as_u64().expect("duration");
    assert!(duration.abs_diff(want) <= 1, "{duration} vs {want}");
    let span = secs_of(after["ended_at"].as_str().expect("ended_at"))
        - secs_of(after["started_at"].as_str().expect("started_at"));
    assert!(
        (span - i64::try_from(duration).expect("fits") / 1_000).abs() <= 1,
        "ended_at follows the duration: {span} s for {duration} ms"
    );
    for key in [
        "id",
        "title",
        "started_at",
        "tracks",
        "mode",
        "engines",
        "schema",
    ] {
        assert_eq!(after[key], before[key], "{key} is unchanged");
    }
    assert_eq!(hashes[0], sha(&dir.join("mic.opus")), "mic.opus bytes");
    assert_eq!(
        hashes[1],
        sha(&dir.join("system.opus")),
        "system.opus bytes"
    );
    assert!(!dir.join("meta.json.tmp").exists());
    assert_eq!(recovered_lines(&e), vec!["1".to_string()]);

    let again = e.store.recover(&calls, &e.log);
    assert!(again.recovered.is_empty(), "recovered once");
    assert!(again.queue.is_empty());
}

#[test]
fn a_crash_at_minute_20_recovers_at_least_19_min_50_s() {
    // Acceptance 2 (SC-005): 20 minutes recorded on both tracks (the clock moves 1 s per
    // second of audio), the process dies, the next start recovers at least 19:50.
    // Bite: a duration read from the last page only, a truncated scan, the shorter track
    // measured in frames of another rate.
    let e = env();
    let dir = crashed_call(&e, &e.root, stamp(0), 1_200, 1_200);
    let out = e.store.recover(&calls_in(&e.root, &[]), &e.log);
    assert_eq!(out.recovered.len(), 1);
    let duration = meta_of(&dir)["duration_ms"].as_u64().expect("duration");
    assert!(duration >= 1_190_000, "recovered {duration} ms");
    assert!(duration <= 1_200_100, "not more than recorded: {duration}");
}

#[test]
fn recover_requeues_transcribing_and_queues_ready_to_transcribe_oldest_first() {
    // Acceptance 3: `transcribing` becomes `ready-to-transcribe`; every
    // `ready-to-transcribe` call (current and earlier roots) is queued oldest first, ties
    // of one start second by the numeric id suffix (-2 before -10). A call that was
    // already `recovered` is not queued. Bite: a lexical suffix order (-10 before -2),
    // newest first, only the current root, `transcribing` left as it is or not queued,
    // `ready` / `failed` queued.
    let e = env();
    let earlier = e.tmp.path().join("earlier");
    fs::create_dir_all(&earlier).expect("earlier");
    let hour_ago = crashed_call(&e, &earlier, stamp(0), 1, 1);
    // 12 calls in one start second: suffixes none, -1 ... -11, so the numeric order
    // (-2 before -10, -11) differs from the text order (-10, -11 before -2). Validation
    // round 1, mutant 43: with 10 calls the suffixes stopped at -9 and a text sort passed.
    let mut same_second = Vec::new();
    for _ in 0..12 {
        same_second.push(crashed_call(&e, &e.root, stamp(3_600), 1, 1));
    }
    let transcribing = crashed_call(&e, &e.root, stamp(7_200), 1, 1);
    let ready = crashed_call(&e, &e.root, stamp(7_300), 1, 1);
    let recovered = crashed_call(&e, &e.root, stamp(7_400), 1, 1);
    let failed = crashed_call(&e, &e.root, stamp(7_500), 1, 1);
    set_state(&hour_ago, CallState::ReadyToTranscribe);
    for dir in &same_second {
        set_state(dir, CallState::ReadyToTranscribe);
    }
    set_state(&transcribing, CallState::Transcribing);
    set_state(&ready, CallState::Ready);
    set_state(&recovered, CallState::Recovered);
    set_failed(&failed, FailReason::Network, Some(Track::Mic));
    let failed_before = fs::read(failed.join(META_FILE)).expect("meta");

    let out = e.store.recover(&calls_in(&e.root, &[&earlier]), &e.log);
    let ids: Vec<String> = out.queue.iter().map(|c| c.id.to_string()).collect();
    let name = |d: &PathBuf| d.file_name().expect("name").to_string_lossy().into_owned();
    assert!(
        same_second.iter().map(name).any(|n| n.ends_with("-10")),
        "the same-second calls reach a two-digit suffix: {:?}",
        same_second.iter().map(name).collect::<Vec<_>>()
    );
    let mut want = vec![name(&hour_ago)];
    want.extend(same_second.iter().map(name));
    want.push(name(&transcribing));
    assert_eq!(ids, want);
    assert!(
        out.queue.iter().all(|c| c.state == ListState::Queued),
        "{:?}",
        out.queue
    );
    assert_eq!(meta_of(&transcribing)["state"], "ready-to-transcribe");
    assert_eq!(meta_of(&ready)["state"], "ready");
    assert_eq!(meta_of(&recovered)["state"], "recovered");
    assert_eq!(
        fs::read(failed.join(META_FILE)).expect("meta"),
        failed_before,
        "a `failed` call is left as it is, its failure reason included"
    );
    assert_eq!(meta_of(&failed)["state"], "failed");
    assert!(out.recovered.is_empty(), "no `recording` call was found");
    assert_eq!(recovered_lines(&e), vec!["0".to_string()]);
}

#[test]
fn recover_skips_the_active_call_and_only_the_active_call_lists_as_recording() {
    // I-a, I-b: the call `create` made is the store's active call: `recover` leaves it
    // `recording`, `list` shows it as Recording; after `release` it is a crashed call and
    // both treat it so. A planted (unowned) `recording` folder lists as Recovered.
    // Bite: no ownership check (the live call marked recovered), the slot never cleared,
    // `list` mapping every `recording` to Recording.
    let e = env();
    let calls = calls_in(&e.root, &[]);
    let files = e.store.create(&calls, &new_call(stamp(0))).expect("create");
    let dir = files.dir().to_path_buf();
    let other = crashed_call(&e, &e.root, stamp(10), 1, 1);
    let before = fs::read(dir.join(META_FILE)).expect("meta");

    let listed = e.store.list(&calls);
    let state_of = |d: &Path| {
        listed
            .entries
            .iter()
            .find(|c| c.id.as_str() == d.file_name().expect("n").to_string_lossy())
            .expect("listed")
            .state
    };
    assert_eq!(state_of(&dir), ListState::Recording);
    assert_eq!(state_of(&other), ListState::Recovered);

    let out = e.store.recover(&calls, &e.log);
    assert_eq!(out.recovered.len(), 1, "only the unowned call");
    assert_eq!(fs::read(dir.join(META_FILE)).expect("meta"), before);
    assert_eq!(meta_of(&other)["state"], "recovered");

    drop(files);
    e.store.release(&dir);
    let after = e.store.recover(&calls, &e.log);
    assert_eq!(after.recovered.len(), 1, "released: now a crashed call");
    assert_eq!(meta_of(&dir)["state"], "recovered");
}

#[test]
fn recover_under_faults_never_panics_and_never_changes_audio() {
    // Acceptance 8 (SC-006): an unreadable track counts 0 (duration 0, the call is still
    // `recovered`); a `meta.json` that cannot be rewritten (its tmp path is a folder)
    // leaves the folder untouched and uncounted; a missing earlier root is skipped; no
    // `.opus` byte changes anywhere. Bite: a panic on a bad track, the failed write
    // counted or unwinding the others, the audio rewritten, the whole recovery aborted by
    // one bad folder or root.
    let e = env();
    let garbled = crashed_call(&e, &e.root, stamp(0), 3, 3);
    fs::write(garbled.join("mic.opus"), b"not an ogg stream at all").expect("garble");
    let blocked = crashed_call(&e, &e.root, stamp(100), 3, 3);
    fs::create_dir(blocked.join("meta.json.tmp")).expect("tmp as a folder");
    let fine = crashed_call(&e, &e.root, stamp(200), 3, 3);
    let hashes: Vec<Vec<u8>> = [&garbled, &blocked, &fine]
        .iter()
        .flat_map(|d| [sha(&d.join("mic.opus")), sha(&d.join("system.opus"))])
        .collect();
    let blocked_meta = fs::read(blocked.join(META_FILE)).expect("meta");
    let missing = e.tmp.path().join("gone");

    let out = e.store.recover(&calls_in(&e.root, &[&missing]), &e.log);
    assert_eq!(out.recovered.len(), 2, "{:?}", out.recovered);
    let garbled_meta = meta_of(&garbled);
    assert_eq!(garbled_meta["state"], "recovered");
    assert_eq!(garbled_meta["duration_ms"], 0);
    assert_eq!(meta_of(&fine)["state"], "recovered");
    assert_eq!(
        fs::read(blocked.join(META_FILE)).expect("meta"),
        blocked_meta
    );
    let after: Vec<Vec<u8>> = [&garbled, &blocked, &fine]
        .iter()
        .flat_map(|d| [sha(&d.join("mic.opus")), sha(&d.join("system.opus"))])
        .collect();
    assert_eq!(hashes, after, "no audio byte changed");
    assert_eq!(recovered_lines(&e), vec!["2".to_string()]);
}
