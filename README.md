# VeldMap

Geospatial imagery over Copernicus Data Space: catalogue search, local downloads,
2D raster views and a 3D WGS84 globe.

## Transitional architecture

The new modular application uses ordinary typed Rust calls:

- `apps/veldmap` — native and browser entry points and composition.
- `crates/veldmap-core` — domain and application logic.
- `crates/veldmap-render` — rendering and GPU resources.
- `crates/veldmap-storage` — persistence adapters.

`veldmodules/` remains temporarily as migration input, outside the Cargo workspace.
The legacy host, protobuf, RPC, WASI, and Wasmtime scaffolding has been removed.
The current entry points are minimal; product flows migrate in later tasks.

## Build and verify

```bash
cargo test --workspace --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --target wasm32-unknown-unknown --locked
cargo run -p veldmap --locked
```

The workspace uses stable Rust `1.99.1`, Rust 2024, and a committed lockfile.
See `docs/operations.md` for exact pins and verification commands. Runtime state
lives under `runtime/`; see `.env.example` for CDSE downloads.
