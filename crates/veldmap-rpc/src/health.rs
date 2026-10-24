use std::collections::BTreeSet;
use std::sync::Arc;

use connectrpc::{
    ConnectError, RequestContext, Response, Router, ServiceRequest, ServiceResult, ServiceStream,
};
pub use veldmap_proto::grpc::health::v1::health_check_response::ServingStatus;
use veldmap_proto::grpc::health::v1::{HEALTH_CHECK_SPEC, HEALTH_WATCH_SPEC, Health, HealthExt};
pub use veldmap_proto::grpc::health::v1::{HealthCheckRequest, HealthCheckResponse, HealthClient};

use crate::transport::{RouteClass, with_route_class};

struct StaticHealth {
    services: BTreeSet<String>,
}

#[allow(refining_impl_trait)]
impl Health for StaticHealth {
    async fn check(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, HealthCheckRequest>,
    ) -> ServiceResult<HealthCheckResponse> {
        let service = request.service;
        if !service.is_empty() && !self.services.contains(service) {
            return Err(ConnectError::not_found(format!(
                "unknown service: {service}"
            )));
        }
        Response::ok(HealthCheckResponse {
            status: ServingStatus::SERVING.into(),
            ..Default::default()
        })
    }

    async fn watch(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, HealthCheckRequest>,
    ) -> ServiceResult<ServiceStream<HealthCheckResponse>> {
        Err(ConnectError::unimplemented("health watch is not supported"))
    }
}

#[must_use]
pub fn install_health<I, S>(router: Router, services: I) -> Router
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let services = services.into_iter().map(Into::into).collect();
    let router = Arc::new(StaticHealth { services }).register(router);
    let router = with_route_class(router, HEALTH_CHECK_SPEC.procedure, RouteClass::Control);
    with_route_class(router, HEALTH_WATCH_SPEC.procedure, RouteClass::Control)
}
