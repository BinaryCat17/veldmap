# plan.md — VeldMap WASI, ConnectRPC, and Buffa Rewrite Plan

## 0. Agent Execution Protocol

Each chat session executes one task from Section 5 and treats this file as the migration contract.

1. Locate the first unchecked task, or the task explicitly requested by the user.
2. Implement only that task. Do not jump ahead or silently shrink its verification.
3. Follow these rules in every task:
   - Write English only in source, comments, UI text, logs, documentation, and commit messages. Do not introduce Cyrillic characters (`[\u0400-\u04FF]`).
   - Use self-documenting names, small functions, strong domain types, and `thiserror` errors. Keep comments exceptional: unsafe invariants, external protocol/OS quirks, or one-line mathematical citations. Keep comments below 5% of lines in each Rust file.
   - Do not restore `veldcore/platform/host`, `veldsdk`, `buildgen/`, `schema.yaml`, legacy host configs, `generated/` directories, or the `ui-service` process/ABI.
   - Use Rust 2024, workspace dependency inheritance, workspace lints, and modern module paths (`src/foo.rs` plus `src/foo/bar.rs`, not `src/foo/mod.rs`).
   - Compile all seven debug services to `wasm32-wasip2`. Use current-thread Tokio and the WASI configuration from Section 1.3.
   - Add only the minimum essential tests for externally observable behavior and non-obvious invariants. Do not test getters, trivial arithmetic or wiring, or dependency behavior, and do not duplicate integration coverage with unit tests. Geodesy, projection, codecs, cache eviction, resumable downloads, event replay, and stream cancellation require correctness tests; compilation alone is not a migration verification.
   - Keep README at 35 lines or fewer and documents under `docs/` at 80 lines or fewer. Reference protobuf contracts instead of duplicating their field definitions.
4. Run the complete verification for the current task, including its runtime scenario.
5. Only after verification passes, change that task's checkbox from `- [ ]` to `- [x]`. Do not add session notes or alter other task descriptions.
6. If an external runtime assumption fails, record the evidence and revise the roadmap before building later tasks on that assumption.

---

## 1. Target Architecture

### 1.1 Two Configurations, One Domain Implementation

The project has two configurations with the same service behavior:

- **Debug configuration:** seven independent WASI 0.2 command components orchestrated by `process-compose.yaml`. They communicate through native ConnectRPC over loopback TCP. Socket copies and raw RGBA frame transfer are acceptable in this configuration because they make isolation, cancellation, logs, and failures easy to inspect.
- **Composed release configuration:** one optimized binary built from the same domain libraries after the debug system works. Direct domain calls or a standard ConnectRPC `ClientTransport` replacement remove process overhead where measurements justify it. GPU resource sharing, WASM rewriting, and socket replacement are optional optimizations, not assumptions required by the initial design.

Every service crate exposes its domain logic through a library and keeps its WASI executable entry point thin. ConnectRPC handlers are adapters, not owners of business logic. No macro DSL or custom socket ABI is required.

### 1.2 Repository Structure

```text
veldmap/
├── plan.md
├── README.md
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── .cargo/config.toml
├── process-compose.yaml
├── .env.example
├── proto/
│   ├── common.proto
│   ├── provider.proto
│   ├── library.proto
│   ├── tiler.proto
│   ├── cache.proto
│   ├── viewer.proto
│   ├── globe.proto
│   └── health.proto
├── crates/
│   ├── veldmap-proto/
│   ├── veldmap-rpc/
│   └── veldmap-core/
├── services/
│   ├── data-provider/
│   ├── data-library/
│   ├── image-tiler/
│   ├── tile-cache/
│   ├── image-view/
│   ├── globe/
│   └── data-browser/
├── apps/
│   └── veldmap/                 # Added only by Task 15
├── runtime/
│   ├── config/
│   ├── assets/
│   ├── data/
│   ├── state/
│   └── logs/
└── docs/
    ├── overview.md
    ├── services.md
    ├── imagery-and-globe.md
    └── operations.md
```

Service ports in the debug configuration:

| Service | Port | Contract |
|---|---:|---|
| `data-provider` | 50051 | `veldmap.provider.v1.Provider` |
| `data-library` | 50052 | `veldmap.library.v1.Library` |
| `image-tiler` | 50053 | `veldmap.tiler.v1.Tiler` |
| `tile-cache` | 50054 | `veldmap.cache.v1.Cache` |
| `image-view` | 50055 | `veldmap.viewer.v1.Viewer` |
| `globe` | 50056 | `veldmap.globe.v1.Globe` |
| `data-browser` | — | ConnectRPC client and presentation shell |

### 1.3 WASI and Dependency Baseline

