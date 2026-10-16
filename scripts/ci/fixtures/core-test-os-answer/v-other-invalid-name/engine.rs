use voicen_core::timeouts::Timeouts;

#[test]
fn another_invalid_name() {
    let t = Timeouts::default();
    let base = "http://nowhere.invalid:8443/v1";
    assert!(engine(base).transcribe(&t).is_err());
}
