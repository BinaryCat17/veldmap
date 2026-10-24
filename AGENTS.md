# AGENTS.md — VeldMap WASI 0.2 & ConnectRPC Rewrite Plan

## 0. Agent Execution Protocol (CRITICAL — READ FIRST)

You are an autonomous **mimo v2.6 pro** engineering agent executing **one single task** from the Work Plan in Section 5.
Each chat session runs **statelessly without memory** of previous sessions.

1. **Locate Your Task:** Find the **first** task in Section 5 marked `- [ ]` (or the specific task number requested by the user).
2. **Scope Discipline:** Implement **only** that task. Do not jump ahead to future tasks.
3. **Strict Architectural & Code Purity Rules:**
   - **100% English Only:** Zero Cyrillic characters (`[\u0400-\u04FF]`) anywhere in source code, comments, UI labels, logs, docs, or commit messages.
   - **Self-Documenting Code & Extreme-Case-Only Comments:** Code must be self-documenting through clear naming, small single-responsibility functions, strong domain types/enums, and `thiserror` error types mapped to ConnectRPC status codes. **Strip all legacy `//!` file-header essays, multi-sentence `///` prose blocks, and narrative `//` comments.** Comments are permitted **ONLY** in three extreme cases:
     1. `// SAFETY:` invariants immediately above `unsafe` blocks (e.g., `openjp2` C FFI in `codec.rs`).
     2. 1–2 line notes explaining external protocol or OS bugs (e.g., CDSE S3 rejecting presigned query URLs and requiring SigV4 headers, or X11/WSLg HiDPI scale reporting).
     3. 1-line citations for non-trivial mathematical/geodetic standards (e.g., GOST R 51794-2008 Helmert parameters or Krüger's 4th-order series).
     Total comment lines in any `.rs` file must not exceed 5% of the file's lines.
   - **Concise, Non-Redundant Documentation:** Documentation in `README.md` and `docs/` must be brief (<= 80 lines per file), architectural, and never duplicate `.proto` message definitions or narrate code mechanics.
   - **No Legacy Host or Codegen:** Never reintroduce `veldcore/platform/host`, `veldsdk`, `buildgen/`, `schema.yaml`, `config.yaml`, `runner.yaml`, `host.yaml`, or `generated/` directories.
   - **No Tests During Rewrite:** Do not write `#[cfg(test)]` blocks, unit tests, `pytest` suites, or UI test scripts. Tests are intentionally omitted and will be rebuilt from scratch after the migration.
   - **All 7 Services Compile to WASM (`wasm32-wasip2`):** Every microservice under `services/` is a standard Rust binary crate compiled to `wasm32-wasip2` and orchestrated via `process-compose.yaml`.
   - **Modern Rust 2024 Layout & Strict Lints:** Use `edition = "2024"`, workspace dependency inheritance (`{ workspace = true }`), workspace lints (`{ workspace = true }`), and modern module paths (`src/foo.rs` + `src/foo/bar.rs`, **never** `src/foo/mod.rs`).
4. **Update State Constraint:** When your task compiles cleanly and passes verification, update **only** its checkbox in this `AGENTS.md` file from `- [ ]` to `- [x]`. Do **not** alter task descriptions or add session notes to `AGENTS.md`.

---

## 1. Target Workspace & Process Architecture

### 1.1 Repository Structure

```text
veldmap/
├── AGENTS.md
├── README.md                   # Concise <= 35-line project overview & quickstart
├── Cargo.toml                  # Single Cargo workspace ([workspace.dependencies], [workspace.lints])
├── process-compose.yaml        # Orchestrates all 7 WASI 0.2 services
├── .env.example                # COPERNICUS_ACCESS_KEY and COPERNICUS_ACCESS_SECRET
├── proto/                      # Clean ConnectRPC / gRPC contracts (Fleet benchmark style)
│   ├── common.proto            # Shared verbs, EventMeta, ReadOptions, TextureFrame, RasterSource
│   ├── provider.proto          # Stateless Copernicus CDSE adapter (finite item streams)
│   ├── library.proto           # Local download manager & disk register (infinite event streams)
│   ├── tiler.proto             # Stateless raster describe & pyramid tile producer (finite streams)
│   ├── cache.proto             # Disk QOI tile cache under runtime/data/tiles/ (finite streams)
│   ├── viewer.proto            # 2D raster preview canvas (infinite state & frame streams)
│   └── globe.proto             # 3D WGS84 ellipsoid & overlay renderer (infinite state & frame streams)
├── crates/
│   ├── veldmap-proto/          # Compiles proto/*.proto once with prost::bytes::Bytes + ConnectRPC stubs
│   ├── veldmap-rpc/            # Thin ConnectRPC/stream helpers (EventTable, stream_items, WASI sockets)
│   └── veldmap-core/           # Shared domain math: pyramid, tile, geodesy, wheel, range block pool
├── services/                   # 7 independent WASI 0.2 microservices (all target wasm32-wasip2)
│   ├── data-provider/          # Port 50051 — implements provider.Provider
│   ├── data-library/           # Port 50052 — implements library.Library
│   ├── image-tiler/            # Port 50053 — implements tiler.Tiler
│   ├── tile-cache/             # Port 50054 — implements cache.Cache
│   ├── image-view/             # Port 50055 — implements viewer.Viewer (wasi:webgpu + software fallback)
│   ├── globe/                  # Port 50056 — implements globe.Globe (wasi:webgpu + software fallback)
│   └── data-browser/           # Client of 50051..50056 — standard iced UI via wasi-gfx (wasi:surface)
├── runtime/
│   ├── config/                 # Per-service JSON configs (7 service files only)
│   ├── assets/                 # Fonts
│   ├── data/                   # Downloads (dem/source) & tile cache (tiles/)
│   ├── state/                  # Saved pane/tab layout (data-browser.json)
│   └── logs/                   # Service logs
└── docs/                       # Concise English documentation (<= 80 lines per file)
    ├── overview.md
    ├── services.md
    ├── imagery-and-globe.md
    └── operations.md
```

### 1.2 Running All 7 Services as WASI 0.2 Components (`wasi-gfx` + `wasmtime`)

1. **Headless Backend Services (`data-provider`, `data-library`, `image-tiler`, `tile-cache`):**
   Run under standard `wasmtime run -S http -S inherit-network=y --dir ./runtime::/runtime target/wasm32-wasip2/release/<service>.wasm`.
2. **GPU & Windowed Services (`image-view`, `globe`, `data-browser`):**
   - Compile to `wasm32-wasip2` using the `wasi-gfx` patches for `wgpu` and `winit` declared in the root `Cargo.toml` (`[patch.crates-io]`), which route `wgpu` to `wasi:webgpu` and `winit` to `wasi:surface` on `target_arch = "wasm32"` instead of browser `web-sys`.
   - Executed in `process-compose.yaml` via `wasi-gfx-runtime` (Wasmtime with `wasi-webgpu-wasmtime` and `wasi:surface` host bindings enabled).
   - Both `image-view` and `globe` also include a pure-Rust software rasterizer fallback in `veldmap-core::gpu_fallback` so they can render frames in headless environments without a physical GPU.

---

## 2. Helper Crate (`crates/veldmap-rpc`) & Large Texture Transfer Specification

### 2.1 Why `crates/veldmap-rpc` Exists (Thin Helpers, Zero Magic)

Do **not** build a macro DSL or custom ABI. `crates/veldmap-rpc` is a small, transparent library (~250 lines) that removes repetitive ConnectRPC/Tonic boilerplate across the 7 services:

1. **`RpcStream<T>` Type Alias & `stream_from_rx` Helper (for `provider.proto`, `tiler.proto`, `cache.proto`):**
   ```rust
   pub type RpcStream<T> = Pin<Box<dyn ConnectError Stream<Item="Result<T,">> + Send + 'static>>;

   pub fn stream_from_rx<T: 'static + Send>(
       rx: tokio::sync::mpsc::Receiver<Result<T, ConnectError>>,
   ) -> RpcStream<T>;
   ```
   Allows any handler to spawn a task and yield individual items or per-item ConnectRPC errors over a finite stream.

2. **`EventTable<V>` Helper (for `library.proto`, `viewer.proto`, `globe.proto`):**
   Implements the `common.ReadOptions` contract in one reusable struct:
   - Maintains current live entities (`BTreeMap<String, V>`) and a bounded ring-buffer of historical `(EventMeta, Option<V>)` mutations with sortable ULID `event_id` cursors and `Timestamp`s.
   - Provides `pub fn subscribe(&self, opts: ReadOptions) -> RpcStream<Event<V>>`:
     - If `opts.view == VIEW_SNAPSHOT` (or unspecified), immediately emits current items with `op = OP_SNAPSHOT`.
     - If `opts.end == live: true`, seamlessly transitions to streaming live `OP_CREATED`, `OP_UPDATED`, and `OP_DELETED` events via a `tokio::sync::broadcast` receiver until the client cancels the stream.

3. **`LatestFrameBroadcaster` Helper (for `viewer.proto` and `globe.proto` `WatchFrames`):**
   Wraps `tokio::sync::watch::Sender<TextureFrame>` and exposes `subscribe_frames(&self) -> RpcStream<TextureFrame>`.
   - **Backpressure guarantee:** If the producer renders at 144 fps and the UI client reads at 60 fps, `watch` automatically overwrites the unconsumed frame in-place so memory usage stays strictly bounded to 1 frame.

4. **Pre-Configured Server & Client Builders (`128 MiB` Frame Support):**
   `veldmap_rpc::server(service)` and `veldmap_rpc::client_channel(port)` pre-configure:
   - `max_decoding_message_size(128 * 1024 * 1024)` (128 MiB)
   - `max_encoding_message_size(128 * 1024 * 1024)` (128 MiB)
   - `TCP_NODELAY = true` on loopback sockets.

### 2.2 Large Textures & 4K Frames Over Sockets (`common.TextureFrame`)

To make bulk texture and frame transfers fast over loopback sockets today—and trivial for a future build-time zero-copy / `wgpu` handle optimizer to intercept—all pixel transfers obey four rules:

1. **`prost::bytes::Bytes` Everywhere:**
   In `crates/veldmap-proto/build.rs`, configure `prost_build::Config::new().bytes(["."])`. Every protobuf `bytes` field compiles to `bytes::Bytes` rather than `Vec<u8>`. Decoding a 33 MB frame from the network buffer performs a reference-counted `Bytes::slice` (zero `memcpy`).
2. **64 KiB Page-Aligned Pixel Buffers:**
   `veldmap_rpc::alloc_aligned_bytes(len: usize) -> BytesMut` allocates backing buffers aligned to 65,536 bytes (1 WASM page) before rendering or decoding pixels into them.
3. **Uncompressed `RGBA8` for Live Viewport Streams, `QOI` for Disk Cache:**
   - `globe` and `image-view` stream live viewport frames via `WatchFrames` using `PIXEL_FORMAT_RGBA8_SRGB` (raw 4 bytes/pixel, `stride = width * 4`). At 60–144 fps on localhost, raw RGBA8 avoids CPU compression bottlenecks and lets `data-browser` hand the `Bytes` directly to `iced::widget::image::Handle::from_rgba(width, height, frame.pixels)`.
   - `image-tiler` and `tile-cache` use `PIXEL_FORMAT_QOI` when storing/reading 512×512 pyramid tiles on disk under `/runtime/data/tiles/` and `PIXEL_FORMAT_RGBA8_UNORM` when streaming decoded tiles to `globe` and `image-view`.
4. **Stream Cancellation Replaces Epoch Kills:**
   When `data-browser`, `globe`, or `image-view` no longer needs an in-flight `ProduceTiles` or `WatchFrames` stream (e.g., the user panned away or closed a tab), the client simply drops the stream handle. The server checks `tx.is_closed()` between chunks/frames and aborts the work cleanly.

---

## 3. Benchmark ConnectRPC / Protobuf Contracts (`proto/*.proto`)

Every `.proto` file must follow the exact style of the `fleet` benchmark (`common.proto`, `provider.proto`, `host.proto`):
- Explicit `option idempotency_level = NO_SIDE_EFFECTS;` or `IDEMPOTENT;` where applicable.
- **Stateless services (`provider.proto`, `tiler.proto`, `cache.proto`)** return **finite `stream`** responses so each yielded item is validated/handled individually via standard ConnectRPC status codes—**no `string error` fields inside response messages**.
- **Stateful services (`library.proto`, `viewer.proto`, `globe.proto`)** expose `Place*` / `Remove*` returning `stream common.ItemResult`, and `List*` taking `common.ReadOptions` and returning **infinite `stream *Event`** (`common.EventMeta` + `optional T data`).

### 3.1 `proto/common.proto`

```proto
syntax = "proto3";

package common;

import "google/protobuf/timestamp.proto";

// ═══════════════════════════════════════════════════════════════════════════
// 1. GENERIC ITEM RESULTS

enum ItemResultCode {
  ITEM_RESULT_CODE_UNSPECIFIED = 0;
  ITEM_RESULT_CODE_DONE        = 1;
  ITEM_RESULT_CODE_CREATED     = 2;
  ITEM_RESULT_CODE_UPDATED     = 3;
  ITEM_RESULT_CODE_NO_ACTION   = 4;
  ITEM_RESULT_CODE_REPLACED    = 5;
}

message ItemResult {
  string id = 1;
  ItemResultCode code = 2;
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. PLACE & REMOVE OPTIONS

enum PlaceMode {
  PLACE_MODE_UNSPECIFIED = 0;
  PLACE_MODE_UPSERT      = 1;
  PLACE_MODE_CREATE      = 2;
  PLACE_MODE_UPDATE      = 3;
}

message PlaceOptions {
  PlaceMode mode = 1;
  bool dry_run = 2;
  string reason = 3;
}

message RemoveOptions {
  enum MissingPolicy {
    MISSING_POLICY_UNSPECIFIED = 0;
    MISSING_POLICY_OK          = 1;
    MISSING_POLICY_ERROR       = 2;
  }
  MissingPolicy missing = 1;
  bool dry_run = 2;
  string reason = 3;
}

message RemoveRequest {
  repeated string ids = 1;
  RemoveOptions options = 2;
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. LIST / READ (EVENT LOG & SNAPSHOT SUBSCRIPTIONS)

message ReadOptions {
  enum View {
    VIEW_UNSPECIFIED = 0;
    VIEW_SNAPSHOT    = 1;
    VIEW_EVENTS      = 2;
  }
  View view = 1;

  oneof start {
    string after_event_id = 2;
    google.protobuf.Timestamp after_time = 3;
  }

  oneof end {
    string before_event_id = 4;
    google.protobuf.Timestamp before_time = 5;
    bool live = 6;
  }

  uint32 max_events = 7;
}

message Ids {
  repeated string ids = 1;
}

enum Op {
  OP_UNSPECIFIED = 0;
  OP_SNAPSHOT    = 1;
  OP_CREATED     = 2;
  OP_UPDATED     = 3;
  OP_DELETED     = 4;
}

message EventMeta {
  string event_id = 1;
  string id = 2;
  google.protobuf.Timestamp time = 3;
  Op op = 4;
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. SHARED GEOMETRY, RASTER SOURCE & BULK TEXTURE BUFFERS

message GeoPoint {
  double lat = 1;
  double lon = 2;
}

message Ring {
  repeated GeoPoint points = 1;
}

message UtmFrame {
  uint32 zone = 1;
  bool south = 2;
  double x0 = 3;
  double y0 = 4;
  double x1 = 5;
  double y1 = 6;
}

message RasterSource {
  oneof location {
    string local_path = 1;     // Relative to /runtime/data/dem/source
    string s3_identifier = 2;  // Remote CDSE S3 key read via HTTP Range block pool
  }
}

message TileAddr {
  uint32 x = 1;
  uint32 y = 2;
}

enum PixelFormat {
  PIXEL_FORMAT_UNSPECIFIED  = 0;
  PIXEL_FORMAT_RGBA8_UNORM  = 1;
  PIXEL_FORMAT_RGBA8_SRGB   = 2;
  PIXEL_FORMAT_QOI          = 3;
}

message TextureFrame {
  string target_id = 1;        // View id or "globe"
  uint64 frame_seq = 2;        // Monotonic frame counter
  uint32 width = 3;
  uint32 height = 4;
  uint32 row_stride = 5;       // width * 4 for RGBA8
  PixelFormat format = 6;
  bytes pixels = 7;            // Zero-copy prost::bytes::Bytes, 64 KiB page-aligned
}
```

### 3.2 `proto/provider.proto` (Finite Streams — Stateless Adapter)

```proto
syntax = "proto3";

package provider;

import "common.proto";

// ═══════════════════════════════════════════════════════════════════════════
// BUSINESS CONTRACT — Stateless adapter over Copernicus Data Space (CDSE).
// Streams are finite lists: each element is returned or fails individually
// via ConnectRPC status codes without embedding error strings in messages.

service Provider {
  rpc SearchProducts (SearchProductsRequest) returns (stream DataProduct) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ListStoragePath (ListStoragePathRequest) returns (stream StorageEntry) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc LocateProducts (common.Ids) returns (stream DataProduct) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ResolveProductRoots (common.Ids) returns (stream ProductRoot) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc InspectImagery (InspectImageryRequest) returns (stream ImageryLayout) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc SignObjects (common.Ids) returns (stream SignedObject) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message SearchProductsFilter {
  optional string mission = 1;
  optional string name_contains = 2;
  optional common.Ring area = 3;
  optional int64 acquired_from_unix_ms = 4;
  optional int64 acquired_to_unix_ms = 5;
  optional double max_cloud_cover = 6;
}

message SearchProductsRequest {
  SearchProductsFilter filter = 1;
  uint32 limit = 2;
}

message Part {
  string id = 1;
  string name = 2;
  string product_type = 3;
  uint64 size_bytes = 4;
  bool folder = 5;
  bool shown = 6;
  optional string unviewable_reason = 7;
}

message DataProduct {
  string id = 1;
  string name = 2;
  int64 acquired_at_unix_ms = 3;
  uint64 size_bytes = 4;
  repeated common.Ring footprint = 5;
  string product_type = 6;
  optional double cloud_cover = 7;
  bool folder = 8;
  repeated Part parts = 9;
  optional string unviewable_reason = 10;
}

message ListStoragePathRequest {
  string path = 1;
  string page_token = 2;
  bool recursive = 3;
}

message StorageEntry {
  string key = 1;
  uint64 size_bytes = 2;
  int64 modified_at_unix_ms = 3;
  string product_id = 4;
  optional string unviewable_reason = 5;
  optional string next_page_token = 6;
}

message ProductRoot {
  string id = 1;
  string root_id = 2;
}

message InspectImageryRequest {
  string product_id = 1;
  bool downloaded = 2;
  optional string wanted_file = 3;
}

enum ImageryRole {
  IMAGERY_ROLE_UNSPECIFIED = 0;
  IMAGERY_ROLE_PREVIEW     = 1;
  IMAGERY_ROLE_DETAILED    = 2;
}

message ImageryRaster {
  string id = 1;
  ImageryRole role = 2;
  optional string geolocation_id = 3;
}

message ImageryLayout {
  string product_id = 1;
  repeated ImageryRaster rasters = 2;
  optional common.UtmFrame utm = 3;
  optional string unviewable_reason = 4;
  optional string wanted_refused_reason = 5;
}

message SignedObject {
  string id = 1;
  string url = 2;
  map<string, string> headers = 3;
}
```

### 3.3 `proto/library.proto` (Infinite Event Streams — Stateful Register)

```proto
syntax = "proto3";

package library;

import "common.proto";

// ═══════════════════════════════════════════════════════════════════════════
// BUSINESS CONTRACT — Stateful download manager & local disk register.

service Library {
  rpc PlaceDownloads (PlaceDownloadsRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PauseDownloads (common.Ids) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveDownloads (common.RemoveRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PlaceSceneCounts (PlaceSceneCountsRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ListDownloads (ListDownloadsRequest) returns (stream DownloadEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message DownloadSpec {
  string id = 1;          // S3 identifier
  string product_id = 2;  // Scene root identifier
}

message PlaceDownloadsRequest {
  repeated DownloadSpec items = 1;
  common.PlaceOptions options = 2;
}

message SceneCount {
  string product_id = 1;
  uint32 total_files = 2;
}

message PlaceSceneCountsRequest {
  repeated SceneCount items = 1;
  common.PlaceOptions options = 2;
}

enum DownloadStatus {
  DOWNLOAD_STATUS_UNSPECIFIED = 0;
  DOWNLOAD_STATUS_PAUSED      = 1;
  DOWNLOAD_STATUS_DOWNLOADING = 2;
  DOWNLOAD_STATUS_COMPLETE    = 3;
  DOWNLOAD_STATUS_ERROR       = 4;
}

message DownloadItem {
  string id = 1;          // Relative path on disk
  string source_id = 2;   // S3 identifier
  uint64 done_bytes = 3;
  uint64 total_bytes = 4;
  DownloadStatus status = 5;
  int64 modified_at_unix_ms = 6;
  string product_id = 7;
  uint32 sibling_count = 8;
  optional string status_message = 9;
}

message ListDownloadsRequest {
  common.ReadOptions options = 1;
  optional common.Ids filter_to_products = 2;
}

message DownloadEvent {
  common.EventMeta meta = 1;
  optional DownloadItem data = 2;
}
```

### 3.4 `proto/tiler.proto` & `proto/cache.proto` (Finite Streams — Stateless Workers)

```proto
syntax = "proto3";

package tiler;

import "common.proto";

service Tiler {
  rpc DescribeRasters (DescribeRastersRequest) returns (stream RasterDescription) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ProduceTiles (ProduceTilesRequest) returns (stream ProduceEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message DescribeRasterItem {
  string id = 1;
  common.RasterSource source = 2;
  optional common.RasterSource geolocation = 3;
  optional string variable_path = 4;
}

message DescribeRastersRequest {
  repeated DescribeRasterItem items = 1;
}

enum ServeMode {
  SERVE_MODE_UNSPECIFIED = 0;
  SERVE_MODE_POINTWISE   = 1;
  SERVE_MODE_PASS        = 2;
}

message PyramidLevel {
  ServeMode serve = 1;
  uint32 from_level = 2;
  uint64 peak_bytes = 3;
  bool fits_memory = 4;
}

message GeoTie {
  double px = 1;
  double py = 2;
  double lat = 3;
  double lon = 4;
}

message Placement {
  uint32 epsg = 1;
  double x_per_i = 2;
  double x_per_j = 3;
  double x0 = 4;
  double y_per_i = 5;
  double y_per_j = 6;
  double y0 = 7;
}

message RasterVariable {
  string path = 1;
  string label = 2;
  string units = 3;
}

message RasterDescription {
  string id = 1;
  string fingerprint = 2;
  uint32 width = 3;
  uint32 height = 4;
  uint32 tile_side = 5;
  repeated PyramidLevel levels = 6;
  repeated GeoTie ties = 7;
  optional Placement placement = 8;
  optional string binding_warning = 9;
  optional RasterVariable active_variable = 10;
  repeated RasterVariable candidate_variables = 11;
}

message ProduceTilesRequest {
  common.RasterSource source = 1;
  uint32 level = 2;
  repeated common.TileAddr tiles = 3;
  optional string variable_path = 4;
}

message ProducedTile {
  uint32 level = 1;
  uint32 x = 2;
  uint32 y = 3;
  uint32 width = 4;
  uint32 height = 5;
  common.PixelFormat format = 6;
  bytes pixels = 7;
}

message ProduceProgress {
  uint64 read_bytes = 1;
  uint64 total_bytes = 2;
  uint32 done_tiles = 3;
  uint32 wanted_tiles = 4;
}

message ProduceEvent {
  oneof event {
    ProducedTile tile = 1;
    ProduceProgress progress = 2;
  }
}
```

```proto
syntax = "proto3";

package cache;

import "common.proto";

service Cache {
  rpc QueryTiles (QueryTilesRequest) returns (stream CachedTile) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc PlaceTiles (PlaceTilesRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }
}

message QueryTilesRequest {
  string fingerprint = 1;
  uint32 level = 2;
  repeated common.TileAddr tiles = 3;
}

message CachedTile {
  common.TileAddr addr = 1;
  bool hit = 2;
  uint32 width = 3;
  uint32 height = 4;
  common.PixelFormat format = 5;
  bytes pixels = 6;
}

message TileRecord {
  string id = 1;
  string fingerprint = 2;
  uint32 level = 3;
  uint32 x = 4;
  uint32 y = 5;
  bytes qoi = 6;
}

message PlaceTilesRequest {
  repeated TileRecord items = 1;
  common.PlaceOptions options = 2;
}
```

### 3.5 `proto/viewer.proto` & `proto/globe.proto` (Infinite Event & Frame Streams)

```proto
syntax = "proto3";

package viewer;

import "common.proto";
import "tiler.proto";

service Viewer {
  rpc PlaceViews (PlaceViewsRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveViews (common.RemoveRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ControlCamera (CameraControlRequest) returns (stream common.ItemResult);

  rpc ListViews (ListViewsRequest) returns (stream ViewEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc WatchFrames (WatchFramesRequest) returns (stream common.TextureFrame) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message ViewSpec {
  string id = 1;
  common.RasterSource source = 2;
  string label = 3;
  uint32 viewport_width = 4;
  uint32 viewport_height = 5;
  optional string variable_path = 6;
}

message PlaceViewsRequest {
  repeated ViewSpec items = 1;
  common.PlaceOptions options = 2;
}

message CameraControlRequest {
  string view_id = 1;
  oneof action {
    bool fit = 2;
    ZoomAt zoom_at = 3;
    Pan pan = 4;
  }
}

message ZoomAt {
  float x = 1;
  float y = 2;
  float factor = 3;
}

message Pan {
  float dx = 1;
  float dy = 2;
}

message ViewStatus {
  string id = 1;
  uint32 source_width = 2;
  uint32 source_height = 3;
  double scale = 4;
  uint64 read_bytes = 5;
  uint64 total_bytes = 6;
  bool working = 7;
  uint32 ready_tiles = 8;
  uint32 total_tiles = 9;
  uint32 step = 10;
  uint32 total_steps = 11;
  optional string warning_message = 12;
  optional tiler.RasterVariable active_variable = 13;
  repeated tiler.RasterVariable candidate_variables = 14;
}

message ListViewsRequest {
  common.ReadOptions options = 1;
  optional common.Ids filter_to_views = 2;
}

message ViewEvent {
  common.EventMeta meta = 1;
  optional ViewStatus data = 2;
}

message WatchFramesRequest {
  string view_id = 1;
}
```

```proto
syntax = "proto3";

package globe;

import "common.proto";
import "tiler.proto";

service Globe {
  rpc PlaceOutlines (PlaceOutlinesRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveOutlines (common.RemoveRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PlaceOverlays (PlaceOverlaysRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveOverlays (common.RemoveRequest) returns (stream common.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ControlCamera (GlobeCameraRequest) returns (stream common.ItemResult);

  rpc ProbeSurface (ProbeSurfaceRequest) returns (stream ProbeSurfacePoint) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ListOverlays (ListOverlaysRequest) returns (stream OverlayEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc WatchFrames (WatchGlobeFramesRequest) returns (stream common.TextureFrame) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

enum OutlineStyle {
  OUTLINE_STYLE_UNSPECIFIED = 0;
  OUTLINE_STYLE_PLAIN       = 1;
  OUTLINE_STYLE_SELECTED    = 2;
  OUTLINE_STYLE_PENDING     = 3;
}

message OutlineItem {
  string id = 1;
  repeated common.GeoPoint points = 2;
  OutlineStyle style = 3;
}

message PlaceOutlinesRequest {
  repeated OutlineItem items = 1;
  common.PlaceOptions options = 2;
}

enum OverlayRole {
  OVERLAY_ROLE_UNSPECIFIED = 0;
  OVERLAY_ROLE_PREVIEW     = 1;
  OVERLAY_ROLE_DETAILED    = 2;
}

message OverlayRaster {
  common.RasterSource source = 1;
  OverlayRole role = 2;
  optional common.RasterSource geolocation = 3;
  uint32 ordinal = 4;
  optional string variable_path = 5;
}

message OverlaySpec {
  string id = 1;
  string label = 2;
  optional common.UtmFrame utm = 3;
  repeated common.GeoPoint quad = 4;
  bool rough_footprint = 5;
  repeated OverlayRaster rasters = 6;
  optional float opacity = 7;
  bool hidden = 8;
  uint32 z_order = 9;
}

message PlaceOverlaysRequest {
  repeated OverlaySpec items = 1;
  common.PlaceOptions options = 2;
}

message GlobeCameraRequest {
  oneof action {
    Orbit orbit = 1;
    Zoom zoom = 2;
    Focus focus = 3;
    ViewportResize resize = 4;
  }
}

message Orbit {
  float dx = 1;
  float dy = 2;
}

message Zoom {
  float delta = 1;
  float hold_x = 2;
  float hold_y = 3;
}

message Focus {
  common.GeoPoint at = 1;
  double radius_deg = 2;
}

message ViewportResize {
  uint32 width = 1;
  uint32 height = 2;
}

message ProbeSurfaceRequest {
  float x = 1;
  float y = 2;
}

message ProbeSurfacePoint {
  optional common.GeoPoint at = 1;
}

message OverlayStatus {
  string id = 1;
  uint32 ready_tiles = 2;
  uint32 total_tiles = 3;
  bool working = 4;
  float progress_share = 5;
  uint32 step = 6;
  uint32 total_steps = 7;
  bool describing = 8;
  uint64 pass_read_bytes = 9;
  uint64 pass_total_bytes = 10;
  optional uint32 detailed_ordinal = 11;
  optional string detailed_warning = 12;
  optional string general_warning = 13;
  optional tiler.RasterVariable detailed_variable = 14;
  repeated tiler.RasterVariable detailed_candidates = 15;
}

message ListOverlaysRequest {
  common.ReadOptions options = 1;
  optional common.Ids filter_to_overlays = 2;
}

message OverlayEvent {
  common.EventMeta meta = 1;
  optional OverlayStatus data = 2;
}

message WatchGlobeFramesRequest {}
```

---

## 4. Direct `iced` Architecture for `services/data-browser` (No `ui-service`)

`veldmodules/ui-service` is **deleted**. `services/data-browser` uses standard `iced 0.14` directly:
- **Panes & Tabs:** Either `iced::widget::pane_grid` or the existing recursive split tree (`state/layout.rs`) rendered using standard `iced::widget::{row, column, container, scrollable, text, text_input, button, slider, progress_bar, tooltip, mouse_area, image}`.
- **Live Globe & Image Viewports:** Rendered via `iced::widget::mouse_area(iced::widget::image(handle))` where `handle` is updated from the `WatchFrames` stream (`iced::widget::image::Handle::from_rgba(frame.width, frame.height, frame.pixels)`).
- **Async Subscriptions:** `iced::Subscription::run` subscribes to `Library.ListDownloads`, `Viewer.ListViews`, `Viewer.WatchFrames`, `Globe.ListOverlays`, and `Globe.WatchFrames`.

---

## 5. Sequential Work Plan for Mimo v2.6 Pro Agents

Execute **one** task per session. Mark `- [x]` **only** when the verification command succeeds.

- [x] **Task 1: Purge Legacy Host, Codegen, `ui-service`, Tests, and Cyrillic**
  - **Action:**
    1. Delete `veldcore/`, `buildgen/`, `uitests/`, `workspace.yaml`, and `veldmodules/ui-service/`.
    2. Delete every `schema.yaml`, `config.yaml`, `types.proto`, `wraps/`, and `generated/` directory inside `veldmodules/`.
    3. Strip all `#[cfg(test)]` blocks and test files from remaining code in `veldmodules/`.
    4. Scan the entire repository for Cyrillic characters (`[\u0400-\u04FF]`) and translate any remaining strings or comments into clear English.
  - **Verification:** `python3 -c "import pathlib, re; assert not any(re.search(r'[\u0400-\u04FF]', p.read_text(errors='ignore')) for p in pathlib.Path('.').rglob('*') if p.is_file() and '.git' not in p.parts)"` exits 0.

- [x] **Task 2: Purify Codebase Comments, Purge Legacy Docs/Configs, and Modernize Module Paths**
  - **Action:**
    1. Delete obsolete documentation, scripts, and legacy host configs:
       - Delete `CLAUDE.md`, `docs/architecture/`, `docs/decisions/`, `docs/operations/`, `docs/glossary.md`, `docs/limitations.md`, and `docs/roadmap.md`.
       - Delete `runtime/config/core.json`, `runtime/config/services.json`, `runtime/config/ui-service.json`, and `scripts/`.
    2. Rewrite `README.md` into a concise (<= 35 lines) English overview of the WASI 0.2 (`wasm32-wasip2`) + ConnectRPC architecture matching Section 1.
    3. Replace all legacy `mod.rs` files in `veldmodules/` with Rust 2024 module files (`veldmodules/data-browser/src/{components,handlers,state,view}.rs` and `veldmodules/image-tiler/src/adapters.rs`) and delete the `mod.rs` files.
    4. Purify all `.rs` files in `veldmodules/` to enforce self-documenting code:
       - Remove all `//!` module-level essay headers and historical chronicles.
       - Remove all verbose `///` and `//` narrative comments that restate code mechanics or legacy bus/ABI rules.
       - Retain comments **only** for `// SAFETY:` invariants on `unsafe` blocks (`codec.rs`), 1-line citations of geodetic/mathematical formulas, or 1-line external S3/OS quirk notes (<= 5% comment lines per file).
  - **Verification:** `python3 -c "import pathlib; assert not pathlib.Path('CLAUDE.md').exists(); assert not pathlib.Path('docs/roadmap.md').exists(); assert not pathlib.Path('runtime/config/core.json').exists(); assert not list(pathlib.Path('.').rglob('mod.rs')); rs = list(pathlib.Path('veldmodules').rglob('*.rs')); assert all(not any(l.strip().startswith('//!') for l in p.read_text().splitlines()) and sum(1 for l in p.read_text().splitlines() if l.strip().startswith('//')) <= max(3, int(len(p.read_text().splitlines()) * 0.05)) for p in rs)"` exits 0.

- [ ] **Task 3: Create Root Cargo Workspace, `.proto` Contracts, and `crates/veldmap-proto`**
  - **Action:**
    1. Create `proto/common.proto`, `proto/provider.proto`, `proto/library.proto`, `proto/tiler.proto`, `proto/cache.proto`, `proto/viewer.proto`, and `proto/globe.proto` matching Section 3 verbatim.
    2. Create the root `Cargo.toml` (`edition = "2024"`, `[workspace.dependencies]`, and strict `[workspace.lints]` denying `unsafe_op_in_unsafe_fn`, `dbg_macro`, and `todo`).
    3. Create `crates/veldmap-proto` with a `build.rs` that compiles all 7 `.proto` files using `prost-build` (`config.bytes(["."])` so all `bytes` fields use zero-copy `prost::bytes::Bytes`) and generates ConnectRPC / Tonic service traits and clients.
  - **Verification:** `cargo check -p veldmap-proto` and `cargo check -p veldmap-proto --target wasm32-wasip2` succeed with zero warnings.

- [ ] **Task 4: Create `crates/veldmap-rpc` and `crates/veldmap-core`**
  - **Action:**
    1. Create `crates/veldmap-rpc` implementing the stream and socket helpers specified in Section 2.1 (`RpcStream<T>`, `stream_from_rx`, `EventTable<V>` supporting `ReadOptions` `VIEW_SNAPSHOT`/`VIEW_EVENTS`/ULID cursors, `LatestFrameBroadcaster`, `alloc_aligned_bytes`, and 128 MiB message limits).
    2. Create `crates/veldmap-core` migrating and modernizing the shared domain modules with self-documenting types and `thiserror` error enums:
       - `src/pyramid.rs` (from `veldmodules/image-tiler/src/pyramid.rs`)
       - `src/tile.rs` (from `veldmodules/tile-cache/src/tile.rs`)
       - `src/geodesy.rs` and `src/wheel.rs` (from `veldmodules/globe/src/`)
       - `src/tiles.rs` (storing `bytes::Bytes` RGBA8 tiles under the VRAM budget, pyramid ladder, and active pass state)
       - `src/range.rs` (64 KiB block pool, readahead on consumed blocks, prefetch runs, local file + remote HTTP Range reader)
       - `src/config.rs` (loads `/runtime/config/<service>.json` and expands `${VAR}` from `.env`)
       - `src/gpu_fallback.rs` (pure-Rust software rasterizer fallback for headless 2D/3D frame rendering).
  - **Verification:** `cargo clippy -p veldmap-rpc -p veldmap-core --target wasm32-wasip2 -- -D warnings` succeeds.

- [ ] **Task 5: Migrate `services/data-provider` to WASI 0.2 & ConnectRPC (`provider.proto`)**
  - **Action:**
    1. Move `veldmodules/data-provider` to `services/data-provider` as a `wasm32-wasip2` binary crate (`src/main.rs`, `src/catalogue.rs`, `src/s3.rs`, `src/scene.rs`, `src/imagery.rs`, `src/manifest.rs`, `src/mgrs.rs`, `src/time.rs`) and delete `veldmodules/data-provider`.
    2. Implement `provider::Provider` on `127.0.0.1:50051` using `veldmap-rpc` finite streams and structured `thiserror` errors mapped to ConnectRPC status codes.
    3. Preserve all OData search queries, `FRESH_DAYS` fallback widening, scene/part folding, S3 SigV4 signing, manifest parsing, and MGRS UTM frame math with clean, self-documenting code.
  - **Verification:** `cargo build -p veldmap-data-provider --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/data-provider`.

- [ ] **Task 6: Migrate `services/tile-cache` to WASI 0.2 & ConnectRPC (`cache.proto`)**
  - **Action:**
    1. Move `veldmodules/tile-cache` to `services/tile-cache` as a `wasm32-wasip2` binary crate serving `cache::Cache` on `127.0.0.1:50054` and delete `veldmodules/tile-cache`.
    2. Read/write QOI tiles directly under `/runtime/data/tiles/` using standard `std::fs` / `tokio::fs` (atomic `.tmp` write + rename) and LRU budget eviction (`cache_limit_mb`).
  - **Verification:** `cargo build -p veldmap-tile-cache --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/tile-cache`.

- [ ] **Task 7: Migrate `services/data-library` to WASI 0.2 & ConnectRPC (`library.proto`)**
  - **Action:**
    1. Move `veldmodules/data-library` to `services/data-library` as a `wasm32-wasip2` binary crate serving `library::Library` on `127.0.0.1:50052` and delete `veldmodules/data-library`.
    2. Implement the HTTP `.part` resumable downloader (`src/download.rs`) directly in `services/data-library`.
    3. Use `ProviderClient` (`127.0.0.1:50051`) for `SignObjects` and `ResolveProductRoots`.
    4. Maintain `.origin` sidecar files under `/runtime/data/dem/source` and publish live state changes through `veldmap_rpc::EventTable<DownloadItem>` on `ListDownloads`.
  - **Verification:** `cargo build -p veldmap-data-library --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/data-library`.

- [ ] **Task 8: Migrate `services/image-tiler` to WASI 0.2 & ConnectRPC (`tiler.proto`)**
  - **Action:**
    1. Move `veldmodules/image-tiler` to `services/image-tiler` as a `wasm32-wasip2` binary crate serving `tiler::Tiler` on `127.0.0.1:50053` (`src/adapters.rs` + `src/adapters/*.rs`) and delete `veldmodules/image-tiler`.
    2. Preserve all format adapters (`tiff`, `jp2`/`openjp2`, `excerpt`, `codec`, `jpeg`, `png`, `netcdf`/`hdf5-pure`, `grid`, `table`, `radiometry`, `resample`, `fingerprint`, `budget`) with self-documenting code and `thiserror` error handling.
    3. Open `common.RasterSource` via `veldmap_core::range` (calling `ProviderClient::sign_objects` for remote S3 keys).
    4. Stream produced tiles and progress over `ProduceTiles`, and asynchronously store QOI tiles to `CacheClient` (`127.0.0.1:50054`). Abort the pass immediately if the client drops the stream.
  - **Verification:** `cargo build -p veldmap-image-tiler --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/image-tiler`.

- [ ] **Task 9: Migrate `services/image-view` to WASI 0.2 & ConnectRPC (`viewer.proto`)**
  - **Action:**
    1. Move `veldmodules/image-view` to `services/image-view` as a `wasm32-wasip2` binary crate serving `viewer::Viewer` on `127.0.0.1:50055` and delete `veldmodules/image-view`.
    2. Use `TilerClient` (`50053`), `CacheClient` (`50054`), and `veldmap_core::tiles` pyramid ladder mechanics.
    3. Render the 2D canvas into 64 KiB page-aligned `Bytes` (`PIXEL_FORMAT_RGBA8_SRGB`), broadcasting frames via `LatestFrameBroadcaster` (`WatchFrames`) and status updates via `EventTable<ViewStatus>` (`ListViews`).
  - **Verification:** `cargo build -p veldmap-image-view --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/image-view`.

- [ ] **Task 10: Migrate `services/globe` to WASI 0.2 & ConnectRPC (`globe.proto`)**
  - **Action:**
    1. Move `veldmodules/globe` to `services/globe` as a `wasm32-wasip2` binary crate serving `globe::Globe` on `127.0.0.1:50056` and delete `veldmodules/globe`.
    2. Preserve all WGS84 ellipsoid geometry, `SURFACE_LIFT_M`, warp mesh sag subdivision, frustum/horizon ball + 9-probe culling, 4-rank georeference bindings, Transverse Mercator / Pulkovo 1942 Helmert datum math, outlines/ribbon/hatching, and quaternion slerp flights.
    3. Render the 3D scene into 64 KiB page-aligned `Bytes` (`PIXEL_FORMAT_RGBA8_SRGB`), streaming frames via `LatestFrameBroadcaster` (`WatchFrames`) and overlay progress via `EventTable<OverlayStatus>` (`ListOverlays`).
  - **Verification:** `cargo build -p veldmap-globe --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules/globe`.

- [ ] **Task 11: Rewrite `services/data-browser` with Direct `iced` (`wasm32-wasip2`)**
  - **Action:**
    1. Move `veldmodules/data-browser` to `services/data-browser` using modern Rust 2024 module paths (`src/state.rs`, `src/handlers.rs`, `src/view.rs`, `src/components.rs`) and delete `veldmodules/` completely.
    2. Rewrite the UI to use standard `iced 0.14` widgets directly (no `ui-service`), displaying `globe` and `image-view` `TextureFrame` streams via `iced::widget::image::Handle::from_rgba` inside `mouse_area`.
    3. Connect to `50051..50056` via `veldmap-proto` clients and `iced::Subscription::run`.
    4. Preserve all panes/tabs, layout persistence (`/runtime/state/data-browser.json`), catalogue/search/download tables, row states, batch actions, and NetCDF variable selectors.
  - **Verification:** `cargo build -p veldmap-data-browser --target wasm32-wasip2 --release` succeeds and `! test -d veldmodules`.

- [ ] **Task 12: Create `process-compose.yaml` and Concise Documentation (`docs/`)**
  - **Action:**
    1. Create `process-compose.yaml` at the repository root orchestrating all 7 `.wasm` services (`data-provider`, `tile-cache`, `data-library`, `image-tiler`, `image-view`, `globe`, `data-browser`) with readiness probes and dependency ordering.
    2. Create four concise English documents in `docs/` (strictly <= 80 lines per file, referencing `.proto` contracts instead of duplicating them): `docs/overview.md`, `docs/services.md`, `docs/imagery-and-globe.md`, and `docs/operations.md`.
  - **Verification:** `cargo clippy --workspace --target wasm32-wasip2 -- -D warnings` and `cargo build --workspace --target wasm32-wasip2 --release` succeed with zero errors.
