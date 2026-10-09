use std::time::{Duration, Instant};

/// Only the top-level common/ is exempt.
pub fn stamp() -> Duration {
    let started = Instant::now();
    Duration::ZERO.max(Duration::from_nanos(started.elapsed().subsec_nanos() as u64))
}
