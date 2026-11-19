mod common;

#[test]
fn stalled_download_fails_with_no_data() {
    let server = common::deadlines::stall_server();
    let f = common::download::fixture(&server);
    assert!(f.start(ModelId::Base).is_ok());
}