- Use stable Rust with `rust-version = "1.88"`, a pinned `rust-toolchain.toml`, Rust 2024, and a committed `Cargo.lock`.
- Use `connectrpc` 0.9.x, `connectrpc-build`, `buffa`, `buffa-types`, and matching `buffa-codegen`. Pin compatible versions through the lockfile.
- Use `wasm32-wasip2` for the first complete system. WASI 0.3 is a later evaluation item because its native async model is attractive but its Rust target and the required graphics stack must be validated together.
- Configure WASI Tokio networking explicitly in `.cargo/config.toml`:

```toml
[target.wasm32-wasip2]
rustflags = ["--cfg", "tokio_unstable"]
```

- Use a current-thread Tokio runtime in every WASI executable. Do not enable `rt-multi-thread` for WASI components.
- Disable ConnectRPC's native Zstd feature. Enable only the JSON, gzip, client, and server features required by each crate. Keep generated JSON support consistent with the runtime features across the workspace.
- Headless services run under Wasmtime with loopback networking and only the filesystem directories they need. Do not grant every service write access to all of `runtime/`.
- GUI and GPU services run with the pinned `wasi-gfx` host runtime. Pin `wasi-gfx`, `wgpu`, windowing, and related forks by exact revision; do not track moving branches.
- `protoc`, Wasmtime, `process-compose`, and the selected `wasi-gfx-runtime` are documented development prerequisites.

### 1.4 Presentation and Rendering

`data-browser` owns the window/surface, input, clipboard integration supported by the runtime, fonts, and final wgpu presentation. It uses standard iced 0.14 widgets. The preferred implementation is a small WASI shell around the supported iced wgpu renderer; use lower-level `iced_core`/`iced_widget` integration only where the standard shell cannot support the pinned WASI runtime.

`image-view` and `globe` own their 2D/3D rendering state. In the debug configuration they send latest RGBA frames over ConnectRPC and `data-browser` uploads and composites those frames. In a composed configuration they may render into shared GPU resources without changing their domain behavior.

The previous `master:veldmodules/ui-service` code is a reference for surface lifecycle, glyph atlases, scissoring, and external-texture compositing. Do not restore its process, proto UI DSL, host ABI, resource-grant API, or unsafe thread-local implementation. Migrate only behavior that is still needed and compatible with iced 0.14.

Both renderer services retain pure-Rust software fallbacks for headless verification. A renderer fallback is not a substitute for proving the actual `data-browser` window and input path.

---

## 2. ConnectRPC, Streams, and Bulk Data

### 2.1 `veldmap-proto`

`crates/veldmap-proto` compiles all seven application schemas and the standard `grpc.health.v1` contract in one `connectrpc-build` invocation. Its `build.rs`:

- uses `connectrpc_build::Config` with `proto/` as the include root and an include file in `OUT_DIR`;
- enables generated views and the required client/server stubs;
- configures `buffa_codegen::BytesRepr::Bytes` for protobuf `bytes` fields so owned payloads use `buffa::bytes::Bytes`;
- emits a file descriptor set for diagnostics and health/reflection integration when needed;
- writes no generated source into the repository.

The crate uses `connectrpc::include_generated!()` and exports the versioned packages. Generated code is rebuilt by Cargo; there is no legacy codegen directory.

Buffa views borrow request storage and must not be stored in event tables, download state, or other long-lived structures. Convert them to owned messages at the service boundary. `Bytes` provides cheap ownership, cloning, and slicing; it does not guarantee zero network copies or page alignment.

### 2.2 Error Semantics

There are two deliberately different error channels:

1. **Recoverable item failure:** the remaining items are still meaningful. Return `common.ItemError` inside the response's `oneof`. A batch mutation returns `common.ItemResult` containing either `ItemSuccess` or `ItemError`. A finite data stream uses its service-specific result envelope. `ProduceEvent` has a dedicated error variant for a failed tile. These failures do not terminate the stream.
2. **RPC or stream failure:** return `ConnectError` and terminate the RPC. Use this for malformed requests, authentication failure, unavailable upstream services, expired replay cursors, source-wide failures, cancellation, deadlines, and failures after which later items cannot be trusted.

Finite streams end normally after their last item. Infinite subscriptions end on client cancellation, service shutdown, or a fatal RPC error. No contract claims that a stream can never terminate.

`ItemError.id` correlates the failure with the requested product, raster, object, view, or tile key. `message` is concise, English, safe to display, and does not contain credentials or presigned URLs. `retryable` describes the specific item failure; clients still apply bounded retries with backoff.

Domain state such as an unviewable product or a failed background download remains data in its status message. It is not converted into an RPC failure merely because the operation failed asynchronously.

### 2.3 `veldmap-rpc`

Keep this crate small and explicit:

