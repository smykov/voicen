#[test]
fn refused_local_server() {
    let case = common::os_answer::OsAnswer::refused();
    // the refused loopback is 127.0.0.1:1
    assert!(run(&case).is_err());
}
