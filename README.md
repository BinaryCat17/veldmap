# VeldMap - Modular 3D Earth Rendering Engine

VeldMap is a high-performance, modular 3D Earth rendering engine written in Rust. It is designed to be easily embedded into applications on various platforms (Desktop, Web/WASM, Mobile) using a unified interface.

## Project Structure

The project is organized as a Cargo Workspace with the following modules:

*   **`veldmap-core`**: The central hub defining abstract interfaces (Traits), shared data types, and FFI bindings via [UniFFI](https://github.com/mozilla/uniffi-rs). It acts as the "Source of Truth" for the entire system.
*   **`veldmap-render`**: A GPU-accelerated rendering module based on `wgpu`. It implements the `Renderer` interface from `core`.
*   **`veldmap-data`**: A data provider module that handles loading Digital Elevation Models (DEM) and imagery (GeoTIFF, etc.). It implements `TerrainProvider` and `ImageryProvider` interfaces.
*   **`veldmap-geo-math`**: A module implementing precise geographic math (WGS84 ellipsoid conversions) implementing the `GeoMath` interface.
*   **`veldmap-app`**: A standalone demo application (`winit` + `wgpu`) showcasing how to assemble and use the engine components.
*   **`veldmap-server`**: A high-level data server based on `axum` that serves DEM tiles and geoid data over HTTP.

## Architecture

VeldMap follows an **Interface-First** approach:

1.  **Core Interfaces**: All interactions between modules happen through traits defined in `veldmap-core`.
2.  **Private Implementations**: Modules like `render` and `data` are private and expose only a single **Factory Function** to create an instance of their interface (e.g., `create_renderer()`, `create_data_provider()`).
3.  **UniFFI**: The core interfaces are decorated with `#[uniffi::export]`, allowing automatic generation of bindings for Kotlin, Swift, Python, and other languages.

## Getting Started

### Prerequisites

*   Rust (latest stable)
*   Vulkan / Metal / DX12 compatible GPU

### Running the Demo App

To run the demonstration application which renders a 3D view of the Earth:

```bash
cargo run -p veldmap-app
```

### Running Tests

```bash
cargo test
```

## Module Details

### veldmap-core
Defines `TileId`, `DemTile`, and traits:
*   `Renderer`: `render()`, `resize()`, `update()`, `camera_move()`, etc.
*   `TerrainProvider`: `get_tile()`, `get_geoid()`.
*   `GeoMath`: `lat_lon_to_ecef()`, `ecef_to_lat_lon()`.

### veldmap-geo-math
Implements precise WGS84 ellipsoid math. Z-axis points to the North Pole (ECEF).

### veldmap-render
Implements the `Renderer` trait using `wgpu`. It handles:
*   Ray-marching of the terrain.
*   Virtual texturing (indirection textures).
*   Camera control.

### veldmap-data
Implements `TerrainProvider`. Supports:
*   GeoTIFF loading for DEM.
*   Local file caching.
*   PGM format for Geoids (EGM2008).

## License

[License Name]