- ConnectRPC server/client construction for loopback, current-thread Tokio, request timeouts, route-specific message limits, and standard health checks. Static readiness requires `Health.Check`; `Health.Watch` may return `UNIMPLEMENTED` until a consumer requires it.
- Bounded finite-stream helpers over `tokio::sync::mpsc`. Producers check cancellation between tiles, files, and frames. Ordinary work queues remain bounded and never silently drop tiles or mutations.
- `EventTable<V>` with owned live values, a bounded mutation history, monotonic ULID cursors, timestamps, and atomic snapshot-plus-subscribe behavior. A snapshot/live transition must not lose or duplicate a mutation.
- Explicit replay behavior: an unknown or evicted `after_event_id` returns `OUT_OF_RANGE`; time filters and `max_events` are applied consistently; deletion events omit data.
- Conversion between domain `thiserror` errors, recoverable `ItemError` values, and terminal `ConnectError` codes.
- Keep transport and stream helpers independent of images and other resource types. Resource validation belongs in shared domain libraries; extract only generic mechanisms with actual consumers.

Configure 128 MiB only for routes that carry frames or large tile payloads. Keep smaller limits for catalogue, mutation, and control routes. Enable `TCP_NODELAY` on loopback sockets where the selected transport exposes it.

### 2.4 Frames, Tiles, and Backpressure

- Live viewport frames use `PIXEL_FORMAT_RGBA8_SRGB`, `row_stride = width * 4`, and checked `pixels.len() == row_stride * height`. Implement shared validation in `veldmap-core` for both renderer services; do not add an aligned-`BytesMut` API without a real allocation mechanism and benchmark.
- Decoded pyramid tiles use `PIXEL_FORMAT_RGBA8_UNORM`; disk cache payloads use `PIXEL_FORMAT_QOI`.
- Cache tile keys include fingerprint, level, x, and y. `ProduceTilesRequest.fingerprint` must match the current `DescribeRasters` fingerprint before work starts.
- Dropping a ConnectRPC stream cancels work. A new camera/view request starts a new RPC; there is no epoch-kill side channel.
- `image-view` and `globe` own per-target `watch` channels with optional initial frames rather than fabricated empty images, and overwrite stale frames. Prove these semantics through the renderer stream scenarios rather than isolated channel tests.
- `watch` limits queued frame count, not bandwidth. Measure debug frame throughput and memory instead of promising 60–144 fps.
- The composed release may replace frame bytes with internal GPU handles. Such handles are not added to protobuf and are never treated as portable WASI resources outside the composition boundary.

---

## 3. Protobuf Contracts

All schemas use proto3, versioned packages, and the idempotency annotations shown below. Item-result wrappers are part of the contract, not optional implementation conventions.

### 3.1 `proto/common.proto`

```proto
syntax = "proto3";

package veldmap.common.v1;

import "google/protobuf/timestamp.proto";

enum ItemResultCode {
  ITEM_RESULT_CODE_UNSPECIFIED = 0;
  ITEM_RESULT_CODE_DONE        = 1;
  ITEM_RESULT_CODE_CREATED     = 2;
  ITEM_RESULT_CODE_UPDATED     = 3;
  ITEM_RESULT_CODE_NO_ACTION   = 4;
  ITEM_RESULT_CODE_REPLACED    = 5;
}

message ItemSuccess {
  string id = 1;
  ItemResultCode code = 2;
}

enum ItemErrorCode {
  ITEM_ERROR_CODE_UNSPECIFIED        = 0;
  ITEM_ERROR_CODE_INVALID_ARGUMENT   = 1;
  ITEM_ERROR_CODE_NOT_FOUND          = 2;
  ITEM_ERROR_CODE_ALREADY_EXISTS     = 3;
  ITEM_ERROR_CODE_FAILED_PRECONDITION = 4;
  ITEM_ERROR_CODE_PERMISSION_DENIED  = 5;
  ITEM_ERROR_CODE_UNAUTHENTICATED    = 6;
  ITEM_ERROR_CODE_RESOURCE_EXHAUSTED = 7;
  ITEM_ERROR_CODE_UNAVAILABLE        = 8;
  ITEM_ERROR_CODE_INTERNAL           = 9;
  ITEM_ERROR_CODE_DATA_LOSS          = 10;
}

message ItemError {
  string id = 1;
  ItemErrorCode code = 2;
  string message = 3;
  bool retryable = 4;
}

message ItemResult {
  oneof outcome {
    ItemSuccess success = 1;
    ItemError error = 2;
  }
}

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
    string local_path = 1;
    string s3_identifier = 2;
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
  string target_id = 1;
  uint64 frame_seq = 2;
  uint32 width = 3;
  uint32 height = 4;
  uint32 row_stride = 5;
  PixelFormat format = 6;
  bytes pixels = 7;
}
```

`ReadOptions` defaults to a snapshot. `live = true` continues into live events after the selected snapshot/history. `max_events = 0` means no application-level event limit; a positive value ends the stream normally after that many events.

### 3.2 `proto/provider.proto`

