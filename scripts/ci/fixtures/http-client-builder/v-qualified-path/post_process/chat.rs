fn send() {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(timeouts.connect)
        .build();
}
