// Client::builder() is called only in engine/http.rs.
/// Never `reqwest::Client::new()` here: it installs the default resolver.
//! ClientBuilder::new() lives in engine/http.rs (T-079).
    // let client = Client::builder().build();
fn nothing() {}
