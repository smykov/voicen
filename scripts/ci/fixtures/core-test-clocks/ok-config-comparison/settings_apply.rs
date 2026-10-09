use std::time::Duration;

#[test]
fn configured_deadlines_are_compared_not_measured() {
    let t = Timeouts::default();
    assert!(t.connect < Duration::from_secs(10));
    assert!(t.post_processing <= Duration::from_millis(15_000));
    let elapsed_field = Report { elapsed_ms: 1500 };
    assert!(elapsed_field.elapsed_ms < 2000);
}
