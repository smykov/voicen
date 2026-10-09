use std::time::Duration;

use std::time::Instant as Clock;

#[test]
fn bounded_by_an_aliased_clock() {
    let at = Clock::now();
    let got = request();
    assert!(got.is_err());
    assert!(Clock::now() - at < Duration::from_secs(3));
}
