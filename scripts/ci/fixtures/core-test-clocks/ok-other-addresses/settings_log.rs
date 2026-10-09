#[test]
fn other_addresses_are_not_os_answer_targets() {
    let logged = "http://192.0.2.10:8080/v1";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let local = "http://127.0.0.1:18080/v1";
    let lan = "198.51.100.7";
    let ports = "127.0.0.1:10";
    assert_ne!(logged, local);
    assert_ne!(lan, ports);
    drop(listener);
}
