# VeldMap

**VeldMap** is a high-precision 3D Earth rendering engine designed for embedding into existing applications (Rust, C++, Python, Qt, etc.).

Unlike traditional globes that use polygon meshes, VeldMap utilizes **Ray Marching** on the GPU to render the WGS84 ellipsoid with mathematically perfect curvature. This approach eliminates texture distortion at the poles and allows for sub-pixel accuracy when overlaying satellite imagery and terrain data (Copernicus DEM / EGM2008).

## Architecture

The project is structured to provide maximum portability and ease of integration:

### 1. Core Library (`src/lib.rs`)
The heart of the engine. It handles:
- **Ray Marching Renderer:** A high-performance WGPU-based pipeline that performs per-pixel ray-surface intersection.
- **Geophysical Math:** Exact WGS84 ellipsoid math, EGM2008 geoid offsets, and Copernicus DEM integration.
- **Data Streaming:** Asynchronous loading of terrain and imagery tiles directly into GPU textures.

### 2. Rust API (`src/api.rs`)
... (rest of the section) ...

## Features
- [x] WGPU integration & Cross-platform architecture.
- [x] FFI / C-API foundation.
- [ ] **High-Precision Ray Marching:** Per-pixel calculation of ray intersection with the WGS84 ellipsoid.
- [ ] **Terrain-Corrected Visualization:** Step-based ray marching through Copernicus DEM and EGM2008 data for perfect relief rendering.
- [ ] **Perspective-Correct Imagery:** Satellite images projected based on the true 3D intersection point, eliminating distortion and parallax errors.
- [ ] **Vector Overlays:** Points, lines, and polygons rendered with curvature awareness.


## Building
```bash
# Build library
cargo build --release

# Run example app
cargo run --release --bin veldmap-app
```
