//! Fake data only: 127.0.0.1, 192.0.2.1 (RFC 5737), voicen-test.invalid.

/// Formerly timed with Instant::now() and `started.elapsed()`; took < 1.5 s.
fn helper() {}

#[test]
fn comments_name_targets_and_clocks() {
    // 192.0.2.1 blackholes; 127.0.0.1:1 is refused; voicen-test.invalid fails.
    // let started = Instant::now(); assert!(started.elapsed() < Duration::from_secs(1));
        // an indented comment: use std::time::Instant as Clock;
    helper();
}
