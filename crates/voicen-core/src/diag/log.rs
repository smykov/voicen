//! The log writer (T-008): `logs/voicen.log`, rolled into `voicen-*.log`, retained
//! by size and age, and a degraded mode when the folder cannot be written.
//!
//! - Size: the active file stays `<= roll_bytes` (rolled before a line would cross
//!   it, or at the local date change); the rolled `voicen-*.log` files total
//!   `<= total_bytes - roll_bytes`, so the whole set is `<= total_bytes` at every
//!   instant.
//! - Age: rolled files older than `max_age` are deleted at open and after each
//!   roll. Retention touches only `voicen-*.log`, never `voicen.log` or any other
//!   file in the folder.
//! - Failure: `write` never panics and never surfaces an I/O error. An unwritable
//!   log is degraded; `on_unwritable` is called at most once per `Log`; reopening
//!   is tried at most every `reopen_every`, and a successful reopen writes
//!   `LogsRecovered` first.
//!
//! A rolled file is named after the local time of its roll,
//! `voicen-YYYYMMDD-HHMMSS.log`, with `-1`, `-2`, ... when that name is taken (the
//! rename never replaces a file). Its age is its modification time (the time of its
//! last line); size retention deletes the oldest first, by that time and then by
//! name.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use super::event::LogEvent;
use super::format::{format_line, LocalTime};
use crate::clock::{Clock, LocalOffset};

/// The active file (the Windows CI smoke reads `logs\voicen.log`).
const ACTIVE: &str = "voicen.log";
const ROLLED_PREFIX: &str = "voicen-";
const ROLLED_SUFFIX: &str = ".log";
/// Rolls within one second take `-1` .. `-999`; past that the roll fails (degraded).
const MAX_SAME_SECOND_ROLLS: u32 = 999;

/// Sizes and intervals of the log (FR-20: 7 days, 10 MB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogConfig {
    /// The active file is rolled before a line would make it larger (2 MiB).
    pub roll_bytes: u64,
    /// The active file plus the rolled files never exceed this (10 MiB).
    pub total_bytes: u64,
    /// Rolled files older than this are deleted (7 days).
    pub max_age: Duration,
    /// A degraded log tries to reopen at most this often (60 s).
    pub reopen_every: Duration,
}

impl Default for LogConfig {
    /// 2 MiB, 10 MiB, 7 days, 60 s.
    fn default() -> LogConfig {
        const MIB: u64 = 1024 * 1024;
        LogConfig {
            roll_bytes: 2 * MIB,
            total_bytes: 10 * MIB,
            max_age: Duration::from_secs(7 * 86_400),
            reopen_every: Duration::from_secs(60),
        }
    }
}

/// Why the log cannot be written (T-054 shows it to the user once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsUnwritableReason {
    PermissionDenied,
    DiskFull,
    /// The logs path, or a part of it, is not a directory.
    NotADirectory,
    /// Any other error, with its raw OS code if it has one.
    Other(Option<i32>),
}

/// Called at most once per [`Log`] when it becomes unwritable.
pub type OnUnwritable = Box<dyn Fn(LogsUnwritableReason) + Send + Sync>;

/// The one log of the process. `Send + Sync`; whole lines under one mutex.
pub struct Log {
    writer: Mutex<Writer>,
}

struct Writer {
    dir: PathBuf,
    clock: Arc<dyn Clock>,
    offset: Arc<dyn LocalOffset>,
    cfg: LogConfig,
    /// Taken by the first failure, so it is called at most once.
    on_unwritable: Option<OnUnwritable>,
    state: State,
}

enum State {
    Healthy(Active),
    /// The last open or write failed at `failed_at`; lines are dropped until a
    /// reopen, tried at most every `reopen_every` after it.
    Degraded {
        failed_at: SystemTime,
    },
}

/// The open `voicen.log`.
struct Active {
    file: File,
    /// Bytes in the file (its size at open plus every line written since).
    len: u64,
    /// Local day number of the last line (the file's modification time for a file
    /// found at open); `None` while unknown.
    day: Option<u64>,
}

/// The callback and its reason, to be called once the lock is released.
type Notify = Option<(OnUnwritable, LogsUnwritableReason)>;

