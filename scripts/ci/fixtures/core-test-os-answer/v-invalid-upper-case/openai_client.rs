#[test]
fn unresolvable_host_is_network_unavailable() {
    let engine = api_engine("http://VOICEN-TEST.INVALID/v1", Some(KEY));
    assert!(engine.transcribe(&Timeouts::default()).is_err());
}
