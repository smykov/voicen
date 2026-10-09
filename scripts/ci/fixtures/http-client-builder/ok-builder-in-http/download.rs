use crate::engine::http;

fn transfer() {
    let client = http::client_with_read_timeout(connect, no_data)?;
    let sent = client.get(url).send();
}
