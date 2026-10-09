//! T-008: the log writer `diag::Log` on a temp dir with a fake clock (analysis
//! approach 5, 6 and 7).
//!
//! - Concurrency: 8 threads x 500 writes give exactly 4000 whole lines.
//! - Size: after EVERY write the active file is `<= roll_bytes` and the whole set
//!   (`voicen.log` + `voicen-*.log`) is `<= total_bytes`, with a small config and
//!   with the real 2 MiB / 10 MiB one. This bites the spec 006 data-model design
//!   that retains rolled files up to 10 MB and then lets the active file grow to
//!   2 MB (12 MB on disk; T-008 Investigation "Spec gap found").
//! - Date roll at the local midnight of a non-zero UTC offset; age retention at open
//!   and after a roll; foreign files in `logs/` survive; a clock one year off.
//! - Degraded mode: a file at the logs path (`NotADirectory`), `voicen.log` linked
//!   to `/dev/full` (`DiskFull`; the core container runs as uid 0, so mode bits
//!   cannot make a folder unwritable, T-008 Investigation), one callback over 100
//!   writes, a reopen at most every `reopen_every`, `LogsRecovered` after it.
//!
//! Each line is identified by its `rec=` number, so a lost, duplicated or cut line
//! is visible.

mod diag_support;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use diag_support::{
    active_len, active_lines, all_lines, at, closed, dictation_lines, open_log, rolled_names,
    set_len, ACTIVE, NOON_UTC,
};
use voicen_core::clock::utc_compact;
use voicen_core::diag::{
    format_line, DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, LogConfig,
    LogEvent, LogsUnwritableReason,
};
use voicen_core::test_support::TempDir;

const DAY: u64 = 86_400;
const MIB: u64 = 1024 * 1024;

/// A short line identified by `rec`.
fn short(rec: u64) -> LogEvent {
    LogEvent::Dictation(DictationLine {
        recording: rec,
        engine: None,
        outcome: DictationOutcome::TooShort,
        detector: None,
        press_to_frame_ms: None,
        duration_ms: Some(120),
        stop_to_text_ms: None,
        text_to_paste_ms: None,
        post_processing: None,
    })
}

/// A long line (every key, the largest values), identified by `rec` (about 300
/// bytes).
fn long(rec: u64) -> LogEvent {
    LogEvent::Dictation(DictationLine {
        recording: rec,
        engine: Some(EngineTag::LocalServer),
        outcome: DictationOutcome::Failed {
            failure: FailureTag::KeyStoreUnavailable,
            http_status: Some(u16::MAX),
        },
        detector: Some(DetectorTag::Energy),
        press_to_frame_ms: Some(u64::MAX),
        duration_ms: Some(u64::MAX),
        stop_to_text_ms: Some(u64::MAX),
        text_to_paste_ms: Some(u64::MAX),
        post_processing: None,
    })
}

/// The `rec=` numbers of `lines`, each line checked against the closed output.
#[track_caller]
fn recs(lines: &[String]) -> Vec<u64> {
    dictation_lines(lines)
        .iter()
        .map(|l| {
            l.get("rec")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("rec in {:?}", l.raw))
        })
        .collect()
}

fn small(roll_bytes: u64, total_bytes: u64) -> LogConfig {
    LogConfig {
        roll_bytes,
        total_bytes,
        max_age: Duration::from_secs(7 * DAY),
        reopen_every: Duration::from_secs(60),
    }
}

/// Writes `bytes` to `path` and sets its modification time to `t`.
fn plant(path: &Path, bytes: &[u8], t: SystemTime) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .unwrap_or_else(|e| panic!("set_modified {}: {e}", path.display()));
}

/// A rolled file as the log names it (`voicen-YYYYMMDD-HHMMSS.log`, offset 0),
/// last written at `t`; returns its name.
fn plant_rolled(dir: &Path, t: SystemTime, rec: u64) -> String {
    let name = format!("voicen-{}.log", utc_compact(t));
    let line = format!(
        "{} INFO dictation rec={rec} outcome=too_short duration_ms=120\n",
        "2026-01-01T00:00:00.000+00:00"
    );
    plant(&dir.join(&name), line.as_bytes(), t);
    name
}