```proto
syntax = "proto3";

package veldmap.provider.v1;

import "common.proto";

service Provider {
  rpc SearchProducts (SearchProductsRequest) returns (stream DataProductResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ListStoragePath (ListStoragePathRequest) returns (stream StorageEntryResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc LocateProducts (veldmap.common.v1.Ids) returns (stream DataProductResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ResolveProductRoots (veldmap.common.v1.Ids) returns (stream ProductRootResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc InspectImagery (InspectImageryRequest) returns (stream ImageryLayoutResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc SignObjects (veldmap.common.v1.Ids) returns (stream SignedObjectResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message SearchProductsFilter {
  optional string mission = 1;
  optional string name_contains = 2;
  optional veldmap.common.v1.Ring area = 3;
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
  repeated veldmap.common.v1.Ring footprint = 5;
  string product_type = 6;
  optional double cloud_cover = 7;
  bool folder = 8;
  repeated Part parts = 9;
  optional string unviewable_reason = 10;
}

message DataProductResult {
  oneof outcome {
    DataProduct value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
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

message StorageEntryResult {
  oneof outcome {
    StorageEntry value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
}

message ProductRoot {
  string id = 1;
  string root_id = 2;
}

message ProductRootResult {
  oneof outcome {
    ProductRoot value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
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
  optional veldmap.common.v1.UtmFrame utm = 3;
  optional string unviewable_reason = 4;
  optional string wanted_refused_reason = 5;
}

message ImageryLayoutResult {
  oneof outcome {
    ImageryLayout value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
}

message SignedObject {
  string id = 1;
  string url = 2;
  map<string, string> headers = 3;
}

message SignedObjectResult {
  oneof outcome {
    SignedObject value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
}
```

### 3.3 `proto/library.proto`

```proto
syntax = "proto3";

package veldmap.library.v1;

import "common.proto";

service Library {
  rpc PlaceDownloads (PlaceDownloadsRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PauseDownloads (veldmap.common.v1.Ids) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveDownloads (veldmap.common.v1.RemoveRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PlaceSceneCounts (PlaceSceneCountsRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ListDownloads (ListDownloadsRequest) returns (stream DownloadEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message DownloadSpec {
  string id = 1;
  string product_id = 2;
}

message PlaceDownloadsRequest {
  repeated DownloadSpec items = 1;
  veldmap.common.v1.PlaceOptions options = 2;
}

message SceneCount {
  string product_id = 1;
  uint32 total_files = 2;
}

message PlaceSceneCountsRequest {
  repeated SceneCount items = 1;
  veldmap.common.v1.PlaceOptions options = 2;
}

enum DownloadStatus {
  DOWNLOAD_STATUS_UNSPECIFIED = 0;
  DOWNLOAD_STATUS_PAUSED      = 1;
  DOWNLOAD_STATUS_DOWNLOADING = 2;
  DOWNLOAD_STATUS_COMPLETE    = 3;
  DOWNLOAD_STATUS_ERROR       = 4;
}

message DownloadItem {
  string id = 1;
  string source_id = 2;
  uint64 done_bytes = 3;
  uint64 total_bytes = 4;
  DownloadStatus status = 5;
  int64 modified_at_unix_ms = 6;
  string product_id = 7;
  uint32 sibling_count = 8;
  optional string status_message = 9;
}

message ListDownloadsRequest {
  veldmap.common.v1.ReadOptions options = 1;
  optional veldmap.common.v1.Ids filter_to_products = 2;
}

message DownloadEvent {
  veldmap.common.v1.EventMeta meta = 1;
  optional DownloadItem data = 2;
}
```

### 3.4 `proto/tiler.proto`

```proto
syntax = "proto3";

package veldmap.tiler.v1;

import "common.proto";

service Tiler {
  rpc DescribeRasters (DescribeRastersRequest) returns (stream RasterDescriptionResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ProduceTiles (ProduceTilesRequest) returns (stream ProduceEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message DescribeRasterItem {
  string id = 1;
  veldmap.common.v1.RasterSource source = 2;
  optional veldmap.common.v1.RasterSource geolocation = 3;
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

message RasterDescriptionResult {
  oneof outcome {
    RasterDescription value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
}

message ProduceTilesRequest {
  veldmap.common.v1.RasterSource source = 1;
  uint32 level = 2;
  repeated veldmap.common.v1.TileAddr tiles = 3;
  optional string variable_path = 4;
  string fingerprint = 5;
}

message ProducedTile {
  uint32 level = 1;
  uint32 x = 2;
  uint32 y = 3;
  uint32 width = 4;
  uint32 height = 5;
  veldmap.common.v1.PixelFormat format = 6;
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
    veldmap.common.v1.ItemError error = 3;
  }
}
```

### 3.5 `proto/cache.proto`

