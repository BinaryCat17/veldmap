use axum::{
    extract::Path,
    routing::get,
    Router,
    response::IntoResponse,
};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // Инициализация логирования
    tracing_subscriber::fmt::init();

    // Создание роутера
    let app = Router::new()
        .route("/", get(|| async { "VeldMap Data Server is running" }))
        .route("/v1/terrain/:z/:x/:y", get(get_terrain_tile))
        .route("/v1/imagery/:z/:x/:y", get(get_imagery_tile));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_terrain_tile(Path((z, x, y)): Path<(u32, u32, u32)>) -> impl IntoResponse {
    tracing::info!("Requested terrain tile: Z={}, X={}, Y={}", z, x, y);
    // В будущем: чтение из S3 или локального кэша GeoTIFF-ов
    "Terrain tile data placeholder"
}

async fn get_imagery_tile(Path((z, x, y)): Path<(u32, u32, u32)>) -> impl IntoResponse {
    tracing::info!("Requested imagery tile: Z={}, X={}, Y={}", z, x, y);
    // В будущем: получение спутниковых снимков
    "Imagery tile data placeholder"
}
