//! The one wall-clock port of the core (P-011): the settings backup suffix (T-032),
//! history (005) and the connection tester (R-9) read the time through [`Clock`].

use std::time::SystemTime;

/// Wall-clock time source.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// `t` in UTC as `yyyyMMdd-HHmmss` (the `<UTC>` of `settings.json.bad-<UTC>`).
/// A time before the Unix epoch gives the epoch. Std only (no date crate).
pub fn utc_compact(t: SystemTime) -> String {
    // T-032 skeleton (test-writer): not implemented; the red test
    // `clock::tests::utc_compact_table` fails on this value.
    let _ = t;
    String::new()
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