impl Log {
    /// Opens `dir/voicen.log` (creating `dir`), runs retention, and never fails:
    /// an unwritable folder gives a degraded log and one `on_unwritable` call.
    /// Writes nothing by itself (the shell writes `Started` first).
    pub fn open(
        dir: PathBuf,
        clock: Arc<dyn Clock>,
        offset: Arc<dyn LocalOffset>,
        cfg: LogConfig,
        on_unwritable: OnUnwritable,
    ) -> Log {
        let now = clock.now();
        let mut writer = Writer {
            dir,
            clock,
            offset,
            cfg,
            on_unwritable: Some(on_unwritable),
            state: State::Degraded { failed_at: now },
        };
        let notify = match writer.open_active() {
            Ok(active) => {
                writer.state = State::Healthy(active);
                retain(&writer.dir, now, &writer.cfg);
                None
            }
            Err(reason) => writer.fail(now, reason),
        };
        if let Some((callback, reason)) = notify {
            callback(reason);
        }
        Log {
            writer: Mutex::new(writer),
        }
    }

    /// Appends one whole line for `event` (rolling and retaining first when due).
    /// Never panics; an I/O error degrades the log instead of surfacing.
    pub fn write(&self, event: LogEvent) {
        let notify = self
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(&event);
        // Outside the lock: the callback may write to this log.
        if let Some((callback, reason)) = notify {
            callback(reason);
        }
    }
}

impl Writer {
    fn write(&mut self, event: &LogEvent) -> Notify {
        let now = self.clock.now();
        let local = LocalTime::of(now, self.offset.seconds_east(now));
        if let State::Degraded { failed_at } = self.state {
            let due = now
                .duration_since(failed_at)
                // A clock moved back cannot tell the interval: try again.
                .map_or(true, |since| since >= self.cfg.reopen_every);
            if !due {
                return None;
            }
            match self.open_active() {
                Ok(active) => self.state = State::Healthy(active),
                Err(reason) => return self.fail(now, reason),
            }
            let recovered = format_line(now, local.offset, &LogEvent::LogsRecovered);
            if let Err(reason) = self.append(now, &local, &recovered) {
                return self.fail(now, reason);
            }
        }
        let line = format_line(now, local.offset, event);
        match self.append(now, &local, &line) {
            Ok(()) => None,
            Err(reason) => self.fail(now, reason),
        }
    }

    /// Degraded from `now`; the callback, if it was not called before.
    fn fail(&mut self, now: SystemTime, reason: LogsUnwritableReason) -> Notify {
        self.state = State::Degraded { failed_at: now };
        self.on_unwritable.take().map(|callback| (callback, reason))
    }

    /// Creates the folder and opens `voicen.log` for appending. Read access too, so
    /// the handle can always be asked for its size and time (an append-only handle
    /// on Windows carries no read-attributes right).
    fn open_active(&self) -> Result<Active, LogsUnwritableReason> {
        fs::create_dir_all(&self.dir).map_err(|e| unwritable(&self.dir, &e))?;
        let file = OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .open(self.dir.join(ACTIVE))
            .map_err(|e| unwritable(&self.dir, &e))?;
        let meta = file.metadata().ok();
        let len = meta.as_ref().map_or(0, fs::Metadata::len);
        let day = if len == 0 {
            None
        } else {
            meta.and_then(|m| m.modified().ok())
                .map(|t| LocalTime::of(t, self.offset.seconds_east(t)).day_number)
        };
        Ok(Active { file, len, day })
    }

    /// One line plus `'\n'` in one write, after a roll when the line would cross
    /// `roll_bytes` or the local date changed. A line longer than `roll_bytes`
    /// can never fit and is dropped (only a config below one line's length).
    fn append(
        &mut self,
        now: SystemTime,
        local: &LocalTime,
        line: &str,
    ) -> Result<(), LogsUnwritableReason> {
        let bytes = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if bytes > self.cfg.roll_bytes {
            return Ok(());
        }
        let State::Healthy(active) = &self.state else {
            return Ok(());
        };
        let crosses = active.len.saturating_add(bytes) > self.cfg.roll_bytes;
        let new_day = active.day.is_some_and(|day| day != local.day_number);
        if active.len > 0 && (crosses || new_day) {
            self.roll(now, local)?;
        }
        let State::Healthy(active) = &mut self.state else {
            return Ok(());
        };
        let mut buf = Vec::with_capacity(line.len().saturating_add(1));
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        if let Err(e) = active.file.write_all(&buf) {
            // Best effort: cut a partly written line, so the file keeps whole lines.
            let _ = active.file.set_len(active.len);
            return Err(unwritable(&self.dir, &e));
        }
        active.len = active.len.saturating_add(bytes);
        active.day = Some(local.day_number);
        Ok(())
    }

