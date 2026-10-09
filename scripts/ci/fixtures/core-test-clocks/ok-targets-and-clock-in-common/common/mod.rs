// The OS-answer targets and the clock live here only.
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const HOST: &str = "voicen-test.invalid";

pub fn refused() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 1))
}

pub fn blackhole() -> &'static str {
    "http://192.0.2.1/v1"
}

pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let got = f();
    (got, started.elapsed())
}
