mod common;

#[test]
fn unresolvable_host_is_network_unavailable() {
    let case = common::os_answer::OsAnswer::unresolvable();
    let engine = api_engine(&format!("http://{}/v1", case.host), Some(KEY));
    let (got, took) = common::timing::measure(|| engine.transcribe(&case.timeouts));
    common::timing::at_least(took, Duration::ZERO);
    assert!(matches!(got, Err(FailureReason::NetworkUnavailable)));
}
