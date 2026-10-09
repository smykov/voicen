use std::time::Duration;

use voicen_core::timeouts::Timeouts;

#[test]
fn unresolvable_host_is_network_unavailable() {
    // T-078's shape: the .invalid target with a connect timeout of its own.
    let timeouts = Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_secs(5),
        ..Timeouts::default()
    };
    let engine = api_engine("http://voicen-test.invalid/v1", Some(KEY));
    let got = engine.transcribe(&timeouts);
    assert!(matches!(got, Err(FailureReason::NetworkUnavailable)));
}

#[test]
fn unresolvable_host_error_drops_the_query() {
    let engine = api_engine(
        &format!("http://voicen-test.invalid/v1?api-version={QUERY_SECRET}"),
        Some(KEY),
    );
    let got = engine.transcribe(&Timeouts::default());
    assert!(got.is_err());
}
