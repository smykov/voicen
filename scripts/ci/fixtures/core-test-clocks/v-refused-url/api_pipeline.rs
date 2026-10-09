use voicen_core::timeouts::Timeouts;

#[test]
fn refused_server_is_cannot_reach() {
    let t = Timeouts::default();
    let base = "http://127.0.0.1:1/v1";
    let got = run_pipeline(api_settings(base), &t);
    assert!(matches!(got, JobEnd::Failed(FailureReason::CannotReach { .. })));
}
