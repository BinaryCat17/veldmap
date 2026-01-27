# VeldMap Data Provider

This crate is a standalone, high-performance data management system for global 3D rendering. It is designed to work seamlessly with the **VeldMap** rendering engine or any other GIS application.

## Core Philosophy

`veldmap-data` acts as a **Local Tile Server** and **Cache Manager**. It abstracts away the complexity of where data comes from (Cloud, Local Disk, MBTiles, or procedurally generated) and provides a unified interface for requesting terrain (DEM), imagery, and vector data.

## Features

- **Hybrid Storage:** Seamlessly switches between local offline files and online cloud sources.
- **MBTiles Support:** Native support for MBTiles (SQLite) containers—the industry standard for offline maps.
- **Async Streaming:** Built on `tokio`, allowing for non-blocking data loading that won't freeze your UI.
- **Multi-Format:**
    - **DEM:** GeoTIFF (Copernicus), PGM (EGM2008), Raw floating point.
    - **Imagery:** PNG, JPEG, WEBP.
    - **Vector:** Mapbox Vector Tiles (MVT/PBF).
- **Proactive Caching:** Multi-layer caching (Memory L1, Disk L2) with LRU eviction policies.
- **Cross-Platform:** Pure Rust implementation—runs on Desktop, Mobile (Android/iOS via FFI), and Web (WASM).

## Architecture: Quadtree & XYZ

There is often confusion between **Quadtree** and **XYZ indexing**. In `veldmap-data`, we use both:

1.  **Quadtree (Logic):** We use a Quadtree structure to manage the Level of Detail (LOD). The tree decides *which* part of the world needs more detail based on the camera position.
2.  **XYZ (Addressing):** We use the standard `Z/X/Y` (Zoom, X-coordinate, Y-coordinate) scheme to address tiles within that Quadtree. This is the industry standard (used by Google, OSM, Bing) and ensures compatibility with almost all existing map data sources.

## Integration Example

```rust
use veldmap_data::{DataProvider, Config, TileId};

let provider = DataProvider::new(Config {
    cache_dir: "./map_cache",
    offline_mode: false,
});

// Request a tile asynchronously
let tile_id = TileId { z: 14, x: 8621, y: 5120 };
let dem_data = provider.get_dem_tile(tile_id).await?;
```

## Mobile & Offline Use

This crate is designed to be fully functional without an internet connection. By providing a pre-packaged `.mbtiles` or a directory of GeoTIFFs, you can deploy a full-scale global 3D map on a device that is completely offline.
