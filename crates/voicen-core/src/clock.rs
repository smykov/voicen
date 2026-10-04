//! The one wall-clock port of the core (P-011): the settings backup suffix (T-032),
//! history (005) and the connection tester (R-9) read the time through [`Clock`].

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock time source.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The local time zone's offset from UTC (T-008: log timestamps and the local
/// date roll). The wall time itself still comes from [`Clock`]; this port only
/// says how far local time is from UTC at an instant. The shell fills it from the
/// Windows time-zone rules; tests use a fixed offset.
pub trait LocalOffset: Send + Sync {
    /// Seconds east of UTC in effect at `t` (UTC+02:00 -> 7200, UTC-03:30 -> -12600).
    fn seconds_east(&self, t: SystemTime) -> i32;
}

/// The operating system's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// `t` in UTC as `yyyyMMdd-HHmmss` (the `<UTC>` of `settings.json.bad-<UTC>`).
/// A time before the Unix epoch gives the epoch; the sub-second part is dropped.
/// Std only (no date crate).
pub fn utc_compact(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 -> proleptic Gregorian (year, month, day): the
/// days-from-civil inverse (H. Hinnant, "chrono-Compatible Low-Level Date
/// Algorithms"), for non-negative day counts only. Also the date math of the log's
/// local timestamps (`diag::format`, T-008).
pub(crate) fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468; // days since 0000-03-01
    let era = z / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365], from March 1
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// A [`Clock`] that returns a fixed time, changeable with [`set`](Self::set).
#[cfg(any(test, feature = "test-fakes"))]
pub struct FakeClock {
    now: std::sync::Mutex<SystemTime>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeClock {
    pub fn at(t: SystemTime) -> FakeClock {
        FakeClock {
            now: std::sync::Mutex::new(t),
        }
    }

    pub fn set(&self, t: SystemTime) {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = t;
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn utc_compact_table() {
        // Bite: wrong days-to-civil conversion (leap days, century years), fields in
        // the wrong order or not zero-padded, local time instead of UTC, rounding
        // the sub-second part up, or a panic before the epoch.
        let table = [
            (UNIX_EPOCH, "19700101-000000"),
            (at(951_782_400), "20000229-000000"),
            (at(1_000_000_000), "20010909-014640"),
            (at(1_234_567_890), "20090213-233130"),
            (at(1_709_251_199), "20240229-235959"),
            (
                at(1_709_251_199) + Duration::from_millis(999),
                "20240229-235959",
            ),
            (at(4_102_444_799), "20991231-235959"),
            (UNIX_EPOCH - Duration::from_secs(1), "19700101-000000"),
            (
                UNIX_EPOCH - Duration::from_secs(86_400 * 400),
                "19700101-000000",
            ),
        ];
        for (t, expected) in table {
            assert_eq!(utc_compact(t), expected, "{t:?}");
        }
    }

    #[test]
    fn fake_clock_returns_the_time_it_was_set_to() {
        // Fixture check: the service tests derive the backup name from this time.
        let clock = FakeClock::at(at(1_709_251_199));
        assert_eq!(clock.now(), at(1_709_251_199));
        clock.set(at(42));
        assert_eq!(clock.now(), at(42));
    }
}
