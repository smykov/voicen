use voicen_core::timeouts::in_scope;

fn run(r: &mut Request) {
    let timeouts = common::deadlines::in_time();
    r.timeouts = timeouts;
    let _ = r.timeouts.connect;
}
