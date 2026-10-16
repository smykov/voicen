#[test]
fn unresolvable_host_is_reported() {
    let case = common::os_answer::OsAnswer::unresolvable();
    // the host is voicen-test.invalid, which never resolves
    assert!(run(&case).is_err());
}