/// Files that are not the log's: crash files, the session marker, the installer
/// note, and look-alikes that are not `voicen-*.log`. Returns (name, bytes).
fn plant_foreign(dir: &Path, t: SystemTime) -> Vec<(String, Vec<u8>)> {
    let files: Vec<(String, Vec<u8>)> = vec![
        (
            "crash-20260914-120000-4242.txt".to_string(),
            b"kind: panic\n".to_vec(),
        ),
        ("session.marker".to_string(), b"pid: 4242\n".to_vec()),
        (
            "installer-ended".to_string(),
            b"2026-09-01T00:00:00+00:00\n".to_vec(),
        ),
        ("notes.txt".to_string(), b"owner notes\n".to_vec()),
        ("voicen.log.bak".to_string(), b"old copy\n".to_vec()),
        ("voicen.txt".to_string(), b"not a log\n".to_vec()),
    ];
    for (name, bytes) in &files {
        // 20 days old: older than the log's max age, younger than the crash files' 30.
        plant(&dir.join(name), bytes, t - Duration::from_secs(20 * DAY));
    }
    files
}

#[track_caller]
fn assert_foreign_intact(dir: &Path, files: &[(String, Vec<u8>)]) {
    for (name, bytes) in files {
        let got = std::fs::read(dir.join(name))
            .unwrap_or_else(|e| panic!("foreign file {name} was removed: {e}"));
        assert_eq!(&got, bytes, "foreign file {name} was changed");
    }
}

// ---- config ------------------------------------------------------------------------

#[test]
fn log_config_defaults_are_fr20() {
    // FR-20: 7 days, 10 MB, roll at 2 MB; retry an unwritable log every 60 s
    // (spec 006 FR-008/FR-009). Bite: another constant.
    assert_eq!(
        LogConfig::default(),
        LogConfig {
            roll_bytes: 2 * MIB,
            total_bytes: 10 * MIB,
            max_age: Duration::from_secs(7 * DAY),
            reopen_every: Duration::from_secs(60),
        }
    );
}

// ---- writing -----------------------------------------------------------------------

#[test]
fn open_creates_the_dir_writes_nothing_itself_and_appends_whole_lines() {
    // The log creates a missing logs dir (spec 006 T008) and writes nothing by
    // itself, so the shell's Started is the first line of a session. Bite: no
    // create_dir_all, a line written at open (a retention or header line), lines
    // reordered or without their '\n'.
    let tmp = TempDir::new();
    let dir = tmp.path().join("Voicen").join("logs");
    let (log, clock, seen) = open_log(&dir, at(NOON_UTC, 0), 0, LogConfig::default());
    assert!(dir.is_dir(), "open creates {}", dir.display());
    assert_eq!(all_lines(&dir), Vec::<String>::new(), "open wrote a line");

    log.write(LogEvent::Started {
        build: voicen_core::build_info(),
        pid: 4242,
    });
    clock.set(at(NOON_UTC, 250));
    log.write(short(1));
    log.write(LogEvent::LogsRecovered);

    let lines = active_lines(&dir);
    assert_eq!(lines.len(), 3, "{lines:#?}");
    let heads: Vec<String> = lines.iter().map(|l| closed(l).head).collect();
    assert_eq!(heads, ["started", "dictation", "logs recovered"]);
    assert!(
        lines[0].contains(&format!("voicen {} started", voicen_core::build_info())),
        "{lines:#?}"
    );
    assert_eq!(seen.count(), 0);
}

#[test]
fn eight_threads_write_4000_whole_lines() {
    // FR-004 / spec 006 T008: lines written concurrently never interleave or get
    // cut. Bite: the line formatted and written in two writes outside one lock, a
    // shared buffer flushed by two threads, a line lost or written twice.
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&dir, at(NOON_UTC, 0), 0, LogConfig::default());
    std::thread::scope(|s| {
        for t in 0..8u64 {
            let log = Arc::clone(&log);
            s.spawn(move || {
                for i in 0..500u64 {
                    log.write(long(t * 1_000 + i));
                }
            });
        }
    });
    let lines = active_lines(&dir);
    assert_eq!(lines.len(), 4_000, "whole lines in voicen.log");
    let got: BTreeSet<u64> = recs(&lines).into_iter().collect();
    let want: BTreeSet<u64> = (0..8u64)
        .flat_map(|t| (0..500u64).map(move |i| t * 1_000 + i))
        .collect();
    assert_eq!(got, want, "every line exactly once");
    assert_eq!(seen.count(), 0);
}

