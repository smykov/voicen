// Only the top-level engine/http.rs may build a client.

pub(crate) fn client() {
    Client::builder().build()
}
