use reqwest::blocking::Client;

fn transfer(&self) {
    // Per-read timeout (also bounds the wait for the headers).
    let client = Client::builder()
        .connect_timeout(self.timeouts.connect)
        .timeout(self.timeouts.download_no_data)
        .build();
}