    /// `voicen.log` -> a free `voicen-<local time>[-n].log`, then retention, then a
    /// new empty `voicen.log`.
    fn roll(&mut self, now: SystemTime, local: &LocalTime) -> Result<(), LogsUnwritableReason> {
        // Close the file first (Windows renames an open file only when every
        // handle allows it).
        self.state = State::Degraded { failed_at: now };
        let target = free_rolled_path(&self.dir, local)?;
        fs::rename(self.dir.join(ACTIVE), &target).map_err(|e| unwritable(&self.dir, &e))?;
        retain(&self.dir, now, &self.cfg);
        self.state = State::Healthy(self.open_active()?);
        Ok(())
    }
}

/// The first of `voicen-YYYYMMDD-HHMMSS.log`, `...-1.log`, `...-2.log`, ... that
/// does not exist (`fs::rename` would replace it on every platform).
fn free_rolled_path(dir: &Path, t: &LocalTime) -> Result<PathBuf, LogsUnwritableReason> {
    let stamp = format!(
        "{ROLLED_PREFIX}{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
    (0..=MAX_SAME_SECOND_ROLLS)
        .map(|n| {
            if n == 0 {
                dir.join(format!("{stamp}{ROLLED_SUFFIX}"))
            } else {
                dir.join(format!("{stamp}-{n}{ROLLED_SUFFIX}"))
            }
        })
        .find(|path| fs::symlink_metadata(path).is_err())
        .ok_or(LogsUnwritableReason::Other(None))
}

/// A rolled file found by retention.
struct Rolled {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    /// `(stamp, n)` of `voicen-<stamp>[-n].log`; `(stem, 0)` for any other name.
    order: (String, u64),
}

/// Deletes the regular `voicen-*.log` files of `dir` older than `max_age`, then
/// the oldest of them while they total more than `total_bytes - roll_bytes`. Any
/// other file, `voicen.log` included, is never touched. Errors are ignored: a file
/// that cannot be read is skipped, one that cannot be deleted stays counted.
fn retain(dir: &Path, now: SystemTime, cfg: &LogConfig) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut rolled: Vec<Rolled> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name
                .strip_prefix(ROLLED_PREFIX)?
                .strip_suffix(ROLLED_SUFFIX)?;
            let order = rolled_order(stem);
            // Not followed: a link or a directory is not a rolled file.
            let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            Some(Rolled {
                path: entry.path(),
                len: meta.len(),
                modified: meta.modified().unwrap_or(now),
                order,
            })
        })
        .collect();

    rolled.retain(|file| {
        // A file from the future (a clock moved back) has age zero.
        let old = now
            .duration_since(file.modified)
            .is_ok_and(|age| age > cfg.max_age);
        !(old && fs::remove_file(&file.path).is_ok())
    });

    rolled.sort_by(|a, b| (a.modified, &a.order).cmp(&(b.modified, &b.order)));
    let limit = cfg.total_bytes.saturating_sub(cfg.roll_bytes);
    let mut total: u64 = rolled
        .iter()
        .fold(0, |sum, file| sum.saturating_add(file.len));
    for file in &rolled {
        if total <= limit {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            total = total.saturating_sub(file.len);
        }
    }
}

/// `("YYYYMMDD-HHMMSS", n)` for `YYYYMMDD-HHMMSS-n`, else `(stem, 0)`.
fn rolled_order(stem: &str) -> (String, u64) {
    match stem.rsplit_once('-') {
        Some((stamp, n)) if stamp.len() == 15 => match n.parse::<u64>() {
            Ok(n) => (stamp.to_string(), n),
            Err(_) => (stem.to_string(), 0),
        },
        _ => (stem.to_string(), 0),
    }
}

/// The reason of a failed open, write or roll under `dir`. `NotADirectory` when
/// the nearest existing part of `dir` is not a directory, whatever the error says
/// (`create_dir_all` over a file reports `AlreadyExists`; on Windows a file in the
/// middle of the path reports `NotFound` or `AlreadyExists`).
fn unwritable(dir: &Path, err: &io::Error) -> LogsUnwritableReason {
    let blocked_by_file = dir
        .ancestors()
        .find_map(|part| fs::metadata(part).ok())
        .is_some_and(|meta| !meta.is_dir());
    if blocked_by_file {
        return LogsUnwritableReason::NotADirectory;
    }
    match err.kind() {
        io::ErrorKind::PermissionDenied => LogsUnwritableReason::PermissionDenied,
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => LogsUnwritableReason::DiskFull,
        io::ErrorKind::NotADirectory => LogsUnwritableReason::NotADirectory,
        _ => LogsUnwritableReason::Other(err.raw_os_error()),
    }
}
