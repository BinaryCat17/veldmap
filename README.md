# VeldMap

Geospatial imagery workspace over Copernicus Data Space: catalogue search,
local download library, 2D raster canvas and 3D WGS84 globe.

## Architecture

Seven Rust microservices compile to WASI 0.2 components (`wasm32-wasip2`)
and talk over ConnectRPC on localhost sockets:

| Service | Port | Contract |
|---|---|---|
| `data-provider` | 50051 | `provider.Provider` — stateless Copernicus CDSE adapter |
| `data-library` | 50052 | `library.Library` — download manager and disk register |
| `image-tiler` | 50053 | `tiler.Tiler` — raster describe and pyramid tiles |
| `tile-cache` | 50054 | `cache.Cache` — QOI tile disk cache |
| `image-view` | 50055 | `viewer.Viewer` — 2D raster canvas |
| `globe` | 50056 | `globe.Globe` — 3D WGS84 ellipsoid |
| `data-browser` | — | `iced` UI client of all services (`wasi-gfx`) |

- `proto/` — ConnectRPC contracts: stateless services stream finite lists,
  stateful ones stream event logs and live frames.
- `crates/veldmap-proto` — prost messages and stubs, zero-copy `bytes::Bytes`.
- `crates/veldmap-rpc` — stream, event-table and frame-broadcast helpers.
- `crates/veldmap-core` — pyramid tiles, geodesy, ranged readers, GPU fallback.

## Build and run

```bash
cargo build --workspace --target wasm32-wasip2 --release
process-compose up
```

Runtime state lives under `runtime/`; set `COPERNICUS_ACCESS_KEY` and
`COPERNICUS_ACCESS_SECRET` in `.env` (see `.env.example`) for downloads.
