mod common;

const SLACK: Duration = Duration::from_millis(500);

#[test]
fn default_deadline_skips_a_silent_server_at_15_s() {
    let (_, baseline) = common::timing::measure(|| process(answering()));
    let (got, took) = common::timing::measure(|| process(silent()));
    let want = Duration::from_secs(15);
    common::timing::at_least(took, want);
    common::timing::within_spec(took.saturating_sub(baseline), want, SLACK, "SC-003");
    assert_eq!(got, Skipped(Timeout));
}
