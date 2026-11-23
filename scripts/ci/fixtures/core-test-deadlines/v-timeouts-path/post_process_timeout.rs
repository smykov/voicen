fn t() {
    handoff(Timeouts::default());
}
fn u() {
    handoff(Timeouts::from_settings(&s));
}