```proto
syntax = "proto3";

package veldmap.cache.v1;

import "common.proto";

service Cache {
  rpc QueryTiles (QueryTilesRequest) returns (stream CachedTileResult) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc PlaceTiles (PlaceTilesRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }
}

message QueryTilesRequest {
  string fingerprint = 1;
  uint32 level = 2;
  repeated veldmap.common.v1.TileAddr tiles = 3;
}

message CachedTile {
  veldmap.common.v1.TileAddr addr = 1;
  bool hit = 2;
  uint32 width = 3;
  uint32 height = 4;
  veldmap.common.v1.PixelFormat format = 5;
  bytes pixels = 6;
}

message CachedTileResult {
  oneof outcome {
    CachedTile value = 1;
    veldmap.common.v1.ItemError error = 2;
  }
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
  veldmap.common.v1.PlaceOptions options = 2;
}
```

### 3.6 `proto/viewer.proto`

```proto
syntax = "proto3";

package veldmap.viewer.v1;

import "common.proto";
import "tiler.proto";

service Viewer {
  rpc PlaceViews (PlaceViewsRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveViews (veldmap.common.v1.RemoveRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ControlCamera (CameraControlRequest) returns (veldmap.common.v1.ItemResult);

  rpc ListViews (ListViewsRequest) returns (stream ViewEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc WatchFrames (WatchFramesRequest) returns (stream veldmap.common.v1.TextureFrame) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }
}

message ViewSpec {
  string id = 1;
  veldmap.common.v1.RasterSource source = 2;
  string label = 3;
  uint32 viewport_width = 4;
  uint32 viewport_height = 5;
  optional string variable_path = 6;
}

message PlaceViewsRequest {
  repeated ViewSpec items = 1;
  veldmap.common.v1.PlaceOptions options = 2;
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
  optional veldmap.tiler.v1.RasterVariable active_variable = 13;
  repeated veldmap.tiler.v1.RasterVariable candidate_variables = 14;
}

message ListViewsRequest {
  veldmap.common.v1.ReadOptions options = 1;
  optional veldmap.common.v1.Ids filter_to_views = 2;
}

message ViewEvent {
  veldmap.common.v1.EventMeta meta = 1;
  optional ViewStatus data = 2;
}

message WatchFramesRequest {
  string view_id = 1;
}
```

### 3.7 `proto/globe.proto`

```proto
syntax = "proto3";

package veldmap.globe.v1;

import "common.proto";
import "tiler.proto";

service Globe {
  rpc PlaceOutlines (PlaceOutlinesRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveOutlines (veldmap.common.v1.RemoveRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc PlaceOverlays (PlaceOverlaysRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc RemoveOverlays (veldmap.common.v1.RemoveRequest) returns (stream veldmap.common.v1.ItemResult) {
    option idempotency_level = IDEMPOTENT;
  }

  rpc ControlCamera (GlobeCameraRequest) returns (veldmap.common.v1.ItemResult);

  rpc ProbeSurface (ProbeSurfaceRequest) returns (ProbeSurfacePoint) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc ListOverlays (ListOverlaysRequest) returns (stream OverlayEvent) {
    option idempotency_level = NO_SIDE_EFFECTS;
  }

  rpc WatchFrames (WatchGlobeFramesRequest) returns (stream veldmap.common.v1.TextureFrame) {
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
  repeated veldmap.common.v1.GeoPoint points = 2;
  OutlineStyle style = 3;
}

message PlaceOutlinesRequest {
  repeated OutlineItem items = 1;
  veldmap.common.v1.PlaceOptions options = 2;
}

enum OverlayRole {
  OVERLAY_ROLE_UNSPECIFIED = 0;
  OVERLAY_ROLE_PREVIEW     = 1;
  OVERLAY_ROLE_DETAILED    = 2;
}

message OverlayRaster {
  veldmap.common.v1.RasterSource source = 1;
  OverlayRole role = 2;
  optional veldmap.common.v1.RasterSource geolocation = 3;
  uint32 ordinal = 4;
  optional string variable_path = 5;
}

message OverlaySpec {
  string id = 1;
  string label = 2;
  optional veldmap.common.v1.UtmFrame utm = 3;
  repeated veldmap.common.v1.GeoPoint quad = 4;
  bool rough_footprint = 5;
  repeated OverlayRaster rasters = 6;
  optional float opacity = 7;
  bool hidden = 8;
  uint32 z_order = 9;
}

message PlaceOverlaysRequest {
  repeated OverlaySpec items = 1;
  veldmap.common.v1.PlaceOptions options = 2;
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
  veldmap.common.v1.GeoPoint at = 1;
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
  optional veldmap.common.v1.GeoPoint at = 1;
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
  optional veldmap.tiler.v1.RasterVariable detailed_variable = 14;
  repeated veldmap.tiler.v1.RasterVariable detailed_candidates = 15;
}

message ListOverlaysRequest {
  veldmap.common.v1.ReadOptions options = 1;
  optional veldmap.common.v1.Ids filter_to_overlays = 2;
}

message OverlayEvent {
  veldmap.common.v1.EventMeta meta = 1;
  optional OverlayStatus data = 2;
}

message WatchGlobeFramesRequest {}
```

