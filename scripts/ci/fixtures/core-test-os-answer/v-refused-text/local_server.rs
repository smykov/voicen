use std::net::SocketAddr;

use voicen_core::timeouts::Timeouts;

#[test]
fn refused_local_server_by_address() {
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));
    let got = run(&format!("http://{addr}/v1"), &Timeouts::default());
    assert!(got.is_err());
    let again: SocketAddr =
        "127.0.0.1:1".parse().expect("addr");
    assert_eq!(addr, again);
}