// ---- size ----------------------------------------------------------------------------

#[test]
fn size_bound_holds_after_every_write_with_a_small_config() {
    // The invariant: active <= roll_bytes and the whole set <= total_bytes at
    // every instant, so rolled files may total at most total - roll. Bite: the
    // spec's retention "delete oldest while voicen*.log > total" after a roll
    // (the active file then grows to roll on top: total + roll on disk), a roll
    // after the line instead of before it, retention deleting a foreign file.
    // Positive controls: rolls happened, retention keeps the newest lines and
    // drops only the oldest (no gap), and it does not over-delete (the rolled
    // files keep at least total - 2 * roll).
    let (roll, total) = (1_024, 5_120);
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    std::fs::create_dir_all(&dir).expect("logs dir");
    let foreign = plant_foreign(&dir, at(NOON_UTC, 0));
    let (log, clock, seen) = open_log(&dir, at(NOON_UTC, 0), 0, small(roll, total));
    let writes = 600u64;
    for i in 0..writes {
        clock.set(at(NOON_UTC + i, 0));
        log.write(long(i));
        let (active, set) = (active_len(&dir), set_len(&dir));
        assert!(active <= roll, "write {i}: voicen.log is {active} > {roll}");
        assert!(
            set <= total,
            "write {i}: the log files total {set} > {total}"
        );
    }
    assert!(
        rolled_names(&dir).len() >= 2,
        "rolls happened: {:?}",
        rolled_names(&dir)
    );
    let rolled = set_len(&dir) - active_len(&dir);
    assert!(
        rolled >= total - 2 * roll,
        "retention over-deleted: rolled files total {rolled}"
    );
    let mut got = recs(&all_lines(&dir));
    got.sort_unstable();
    let first = *got.first().expect("some lines kept");
    let want: Vec<u64> = (first..writes).collect();
    assert_eq!(got, want, "the newest lines, contiguous, each once");
    assert!(first > 0, "the oldest lines were dropped");
    assert_foreign_intact(&dir, &foreign);
    assert_eq!(seen.count(), 0);
}

#[test]
fn size_bound_holds_after_every_write_with_the_real_config() {
    // The same invariant with LogConfig::default() (2 MiB / 10 MiB), over more
    // than 14 MiB of lines, so retention runs several times. Bite: as above; the
    // 12 MB design fails here once 10 MiB of rolled files are kept and the active
    // file gets its first line.
    let cfg = LogConfig::default();
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, clock, seen) = open_log(&dir, at(NOON_UTC, 0), 0, cfg);
    let mut written = 0u64;
    let mut i = 0u64;
    while written < 14 * MIB {
        let t = at(NOON_UTC, i);
        clock.set(t);
        written += format_line(t, 0, &long(i)).len() as u64 + 1;
        log.write(long(i));
        let (active, set) = (active_len(&dir), set_len(&dir));
        assert!(
            active <= cfg.roll_bytes,
            "write {i}: voicen.log is {active} > {}",
            cfg.roll_bytes
        );
        assert!(
            set <= cfg.total_bytes,
            "write {i}: the log files total {set} > {}",
            cfg.total_bytes
        );
        i += 1;
        assert!(i < 1_000_000, "the log does not grow");
    }
    assert!(rolled_names(&dir).len() >= 3, "{:?}", rolled_names(&dir));
    assert!(
        set_len(&dir) > cfg.total_bytes - 2 * cfg.roll_bytes,
        "retention over-deleted: {}",
        set_len(&dir)
    );
    assert_eq!(seen.count(), 0);
}

#[test]
fn rolls_in_the_same_second_lose_no_line() {
    // Several rolls within one clock second (the rolled name has second
    // resolution) must not overwrite an earlier rolled file or fail the roll
    // (Windows rename does not replace). Bite: rename over an existing
    // voicen-<same second>.log, a failed rename leaving voicen.log over its limit.
    let (roll, total) = (400, 100_000);
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&dir, at(NOON_UTC, 0), 0, small(roll, total));
    for i in 0..40u64 {
        log.write(short(i));
        assert!(active_len(&dir) <= roll, "write {i}");
    }
    assert!(rolled_names(&dir).len() >= 3, "{:?}", rolled_names(&dir));
    let mut got = recs(&all_lines(&dir));
    got.sort_unstable();
    assert_eq!(got, (0..40u64).collect::<Vec<_>>(), "every line once");
    assert_eq!(seen.count(), 0);
}

