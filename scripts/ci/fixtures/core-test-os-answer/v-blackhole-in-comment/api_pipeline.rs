#[test]
fn blackhole_connect_is_bounded() {
    let case = common::os_answer::Unanswered::blackhole();
        // 192.0.2.1 blackholes the connect
    assert!(run(&case).is_err());
}