---

## 4. `data-browser` and Composed Release

The browser uses standard iced widgets for panes, tabs, tables, forms, selectors, progress, and viewport interaction. It subscribes to `Library.ListDownloads`, `Viewer.ListViews`, `Viewer.WatchFrames`, `Globe.ListOverlays`, and `Globe.WatchFrames`. Latest frame messages update GPU image resources; input updates camera RPCs and local pane state.

The presentation spike in Task 5 must prove the runtime before full UI migration. It includes a real surface, text, keyboard and pointer input, resize/HiDPI handling, and an animated external image. The full browser in Task 13 preserves the existing recursive pane layout and state persistence without restoring the UI proto DSL.

Task 15 builds the composed release from service libraries. Start with direct domain calls or standard ConnectRPC transport substitution while preserving request validation and error semantics. Then benchmark RPC-heavy work, tile production, frame delivery, memory use, and startup. Add shared textures, binary rewriting, or socket replacement only behind an explicit design review and measured benefit. The debug ConnectRPC configuration remains available for diagnostics.

---

## 5. Sequential Work Plan

- [x] **Task 1: Purge Legacy Host, Codegen, `ui-service`, Tests, and Cyrillic**
  - **Action:**
    1. Delete `veldcore/`, `buildgen/`, `uitests/`, `workspace.yaml`, and `veldmodules/ui-service/`.
    2. Delete legacy `schema.yaml`, `config.yaml`, `types.proto`, `wraps/`, and `generated/` paths inside `veldmodules/`.
    3. Remove obsolete tests from the legacy codebase; later tasks rebuild focused tests against the new architecture.
    4. Translate all remaining source, comments, UI labels, and documentation into English.
  - **Verification:** `python3 -c "import pathlib, re, subprocess; files = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], text=True).splitlines(); assert not any(re.search(r'[\u0400-\u04FF]', data.decode()) for name in files if b'\0' not in (data := pathlib.Path(name).read_bytes()))"` exits 0.

- [x] **Task 2: Purify Comments, Purge Legacy Docs/Configs, and Modernize Module Paths**
  - **Action:**
    1. Delete obsolete docs, scripts, and host configs listed in the original migration record.
    2. Rewrite `README.md` into a concise English overview.
    3. Replace legacy `mod.rs` files with Rust 2024 module paths.
    4. Remove narrative Rust comments while retaining only the exceptional categories from Section 0.
  - **Verification:** `python3 -c "import pathlib; assert not pathlib.Path('CLAUDE.md').exists(); assert not pathlib.Path('docs/roadmap.md').exists(); assert not pathlib.Path('runtime/config/core.json').exists(); assert not list(pathlib.Path('.').rglob('mod.rs')); rs = list(pathlib.Path('veldmodules').rglob('*.rs')); assert all(not any(l.strip().startswith('//!') for l in p.read_text().splitlines()) and sum(1 for l in p.read_text().splitlines() if l.strip().startswith('//')) <= max(3, int(len(p.read_text().splitlines()) * 0.05)) for p in rs)"` exits 0.

- [x] **Task 3: Create the Workspace, ConnectRPC Contracts, and `veldmap-proto`**
  - **Action:**
    1. Create `rust-toolchain.toml`, `.cargo/config.toml`, and the root Rust 2024 workspace with resolver 3, MSRV 1.88, shared dependencies, and strict lints denying `unsafe_op_in_unsafe_fn`, `dbg_macro`, and `todo`.
    2. Create the seven schemas in Section 3 with versioned packages and typed non-terminal item errors.
    3. Create `crates/veldmap-proto` with one `connectrpc-build` generation pass, generated views and service stubs, `BytesRepr::Bytes` for byte payloads, an `OUT_DIR` include file, and a descriptor set.
    4. Create a temporary `src/main.rs` to check success/error variants, event options, byte payload types, and Buffa encode/decode round trips. Run it with `cargo run -p veldmap-proto`, then delete the entry point; do not add `#[cfg(test)]`, test files, test targets, or test dependencies. Retain `Cargo.lock`; do not add generated source directories.
  - **Verification:** `cargo fmt --all -- --check`, the temporary `cargo run -p veldmap-proto` contract-check scenario with its entry point removed afterward, `cargo clippy -p veldmap-proto --all-targets -- -D warnings`, and `cargo clippy -p veldmap-proto --all-targets --target wasm32-wasip2 -- -D warnings` succeed.

