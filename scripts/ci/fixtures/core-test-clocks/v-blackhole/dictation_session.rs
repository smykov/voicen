#[test]
fn saved_local_server_is_used_by_the_next_press() {
    rig.save(|s| {
        s.engine = EngineKind::LocalServer;
        s.local_server.base_url = "http://192.0.2.1:9/v1".to_string();
    });
}
