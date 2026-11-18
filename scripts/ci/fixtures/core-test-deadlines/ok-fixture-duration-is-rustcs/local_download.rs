// Not a token: the Duration argument is refused by rustc once common::download::fixture
// takes the server only (pinned by fn-pointer checks in common/deadlines_tests.rs), so the
// tripwire stays silent here and the decision is the compiler's.
fn legacy(server: &Server) {
    let f = fixture(&server, Duration::from_secs(2));
    let _ = f;
}
