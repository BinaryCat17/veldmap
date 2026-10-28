use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt as _;
use hyper::body::Incoming;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustls::{ClientConfig, RootCertStore};
use tokio::time::{Instant, Sleep};
use wasmtime_wasi_http::{Error, RequestOptions, WasiBody, WasiHttpHooks};

use crate::policy::ComponentPolicy;

const IO_TIMEOUT: Duration = Duration::from_secs(10);

type IoFuture = Box<dyn Future<Output = Result<(), Error>> + Send>;
type SendFuture = Box<
    dyn Future<Output = std::result::Result<(http::Response<WasiBody>, IoFuture), Error>> + Send,
>;

pub struct EgressHooks {
    allowed_origins: BTreeSet<String>,
    tls_config: Option<Arc<ClientConfig>>,
}

impl EgressHooks {
    pub fn new(policy: &ComponentPolicy, directory: &Path) -> anyhow::Result<Self> {
        let allowed_origins = policy.egress_origins.iter().cloned().collect();
        if policy.additional_tls_roots.is_empty() {
            return Ok(Self {
                allowed_origins,
                tls_config: None,
            });
        }
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for path in &policy.additional_tls_roots {
            let path = directory.join(path);
            let pem = fs::read(&path)
                .with_context(|| format!("failed to read TLS roots {}", path.display()))?;
            let certificates = rustls_pemfile::certs(&mut pem.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("failed to parse TLS roots {}", path.display()))?;
            let (accepted, rejected) = roots.add_parsable_certificates(certificates);
            if accepted == 0 || rejected != 0 {
                return Err(anyhow!("invalid additional TLS roots {}", path.display()));
            }
        }
        let tls_config =
            ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            allowed_origins,
            tls_config: Some(Arc::new(tls_config)),
        })
    }
}

impl WasiHttpHooks for EgressHooks {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        _fut: Box<dyn Future<Output = Result<(), Error>> + Send>,
    ) -> SendFuture {
        let allowed = request
            .uri()
            .scheme()
            .zip(request.uri().authority())
            .is_some_and(|(scheme, authority)| {
                self.allowed_origins
                    .contains(&format!("{scheme}://{authority}"))
            });
        if !allowed {
            return Box::new(async { Err(Error::HttpRequestDenied) });
        }
        let Some(tls_config) = self.tls_config.clone() else {
            return Box::new(async move {
                let (response, io) =
                    wasmtime_wasi_http::default_send_request(request, options).await?;
                Ok((
                    response.map(|body| body.boxed_unsync()),
                    Box::new(io) as IoFuture,
                ))
            });
        };
        Box::new(async move {
            let connect_timeout = options
                .as_ref()
                .and_then(|options| options.connect_timeout)
                .unwrap_or(IO_TIMEOUT);
            let first_byte_timeout = options
                .as_ref()
                .and_then(|options| options.first_byte_timeout)
                .unwrap_or(IO_TIMEOUT);
            let between_bytes_timeout = options
                .and_then(|options| options.between_bytes_timeout)
                .unwrap_or(IO_TIMEOUT);
            let mut connector = HttpConnector::new();
            connector.enforce_http(false);
            connector.set_connect_timeout(Some(connect_timeout));
            let connector = HttpsConnectorBuilder::new()
                .with_tls_config((*tls_config).clone())
                .https_or_http()
                .enable_http1()
                .build();
            let client = Client::builder(TokioExecutor::new()).build(connector);
            let response = tokio::time::timeout(first_byte_timeout, client.request(request))
                .await
                .map_err(|_| Error::ConnectionReadTimeout)?
                .map_err(|error| Error::InternalError(Some(error.to_string())))?;
            let response =
                response.map(|body| TimeoutBody::new(body, between_bytes_timeout).boxed_unsync());
            Ok((response, Box::new(async { Ok(()) }) as IoFuture))
        })
    }
}

struct TimeoutBody {
    inner: Incoming,
    timeout: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl TimeoutBody {
    fn new(inner: Incoming, timeout: Duration) -> Self {
        Self {
            inner,
            timeout,
            deadline: Box::pin(tokio::time::sleep(timeout)),
        }
    }
}

impl Body for TimeoutBody {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(context) {
            Poll::Ready(Some(Ok(frame))) => {
                let deadline = Instant::now() + self.timeout;
                self.deadline.as_mut().reset(deadline);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error.into()))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => match self.deadline.as_mut().poll(context) {
                Poll::Ready(()) => Poll::Ready(Some(Err(Error::ConnectionReadTimeout))),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
