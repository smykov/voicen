//! The one client constructor (T-079).
use reqwest::blocking::Client;

pub(crate) fn client(connect: Duration) -> Result<Client, TransportError> {
    Client::builder()
        .connect_timeout(connect)
        .dns_resolver(Arc::new(DeadlineResolver::system()))
        .build()
        .map_err(|_| TransportError::Setup)
}

fn also_here() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::ClientBuilder::new()
}
