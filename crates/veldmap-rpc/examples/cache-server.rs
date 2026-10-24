use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use connectrpc::{
    ConnectError, RequestContext, Response, Router, ServiceRequest, ServiceResult, ServiceStream,
};
use veldmap_proto::veldmap::cache::v1::{
    CACHE_PLACE_TILES_SPEC, CACHE_QUERY_TILES_SPEC, CACHE_SERVICE_NAME, Cache, CacheExt,
    CachedTile, CachedTileResult, PlaceTilesRequest, QueryTilesRequest,
};
use veldmap_proto::veldmap::common::v1::{ItemResult, TileAddr};
use veldmap_rpc::error::{DomainError, DomainErrorKind};
use veldmap_rpc::health::install_health;
use veldmap_rpc::stream::{BoundedSender, bounded_stream};
use veldmap_rpc::transport::{RouteClass, TransportResult, bind_loopback, with_route_class};

struct CacheExample;

#[allow(refining_impl_trait)]
impl Cache for CacheExample {
    async fn query_tiles(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, QueryTilesRequest>,
    ) -> ServiceResult<ServiceStream<CachedTileResult>> {
        let request = request.to_owned_message();
        let (sender, stream) = bounded_stream(NonZeroUsize::MIN);
        match request.level {
            0 => drop(tokio::spawn(async move {
                produce_finite(sender, request.tiles).await;
            })),
            1 => drop(tokio::spawn(async move {
                produce_long(sender).await;
            })),
            _ => return Err(ConnectError::invalid_argument("unknown example level")),
        }
        Response::stream_ok(stream)
    }

    async fn place_tiles(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, PlaceTilesRequest>,
    ) -> ServiceResult<ServiceStream<ItemResult>> {
        Err(ConnectError::unimplemented(
            "place tiles is outside this example",
        ))
    }
}

async fn produce_finite(sender: BoundedSender<CachedTileResult>, tiles: Vec<TileAddr>) {
    for addr in tiles {
        match finite_result(addr) {
            Ok(result) => {
                if sender.send(result).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.fail(error).await;
                return;
            }
        }
    }
}

async fn produce_long(sender: BoundedSender<CachedTileResult>) {
    for x in 0..1024 {
        if x != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if sender
            .send(cache_miss(TileAddr {
                x,
                y: 0,
                ..Default::default()
            }))
            .await
            .is_err()
        {
            marker("PRODUCER_TERMINATED");
            return;
        }
    }
}

fn finite_result(addr: TileAddr) -> Result<CachedTileResult, ConnectError> {
    if addr.x != 0 {
        return Ok(cache_miss(addr));
    }
    let id = format!("0/{}/{}", addr.x, addr.y);
    let error = DomainError::new(DomainErrorKind::NotFound, "example tile is absent")
        .into_item_error(id)?;
    Ok(CachedTileResult {
        outcome: error.into(),
        ..Default::default()
    })
}

fn cache_miss(addr: TileAddr) -> CachedTileResult {
    CachedTileResult {
        outcome: CachedTile {
            addr: addr.into(),
            hit: false,
            ..Default::default()
        }
        .into(),
        ..Default::default()
    }
}

fn marker(value: &str) {
    println!("{value}");
    io::stdout().flush().expect("failed to flush stdout");
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> TransportResult<()> {
    let router = Arc::new(CacheExample).register(Router::new());
    let router = with_route_class(router, CACHE_QUERY_TILES_SPEC.procedure, RouteClass::Bulk);
    let router = with_route_class(
        router,
        CACHE_PLACE_TILES_SPEC.procedure,
        RouteClass::Mutation,
    );
    let router = install_health(router, [CACHE_SERVICE_NAME]);
    let bound = bind_loopback(0).await?;
    marker(&format!("READY {}", bound.local_addr()?));
    bound.serve(router).await?;
    Ok(())
}