// ---- date roll -------------------------------------------------------------------------

#[test]
fn date_roll_follows_local_midnight_at_an_offset() {
    // Spec 006 FR-008: the active file is rolled when the LOCAL date changes. At
    // UTC+02:00 local midnight is 22:00 UTC. Bite: the UTC date used (no roll at
    // 22:00:30 UTC, a roll at 00:00:30 UTC), the offset applied with the wrong
    // sign, a roll on every write.
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    // 2026-10-04T21:59:00Z = 23:59:00 local.
    let (log, clock, seen) = open_log(&dir, at(1_791_151_140, 0), 7_200, LogConfig::default());
    log.write(short(1));
    assert_eq!(rolled_names(&dir), Vec::<String>::new());

    // 22:00:30Z = 00:00:30 local on 2026-10-05.
    clock.set(at(1_791_151_230, 0));
    log.write(short(2));
    let rolled = rolled_names(&dir);
    assert_eq!(rolled.len(), 1, "one roll at local midnight: {rolled:?}");
    let name = &rolled[0];
    let stem = name
        .strip_prefix("voicen-")
        .and_then(|n| n.strip_suffix(".log"))
        .unwrap_or_default();
    let ok = stem.len() >= 15
        && stem.as_bytes()[..8].iter().all(u8::is_ascii_digit)
        && stem.as_bytes()[8] == b'-'
        && stem.as_bytes()[9..15].iter().all(u8::is_ascii_digit);
    assert!(
        ok,
        "rolled name {name:?} is not voicen-YYYYMMDD-HHMMSS[...].log"
    );
    assert_eq!(recs(&diag_support::lines_of(&dir.join(name))), vec![1]);
    assert_eq!(recs(&active_lines(&dir)), vec![2]);

    // 23:59:59Z = 01:59:59 local, same local date: no roll.
    clock.set(at(1_791_158_399, 0));
    log.write(short(3));
    // 2026-10-05T00:00:30Z: the UTC date changes, the local one (02:00:30) does not.
    clock.set(at(1_791_158_430, 0));
    log.write(short(4));
    assert_eq!(rolled_names(&dir).len(), 1, "{:?}", rolled_names(&dir));
    assert_eq!(recs(&active_lines(&dir)), vec![2, 3, 4]);
    assert_eq!(seen.count(), 0);
}

// ---- age retention -----------------------------------------------------------------------

#[test]
fn rolled_files_older_than_max_age_are_deleted_at_open() {
    // Spec 006 FR-008: rolled files older than 7 days are deleted at start;
    // younger ones, the active file and every other file stay. Bite: retention
    // not run at open, age measured the wrong way round, a pattern that also
    // matches crash files, the marker, the installer note or look-alikes.
    let t = at(NOON_UTC, 0);
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    std::fs::create_dir_all(&dir).expect("logs dir");
    let foreign = plant_foreign(&dir, t);
    let young = [
        plant_rolled(&dir, t - Duration::from_secs(DAY), 1),
        plant_rolled(&dir, t - Duration::from_secs(3 * DAY), 2),
    ];
    let old = [
        plant_rolled(&dir, t - Duration::from_secs(8 * DAY), 3),
        plant_rolled(&dir, t - Duration::from_secs(30 * DAY), 4),
    ];
    let active = "2026-10-04T11:00:00.000+00:00 INFO dictation rec=5 outcome=too_short\n";
    plant(&dir.join(ACTIVE), active.as_bytes(), t);

    let (_log, _clock, seen) = open_log(&dir, t, 0, small(10_000, 100_000));

    let names = rolled_names(&dir);
    for n in &young {
        assert!(names.contains(n), "{n} (younger than 7 days) was deleted");
    }
    for n in &old {
        assert!(!names.contains(n), "{n} (older than 7 days) was kept");
    }
    assert!(
        recs(&all_lines(&dir)).contains(&5),
        "the active file's line was lost"
    );
    assert_foreign_intact(&dir, &foreign);
    assert_eq!(seen.count(), 0);
}

