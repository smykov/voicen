use std::time::{Duration, Instant};

pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let got = f();
    (got, started.elapsed())
}
