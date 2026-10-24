use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use connectrpc::client::{CallOptions, ClientConfig, HttpClient};
use connectrpc::{BoundServer, Limits, Router, Server};
use http::Uri;

pub const CONTROL_MESSAGE_LIMIT: usize = 64 * 1024;
pub const CATALOG_MESSAGE_LIMIT: usize = 4 * 1024 * 1024;
pub const MUTATION_MESSAGE_LIMIT: usize = 4 * 1024 * 1024;
pub const BULK_MESSAGE_LIMIT: usize = 128 * 1024 * 1024;

pub type TransportResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteClass {
    Control,
    Catalog,
    Mutation,
    Bulk,
}

impl RouteClass {
    #[must_use]
    pub fn max_message_bytes(self) -> usize {
        match self {
            Self::Control => CONTROL_MESSAGE_LIMIT,
            Self::Catalog => CATALOG_MESSAGE_LIMIT,
            Self::Mutation => MUTATION_MESSAGE_LIMIT,
            Self::Bulk => BULK_MESSAGE_LIMIT,
        }
    }
}

#[must_use]
pub fn route_limits(class: RouteClass) -> Limits {
    let message_limit = class.max_message_bytes();
    Limits::default()
        .with_max_message_size(message_limit)
        .with_max_request_body_size(message_limit + connectrpc::envelope::HEADER_SIZE)
}

#[must_use]
pub fn with_route_class(router: Router, procedure: &str, class: RouteClass) -> Router {
    router.with_route_limits(procedure, route_limits(class))
}

#[must_use]
pub fn call_options(class: RouteClass) -> CallOptions {
    CallOptions::default().with_max_message_size(class.max_message_bytes())
}

#[must_use]
pub fn client_config(uri: Uri, request_timeout: Duration) -> ClientConfig {
    ClientConfig::new(uri)
        .with_default_timeout(request_timeout)
        .with_default_max_message_size(CATALOG_MESSAGE_LIMIT)
}

#[must_use]
pub fn plaintext_client() -> HttpClient {
    HttpClient::plaintext()
}

pub async fn bind_loopback(port: u16) -> TransportResult<BoundServer> {
    Server::bind(SocketAddr::from(([127, 0, 0, 1], port))).await
}
