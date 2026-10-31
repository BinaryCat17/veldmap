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
The current application proves shared eframe/wgpu rendering on native and browser;
product flows migrate in later tasks.

## Build and verify

```bash
cargo test --workspace --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --target wasm32-unknown-unknown --locked
cargo run -p veldmap --locked
```

See `docs/operations.md` for toolchain pins and verification commands. `plan.md`
contains the temporary migration contract and clean-artifact limits. Runtime state
lives under `runtime/`; see `.env.example` for CDSE downloads.
