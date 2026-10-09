mod common;

use common::os_answer::Unanswered;

#[test]
fn blackhole_connect_is_bounded_by_connect_timeout() {
    // The never-answered target comes with its deadlines and accepted endings.
    let case = Unanswered::blackhole();
    let mut h = harness_with(energy_gate(), Some(case.timeouts), creds_with_key());
    let rec = h.record(speech_3s(), api_settings(&format!("http://{}/v1", case.host)));
    let (report, _) = h.run(rec);
    assert!(matches!(&report.end, JobEnd::Failed(r) if case.ended_by_connect_or_os(r)));
}

#[test]
fn saved_local_server_is_used_by_the_next_press() {
    // Never contacted: a TEST-NET-2 settings value, not an OS-answer target.
    rig.save(|s| s.local_server.base_url = "http://198.51.100.9:9/v1".to_string());
}
