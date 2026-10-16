#[test]
fn a_host_built_from_parts() {
    let name = "voicen-test";
    let base = format!("http://{}.invalid/v1", name);
    assert!(engine(&base).transcribe(&Timeouts::default()).is_err());
}