- [x] **Task 4: Create `veldmap-rpc` and Prove WASI ConnectRPC Streaming**
  - **Action:**
    1. Implement the resource-agnostic helpers and error mappings in Section 2.3, including route-specific limits, health checks, event replay, and cancellation.
    2. Add only essential native tests for custom event replay invariants and recoverable/terminal error mapping. Use the integration scenario for bounded streaming and cancellation instead of retesting Tokio channels.
    3. Add small example Cache client/server components and a host-driven integration test. Return a recoverable item error followed by a valid item, then cancel an active stream and observe producer termination.
  - **Verification:** `cargo test -p veldmap-rpc`, native/WASI clippy with `-D warnings`, both example components build for `wasm32-wasip2`, and the two-component Wasmtime integration test passes its stream and cancellation assertions.

- [ ] **Task 5: Prove the `wasi-gfx` Presentation Shell**
  - **Action:**
    1. Pin compatible graphics/runtime forks and create the minimal `services/data-browser` shell without migrating the legacy UI yet.
    2. Render text and basic iced widgets through wgpu, present through `wasi-gfx`, and composite an animated external image.
    3. Handle pointer, wheel, keyboard text input, resize, scale factor, and clean shutdown. Record exact runtime prerequisites and the verified launch command.
  - **Verification:** `cargo build -p veldmap-data-browser --target wasm32-wasip2` succeeds with zero warnings, and the component passes a real window smoke scenario covering rendering, animation, text entry, pointer input, resize/HiDPI, and shutdown. Compilation alone does not complete this task.

- [ ] **Task 6: Create `veldmap-core`**
  - **Action:**
    1. Migrate and modernize `pyramid`, `tile`, `geodesy`, `wheel`, `tiles`, `range`, `config`, and `gpu_fallback` from the legacy modules.
    2. Implement bounded VRAM-aware tile state, 64 KiB range-block pooling with readahead/prefetch, local and HTTP Range readers, `${VAR}` config expansion, and pure-Rust 2D/3D fallbacks.
    3. Keep remote signing behind a standard Rust trait so `range` does not depend on a concrete socket transport.
    4. Add shared RGBA frame-layout and pixel-format validation for the renderer services, keeping resource policy out of `veldmap-rpc`.
    5. Add golden tests for geodesy/projection and focused tests for pyramid math, tile budgets, range reads, prefetch cancellation, and software rendering.
  - **Verification:** `cargo test -p veldmap-core`, `cargo clippy -p veldmap-core --all-targets -- -D warnings`, and `cargo clippy -p veldmap-core --all-targets --target wasm32-wasip2 -- -D warnings` succeed.