#[test]
fn rolled_files_are_deleted_after_a_roll_once_they_age() {
    // Retention also runs after each roll, not only at open. A 6-day-old rolled
    // file survives the open; two days later the date roll's retention deletes
    // it, and keeps the younger one. Bite: retention only at open.
    let t = at(NOON_UTC, 0);
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    std::fs::create_dir_all(&dir).expect("logs dir");
    let foreign = plant_foreign(&dir, t);
    let aging = plant_rolled(&dir, t - Duration::from_secs(6 * DAY), 1);
    let young = plant_rolled(&dir, t - Duration::from_secs(DAY), 2);
    let active = "2026-10-04T11:00:00.000+00:00 INFO dictation rec=3 outcome=too_short\n";
    plant(&dir.join(ACTIVE), active.as_bytes(), t);

    let (log, clock, seen) = open_log(&dir, t, 0, small(10_000, 100_000));
    assert!(
        rolled_names(&dir).contains(&aging),
        "6 days old: kept at open"
    );

    clock.set(t + Duration::from_secs(2 * DAY));
    log.write(short(4));
    let names = rolled_names(&dir);
    assert!(
        !names.contains(&aging),
        "8 days old after the roll: deleted"
    );
    assert!(names.contains(&young), "3 days old after the roll: kept");
    assert_eq!(recs(&active_lines(&dir)).last(), Some(&4));
    assert_foreign_intact(&dir, &foreign);
    assert_eq!(seen.count(), 0);
}

#[test]
fn a_clock_one_year_off_deletes_at_most_what_the_rules_name() {
    // Spec 006 T015: a clock moved forward or back one year never deletes the
    // active file or a foreign file; forward, every rolled file is older than 7
    // days (the rule names them all); back, every rolled file lies in the future
    // and none is old. Bite: an age computed with a wrapping subtraction (a
    // future file looks ancient), the active file rolled and deleted at open,
    // retention by "anything not today".
    let t = at(NOON_UTC, 0);
    let year = Duration::from_secs(365 * DAY);
    for (label, now, all_old) in [("forward", t + year, true), ("back", t - year, false)] {
        let tmp = TempDir::new();
        let dir = tmp.path().join("logs");
        std::fs::create_dir_all(&dir).expect("logs dir");
        let foreign = plant_foreign(&dir, t);
        let planted: Vec<String> = (1..=4u64)
            .map(|d| plant_rolled(&dir, t - Duration::from_secs(d * DAY), d))
            .collect();
        let active = "2026-10-04T11:00:00.000+00:00 INFO dictation rec=9 outcome=too_short\n";
        plant(&dir.join(ACTIVE), active.as_bytes(), t);

        let (log, _clock, seen) = open_log(&dir, now, 0, small(10_000, 100_000));
        let names = rolled_names(&dir);
        for n in &planted {
            assert_eq!(
                names.contains(n),
                !all_old,
                "{label}: rolled file {n} kept = {}",
                names.contains(n)
            );
        }
        assert!(
            recs(&all_lines(&dir)).contains(&9),
            "{label}: the active file's line was deleted at open"
        );
        assert_foreign_intact(&dir, &foreign);

        log.write(short(10));
        assert_eq!(
            recs(&active_lines(&dir)).last(),
            Some(&10),
            "{label}: the new line is written"
        );
        assert_foreign_intact(&dir, &foreign);
        assert_eq!(seen.count(), 0, "{label}");
    }
}

// ---- degraded mode ------------------------------------------------------------------------

