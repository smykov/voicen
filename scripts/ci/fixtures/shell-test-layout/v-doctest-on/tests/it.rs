// T-092 fixture: an integration test (kind test): every one gets -tests link args.
#[test]
fn it_runs() {
    assert_eq!(voicen_lib::run(), 7);
}
