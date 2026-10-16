use std::time::{Duration, Instant};

use voicen_core::timeouts::Timeouts;

#[test]
fn configured_deadline_skips_at_5_s() {
    let t = Timeouts { post_processing: Duration::from_secs(5), ..Timeouts::default() };
    let started = Instant::now();
    let got = process(silent_server(), &t);
    let took = started.elapsed();
    assert_eq!(got, Skipped(Timeout));
    assert!(took < Duration::from_millis(1500) + Duration::from_secs(5), "{took:?}");
}
