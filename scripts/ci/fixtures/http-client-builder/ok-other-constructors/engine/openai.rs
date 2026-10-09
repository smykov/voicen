use reqwest::blocking::{multipart, RequestBuilder};

fn others() {
    let api = ApiClient::new();
    let store = ModelStore::new(dir, catalog);
    let b = HttpClientBuilder::new();
    let form = multipart::Form::new();
}
