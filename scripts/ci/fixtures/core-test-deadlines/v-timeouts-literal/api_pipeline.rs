use voicen_core::timeouts::Timeouts;

fn t() {
    handoff(Timeouts {
        connect: Duration::from_millis(300),
        ..in_time()
    });
}
