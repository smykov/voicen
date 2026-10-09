pub struct Unanswered {
    pub host: String,
    pub timeouts: Timeouts,
}

impl Unanswered {
    pub fn blackhole() -> Unanswered {
        let connect = Duration::from_millis(300);
        Unanswered {
            host: "192.0.2.1".to_string(),
            timeouts: Timeouts { connect, api_transcription: 10 * connect, ..Timeouts::default() },
        }
    }
}
