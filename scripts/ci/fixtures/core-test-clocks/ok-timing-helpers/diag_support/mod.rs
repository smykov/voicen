use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn stamp(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}
