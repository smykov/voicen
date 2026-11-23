// Decision #258 (OQ-82 (a)): the settings route is a named T1 residual, not a token.
fn settings() {
    let ts = TimeoutSettings { connect_s: 3, read_s: 5, write_s: 5 };
    let mut s = Settings::default();
    s.timeouts.api_transcription_s = 77;
    s.timeouts.connect_s = 9;
    let _ = (ts, s);
}
