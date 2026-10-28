use std::error::Error;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::time::Duration;

use veldmap_proto::veldmap::cache::v1::__buffa::oneof::cached_tile_result::Outcome;
use veldmap_proto::veldmap::cache::v1::{
    CACHE_SERVICE_NAME, CacheClient, CachedTileResult, QueryTilesRequest,
};
use veldmap_proto::veldmap::common::v1::{ItemErrorCode, TileAddr};
use veldmap_rpc::health::{HealthCheckRequest, HealthClient, ServingStatus};
use veldmap_rpc::transport::{RouteClass, call_options, client_config, plaintext_client};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult<()> {
    let address = std::env::args()
        .nth(1)
        .ok_or("usage: cache-client <socket-address>")?
        .parse::<SocketAddr>()?;
    let health = HealthClient::new(
        plaintext_client(),
        client_config(format!("http://{address}").parse()?, REQUEST_TIMEOUT),
    );
    let cache = CacheClient::new(
        plaintext_client(),
        client_config(format!("http://{address}").parse()?, REQUEST_TIMEOUT),
    );

    let response = health
        .check(HealthCheckRequest {
            service: CACHE_SERVICE_NAME.into(),
            ..Default::default()
        })
        .await?
        .into_owned();
    assert_eq!(response.status, ServingStatus::SERVING);

    let mut finite = cache
        .query_tiles_with_options(
            request(
                0,
                vec![
                    TileAddr {
                        x: 0,
                        y: 0,
                        ..Default::default()
                    },
                    TileAddr {
                        x: 1,
                        y: 0,
                        ..Default::default()
                    },
                ],
            ),
            call_options(RouteClass::Bulk),
        )
        .await?;
    let first = finite
        .message::<CachedTileResult>()
        .await?
        .ok_or("finite stream ended before its item error")?
        .to_owned_message();
    let Some(Outcome::Error(error)) = first.outcome else {
        return Err("first finite result was not an item error".into());
    };
    assert_eq!(error.id, "0/0/0");
    assert_eq!(error.code, ItemErrorCode::NotFound);

    let second = finite
        .message::<CachedTileResult>()
        .await?
        .ok_or("finite stream ended after its item error")?
        .to_owned_message();
    let Some(Outcome::Value(tile)) = second.outcome else {
        return Err("second finite result was not a cache item".into());
    };
    assert!(!tile.hit);
    assert_eq!(
        tile.addr.as_option().map(|addr| (addr.x, addr.y)),
        Some((1, 0))
    );
    assert!(finite.message::<CachedTileResult>().await?.is_none());
    marker("CACHE_STREAM_OK")?;

    let mut active = cache
        .query_tiles_with_options(
            request(
                1,
                vec![TileAddr {
                    x: 0,
                    y: 0,
                    ..Default::default()
                }],
            ),
            call_options(RouteClass::Bulk),
        )
        .await?;
    let first = active
        .message::<CachedTileResult>()
        .await?
        .ok_or("long stream ended before its first item")?
        .to_owned_message();
    assert!(matches!(first.outcome, Some(Outcome::Value(_))));
    tokio::time::sleep(Duration::from_millis(20)).await;
    marker("CACHE_TIMER_OK")?;
    drop(active);
    marker("CACHE_CANCEL_OK")?;
    Ok(())
}

fn request(level: u32, tiles: Vec<TileAddr>) -> QueryTilesRequest {
    QueryTilesRequest {
        fingerprint: "example".into(),
        level,
        tiles,
        ..Default::default()
    }
}

fn marker(value: &str) -> io::Result<()> {
    println!("{value}");
    io::stdout().flush()
}
