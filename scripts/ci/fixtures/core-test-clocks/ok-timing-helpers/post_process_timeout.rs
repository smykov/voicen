mod common;

const SLACK: Duration = Duration::from_millis(500);

#[test]
fn default_deadline_skips_a_silent_server_at_15_s() {
    let baseline = common::timing::fastest((0..3).map(|_| common::timing::measure(|| process(answering())).1));
    let (got, took) = common::timing::measure(|| process(silent()));
    let want = Duration::from_secs(15);
    common::timing::at_least(took, want);
    common::timing::within_spec(took.less(baseline), want, SLACK, "SC-003");
    assert_eq!(got, Skipped(Timeout));
}
