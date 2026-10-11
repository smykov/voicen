// T-092 mechanism: an integration test (kind test) still runs; it is the only test exe kind
// build.rs configures (rustc-link-arg-tests).
#[test]
fn integration_runs() {
    assert_eq!(voicen_lib::run(), 7);
}