#[test]
fn a_file_at_the_logs_path_degrades_with_one_not_a_directory_callback() {
    // Acceptance failure branch; spec 006 FR-009 / T017. The probe in
    // voicen-rust:1.99: create_dir_all on a file gives AlreadyExists, opening
    // under it NotADirectory; both are NotADirectory. The clock moves 61 s per
    // write, past reopen_every each time, so every write is a failed reopen.
    // Bite: open or write panicking, a callback per write or per failed reopen,
    // no callback, another reason, the file overwritten or appended to.
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    std::fs::write(&logs, b"not a directory\n").expect("plant file");
    let (log, clock, seen) = open_log(&logs, at(NOON_UTC, 0), 0, LogConfig::default());
    for i in 0..100u64 {
        clock.set(at(NOON_UTC + 61 * i, 0));
        log.write(short(i));
    }
    assert_eq!(seen.count(), 1, "exactly one callback over 100 writes");
    assert_eq!(seen.reasons(), vec![LogsUnwritableReason::NotADirectory]);
    assert_eq!(
        std::fs::read(&logs).expect("still a file"),
        b"not a directory\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_degrades_with_one_disk_full_callback() {
    // Acceptance failure branch: voicen.log linked to /dev/full; a write returns
    // StorageFull (probe in voicen-rust:1.99), which is DiskFull. The clock moves
    // 61 s per write, so each write after the first is a reopen (which succeeds:
    // /dev/full opens) followed by a failed write. Bite: the error mapped to
    // Other, a callback per failed write or per degraded period, a panic on the
    // failed write or flush, a buffered writer that never reports the failure.
    let probe = std::fs::OpenOptions::new()
        .append(true)
        .open("/dev/full")
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"x\n"));
    assert_eq!(
        probe.map_err(|e| e.kind()),
        Err(std::io::ErrorKind::StorageFull),
        "precondition: /dev/full refuses writes with StorageFull"
    );
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    std::fs::create_dir_all(&logs).expect("logs dir");
    std::os::unix::fs::symlink("/dev/full", logs.join(ACTIVE)).expect("symlink");
    // The fake clock runs on the UTC date of the modification time the log itself
    // sees through the link (/dev/full's), from 01:00 to at most 01:00 + 100 x 61 s
    // on that date, offset 0. Whatever that time is (container start, host boot)
    // and whatever today's date is, the log's day never differs from the file's,
    // so no date roll can move the link away and every write after the first is
    // a reopen plus a failed write.
    let file_day = std::fs::metadata(logs.join(ACTIVE))
        .and_then(|m| m.modified())
        .expect("modification time of the linked /dev/full")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("/dev/full modified after the epoch")
        .as_secs()
        / DAY
        * DAY;
    let (log, clock, seen) = open_log(&logs, at(file_day + 3_600, 0), 0, LogConfig::default());
    for i in 0..100u64 {
        clock.set(at(file_day + 3_600 + 61 * i, 0));
        log.write(short(i));
    }
    assert_eq!(seen.count(), 1, "exactly one callback over 100 writes");
    assert_eq!(seen.reasons(), vec![LogsUnwritableReason::DiskFull]);
    assert!(
        std::fs::symlink_metadata(logs.join(ACTIVE)).is_ok_and(|m| m.file_type().is_symlink()),
        "voicen.log is still the link (no roll moved it away)"
    );
}

#[test]
fn a_degraded_log_reopens_at_most_every_interval_and_writes_logs_recovered() {
    // Spec 006 FR-009 / T017: retry at most every reopen_every (60 s, fake clock);
    // once writable, the next due write writes LogsRecovered first, with no second
    // callback; lines dropped while degraded are not replayed. Bite: a reopen on
    // every write (the dir appears at +59 s), no reopen ever, a second callback,
    // no LogsRecovered line, dropped lines queued and written later.
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    std::fs::write(&logs, b"x").expect("plant file");
    let t0 = NOON_UTC;
    let (log, clock, seen) = open_log(&logs, at(t0, 0), 0, LogConfig::default());
    for i in 0..3u64 {
        log.write(short(i));
    }
    assert_eq!(seen.count(), 1);

    std::fs::remove_file(&logs).expect("make the path free");
    clock.set(at(t0 + 59, 0));
    log.write(short(100));
    assert!(
        !logs.exists(),
        "reopened before reopen_every had passed since the failure"
    );

    clock.set(at(t0 + 61, 0));
    log.write(short(101));
    clock.set(at(t0 + 62, 0));
    log.write(short(102));
    let lines = active_lines(&logs);
    let heads: Vec<String> = lines.iter().map(|l| closed(l).head).collect();
    assert_eq!(
        heads,
        ["logs recovered", "dictation", "dictation"],
        "{lines:#?}"
    );
    assert_eq!(recs(&lines), vec![101, 102]);
    assert_eq!(seen.count(), 1, "no second callback after recovery");
}
