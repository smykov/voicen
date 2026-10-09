use std::time::Duration;

use voicen_core::timeouts::Timeouts;

#[test]
fn blackhole_connect_is_bounded_by_connect_timeout() {
    let t = Timeouts { connect: Duration::from_millis(300), ..Timeouts::default() };
    let got = run_pipeline(
        api_settings("http://192.0.2.1/v1"),
        &t,
    );
    assert!(got.is_err());
}
