mod diag_support;

#[test]
fn line_appears() {
    assert!(diag_support::wait_for_line(std::time::Duration::from_secs(1)));
}
