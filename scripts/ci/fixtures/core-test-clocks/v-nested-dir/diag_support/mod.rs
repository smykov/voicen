use std::time::{Duration, Instant};

/// Waits for the log line.
pub fn wait_for_line(budget: Duration) -> bool {
    loop {
        let started = Instant::now();
        if poll() || budget == Duration::ZERO {
            return true;
        }
        let _ = started;
    }
}
