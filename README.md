# VeldMap

Geospatial imagery over Copernicus Data Space: catalogue search, local downloads,
2D raster views and a 3D WGS84 globe.

## WASI 0.3 roadmap

The workspace targets `wasm32-wasip3` with generated ConnectRPC contracts.
`host/veldmap-host` is the native Wasmtime embedder with per-component socket
and HTTP egress policies. Reference services remain separate components with
independent rendering state; the later composed profile may optimize measured
GPU and transport costs without changing service behavior.

- `proto/` — authoritative protobuf and streaming contracts.
- `crates/veldmap-proto` — Buffa messages, JSON, and ConnectRPC stubs.
- `crates/veldmap-rpc` — shared transport, stream, health, and error helpers.
- `host/veldmap-host` — native WASI 0.3 command-component runner.
- `scripts/check_component_imports.py` — artifact interface inspection.

## Build and verify

```bash
cargo test --workspace --locked
cargo build -p veldmap-rpc --target wasm32-wasip3 --examples --features http-example --locked
python3 scripts/check_component_imports.py target/wasm32-wasip3/debug/examples/*.wasm
```

Run a component with its configured policy:

```bash
cargo run -p veldmap-host --locked -- target/wasm32-wasip3/debug/examples/cache-server.wasm
```

See `docs/operations.md` for exact tool pins, HTTP verification, and launch details.
Runtime state lives under `runtime/`; see `.env.example` for future CDSE downloads.