- [ ] **Task 7: Migrate `services/data-provider`**
  - **Action:**
    1. Move `veldmodules/data-provider` to `services/data-provider` as a library plus thin WASI binary and delete `veldmodules/data-provider`.
    2. Implement `Provider` on `127.0.0.1:50051` using the result envelopes and error mapping from Section 2.
    3. Preserve OData search, `FRESH_DAYS` fallback widening, scene/part folding, S3 SigV4 signing, manifest parsing, and MGRS UTM math.
    4. Add fixture-based tests and a real WASI RPC smoke test against local CDSE/S3 fixtures; keep live CDSE verification separate from deterministic tests.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-provider --target wasm32-wasip2 --release`, the WASI RPC smoke scenario, and `! test -d veldmodules/data-provider` succeed.

- [ ] **Task 8: Migrate `services/tile-cache`**
  - **Action:**
    1. Move the cache to `services/tile-cache`, expose its domain library, serve `Cache` on `127.0.0.1:50054`, and delete `veldmodules/tile-cache`.
    2. Read/write QOI tiles under `/runtime/data/tiles/` with atomic temporary writes, rename, and LRU eviction under `cache_limit_mb`.
    3. Distinguish misses, corrupt individual entries, and fatal store errors. Test atomic recovery, eviction, and non-terminal item errors.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, a WASI place/query/eviction smoke scenario, release WASI build, and `! test -d veldmodules/tile-cache` succeed.

- [ ] **Task 9: Migrate `services/data-library`**
  - **Action:**
    1. Move the library to `services/data-library`, serve `Library` on `127.0.0.1:50052`, and delete `veldmodules/data-library`.
    2. Implement `.part` resumable HTTP downloads, `.origin` sidecars, pause/remove behavior, and scene counts under `/runtime/data/dem/source`.
    3. Use generated Provider clients for signing/root resolution and publish owned `DownloadItem` values through `EventTable`.
    4. Test interrupted ranges, resume validation, pause/cancel, replay gaps, and per-item failures against a local HTTP fixture and mock Provider.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, a WASI download/event-stream smoke scenario, release WASI build, and `! test -d veldmodules/data-library` succeed.

- [ ] **Task 10: Migrate `services/image-tiler`**
  - **Action:**
    1. Move the tiler to `services/image-tiler`, serve `Tiler` on `127.0.0.1:50053`, and delete `veldmodules/image-tiler`.
    2. Preserve all TIFF, JP2/OpenJP2, excerpt, codec, JPEG, PNG, NetCDF/HDF5, grid, table, radiometry, resample, fingerprint, and budget adapters with typed errors.
    3. Open sources through `veldmap_core::range`, verify request fingerprints, stream tiles/progress/item errors, cache QOI asynchronously, and stop when clients cancel.
    4. Test every adapter on small fixtures, including actual WASI OpenJP2 decoding and a cancelled production pass.
  - **Verification:** native adapter tests, native/WASI clippy with `-D warnings`, the WASI describe/produce/cache/cancel scenario, release WASI build, and `! test -d veldmodules/image-tiler` succeed.

- [ ] **Task 11: Migrate `services/image-view`**
  - **Action:**
    1. Move the viewer to `services/image-view`, serve `Viewer` on `127.0.0.1:50055`, and delete `veldmodules/image-view`.
    2. Use Tiler and Cache clients plus `veldmap_core::tiles` for the pyramid ladder.
    3. Render wgpu or software RGBA frames, validate dimensions/stride, broadcast latest frames per view, and publish `ViewStatus` through `EventTable`.
    4. Test fit/zoom/pan, resize, variable selection, frame sequencing, cancellation, and a real `WatchFrames` stream.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, a headless WASI frame/status smoke scenario, release WASI build, and `! test -d veldmodules/image-view` succeed.

- [ ] **Task 12: Migrate `services/globe`**
  - **Action:**
    1. Move the globe to `services/globe`, serve `Globe` on `127.0.0.1:50056`, and delete `veldmodules/globe`.
    2. Preserve WGS84 geometry, `SURFACE_LIFT_M`, warp subdivision, culling, georeference ranks, Transverse Mercator/Pulkovo math, outlines/hatching, and quaternion flights.
    3. Render wgpu or software frames and publish overlay status/latest frames with bounded queues and cancellation.
    4. Add golden math/culling tests and a headless WASI scenario covering overlays, camera control, surface probing, and frames.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, the headless WASI smoke scenario, release WASI build, and `! test -d veldmodules/globe` succeed.

- [ ] **Task 13: Complete `services/data-browser`**
  - **Action:**
    1. Extend the Task 5 shell with the legacy browser behavior and delete `veldmodules/` completely.
    2. Preserve panes/tabs, layout persistence, catalogue/search/download tables, row states, batch actions, NetCDF selectors, and camera interaction.
    3. Connect to all six backend/renderer services with generated ConnectRPC clients and iced subscriptions; composite latest viewer/globe frames through wgpu.
    4. Coalesce camera input with bounded pending state and at most one in-flight `ControlCamera` RPC per view or globe. Accumulate pan/orbit deltas, preserve zoom anchoring and discrete-action ordering, collapse resize to the latest dimensions, and do not emit one RPC per raw pointer event.
    5. Exercise a complete user flow against local fixtures, including recoverable item errors, reconnects, live view resizing, and clean stream cancellation.
  - **Verification:** native tests where applicable, native/WASI clippy with `-D warnings`, release WASI build, the real end-to-end window scenario, and `! test -d veldmodules` succeed.

- [ ] **Task 14: Orchestrate and Document the Debug System**
  - **Action:**
    1. Create `process-compose.yaml` for all seven WASI components with least-privilege filesystem/network settings, readiness probes, and dependency ordering.
    2. Use standard Connect health checks for backend readiness and keep service logs under `runtime/logs/`.
    3. Update the 35-line README and create concise `docs/overview.md`, `services.md`, `imagery-and-globe.md`, and `operations.md` (each at most 80 lines).
    4. Document exact pinned prerequisites, debug launch, deterministic fixture run, and optional live CDSE run without exposing secrets.
    5. Measure and record representative debug `WatchFrames` throughput and memory at the configured window size before choosing any resolution or rate limits.
  - **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace`, native/WASI workspace clippy with `-D warnings`, `cargo build --workspace --target wasm32-wasip2 --release`, and a complete `process-compose` user-flow smoke scenario all pass.

- [ ] **Task 15: Build and Measure the Composed Release**
  - **Action:**
    1. Create `apps/veldmap` from the existing domain libraries and produce one optimized release binary while keeping the Task 14 debug configuration available.
    2. Replace process boundaries first with direct domain calls or a standard ConnectRPC `ClientTransport` implementation, preserving validation, errors, cancellation, and event ordering.
    3. Benchmark startup, catalogue operations, tile production, 4K frame delivery, memory, and CPU against the debug baseline. Add GPU handle sharing or binary/socket rewriting only after a separate measured design review.
    4. Run semantic parity tests and the complete user flow on the composed binary. Document measurements and any optimizations not adopted.
  - **Verification:** the composed release build, native/workspace tests, semantic parity checks, and the complete release user-flow smoke scenario pass; benchmark results are recorded without claiming unmeasured zero-copy behavior.
