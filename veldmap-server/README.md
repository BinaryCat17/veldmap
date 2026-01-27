# VeldMap Server

High-performance GIS backend for streaming planetary data.

## Purpose

The `veldmap-server` is responsible for serving massive datasets to `veldmap-data` clients. It handles the "heavy lifting" of data storage, tiling, and network transmission.

## Features

- **Tile Streaming:** Provides XYZ tiles for DEM, imagery, and vector data over HTTP.
- **On-the-fly Processing:** Can crop and reproject GeoTIFFs into tiles dynamically.
- **Global Coverage:** Designed to be backed by S3 or other object storage containing global Copernicus DEM and satellite datasets.
- **Zstd Compression:** Minimizes bandwidth usage for high-precision height data.
- **REST & gRPC:** Support for modern protocols to ensure low-latency data delivery.

## API Specification (Concept)

### Get Terrain Tile
`GET /v1/terrain/{z}/{x}/{y}.bin`
Returns raw Float32 data (or compressed) for the requested tile.

### Get Imagery Tile
`GET /v1/imagery/{z}/{x}/{y}.webp`
Returns satellite image tile.

## Deployment

Designed to run in Docker/Kubernetes, scaling horizontally to handle thousands of concurrent `veldmap` users.
