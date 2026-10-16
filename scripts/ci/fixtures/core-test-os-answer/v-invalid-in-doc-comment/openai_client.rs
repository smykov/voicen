//! Fake data only: example.com, voicen-test.invalid.

#[test]
fn transcribes() {
    assert!(run("http://example.com/v1").is_ok());
}
