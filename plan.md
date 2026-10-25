# plan.md — VeldMap WASI 0.3, ConnectRPC, and Buffa Rewrite Plan

## 0. Agent Execution Protocol

Each chat session executes one task from Section 5 and treats this file as the migration contract.

1. Locate the first unchecked task, or the task explicitly requested by the user.
2. Implement only that task. Do not jump ahead or silently shrink its verification.
3. Follow these rules in every task:
   - Write English only in source, comments, UI text, logs, documentation, and commit messages. Do not introduce Cyrillic characters (`[\u0400-\u04FF]`).
   - Use self-documenting names, small functions, strong domain types, and `thiserror` errors. Keep comments exceptional: unsafe invariants, external protocol/OS quirks, or one-line mathematical citations. Keep comments below 5% of lines in each Rust file.
   - Do not restore `veldcore/platform/host`, `veldsdk`, `buildgen/`, `schema.yaml`, legacy host configs, `generated/` directories, or the `ui-service` process/ABI.
   - Use Rust 2024, workspace dependency inheritance, workspace lints, and modern module paths (`src/foo.rs` plus `src/foo/bar.rs`, not `src/foo/mod.rs`). There is no Rust 2026 edition; do not use the compiler's `future` edition.
   - Compile all seven debug services and the composed release to `wasm32-wasip3`. Use the runtime and WASI configuration from Section 1.3. Native binaries are permitted for tests and development tools, never for delivered services or applications.
   - Add only the minimum essential tests for externally observable behavior and non-obvious invariants. Do not test getters, trivial arithmetic or wiring, or dependency behavior, and do not duplicate integration coverage with unit tests. Geodesy, projection, codecs, cache eviction, resumable downloads, snapshot invalidation/coalescing, and stream cancellation require correctness tests; compilation alone is not migration verification.
   - Keep README at 35 lines or fewer and documents under `docs/` at 80 lines or fewer. Reference protobuf contracts instead of duplicating their field definitions.
4. A replacement may coexist with its predecessor only while being verified. After the replacement passes its complete behavioral and real runtime scenario, remove superseded source, dependencies, feature flags, compatibility shims, configuration, documentation, and unused lockfile entries in the same task. Update all call sites and rerun verification. Do not retain an unused compatibility layer or second implementation "just in case".
5. Run the complete verification for the current task, including its runtime scenario.
6. Only after verification passes, change that task's checkbox from `- [ ]` to `- [x]`. Do not add session notes or alter other task descriptions.
7. If an external runtime assumption fails, record the evidence and revise the roadmap before building later tasks on that assumption. Pin a verified replacement rather than silently falling back to an older architecture.

---

## 1. Target Architecture

### 1.1 Two Configurations, One Domain Implementation

The project has two configurations with the same service behavior:

- **Debug configuration:** seven independent WASI 0.3 components orchestrated by `process-compose.yaml`. They communicate through native ConnectRPC over loopback TCP. Socket copies and raw RGBA frame transfer are acceptable because they make isolation, cancellation, logs, and failures easy to inspect.
- **Composed release configuration:** one optimized WASI 0.3 component built from the same domain libraries after the debug system works. Direct domain calls or a standard ConnectRPC `ClientTransport` replacement remove process overhead where measurements justify it. GPU resource sharing, WASM rewriting, and socket replacement are optional optimizations, not assumptions required by the initial design.

Every service crate exposes its domain logic through a library and keeps its WASI executable entry point thin. ConnectRPC handlers are adapters, not owners of business logic. No macro DSL or custom socket ABI is required. The delivered application never becomes a native binary.

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
│   └── veldmap/
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

- Use Rust 1.100 with `rust-version = "1.100"`, Rust 2024, a pinned `rust-toolchain.toml`, and a committed `Cargo.lock`. Until stable 1.100 is available, pin `1.100.0-beta.7`; move to stable 1.100.x as soon as the same workspace verification passes.
- Target WASI 0.3 through `wasm32-wasip3` now. Do not maintain a second WASI target or a long-term dual-target compatibility policy.
- Pin direct dependencies exactly and lock the complete transitive graph. The verified starting set is Tokio 1.54.1, ConnectRPC 0.9.1 with a compatible exactly pinned `connectrpc-build`, and Buffa/`buffa-types`/`buffa-codegen` 0.9.2. Upgrade a dependency only with its compatible consumers, never by independently selecting the newest package.
- Configure WASI Tokio support explicitly in `.cargo/config.toml`:

```toml
[target.wasm32-wasip3]
rustflags = ["--cfg", "tokio_unstable"]
```

- Headless services use a current-thread Tokio runtime. The presentation component integrates WASI 0.3 native async with the component-model executor; do not assume Tokio's I/O driver can drive native WASI 0.3 sockets or graphics. Do not enable `rt-multi-thread` without verified cooperative-thread support.
- Disable ConnectRPC's native Zstd feature. Enable only the JSON, gzip, client, and server features required by each crate. Keep generated JSON support consistent with runtime features across the workspace.
- Use Wasmtime 50 for headless components. Grant loopback networking and only required filesystem directories; never grant every service write access to all of `runtime/`.
- GUI and GPU components use the exact host/guest/interface set in Section 1.4. Record `protoc`, Wasmtime, `process-compose`, and runtime build/install prerequisites in `docs/operations.md`.
- Native test harnesses may exercise domain libraries and spawn WASI components. This does not permit native delivery artifacts.

### 1.4 Presentation and Rendering

`data-browser` owns the window/surface, input, runtime-supported clipboard and accessibility integration, fonts, ConnectRPC clients, and final presentation. Use iced development revision `cd04b74ca7073766f654535d6e72c452ef749d07` (`0.15.0-dev`, MSRV 1.93) and guest wgpu 30, pinned exactly with their compatible `iced_core`, `iced_runtime`, `iced_widget`, and renderer crates.

There is no ready-made iced/WASI/wgpu shell. Implement and own the required wgpu dispatch/backend, compositor/surface integration, event translation, and native-async executor adapter. Keep this platform layer separate from application state and domain logic.

Use `wasi-gfx-runtime` revision `9c79e69dbb00fd04f04eb7feee3804c8eec73392` as the initial host pin; it uses host wgpu 29. Guest wgpu 30 and host wgpu 29 are valid only when both speak the identical versioned WIT contract. The initial interface pins are `wasi:webgpu@0.3.0-rc.2` and `wasi-gfx:surface@0.2.0`. These are evolving pre-release contracts: review and test host, guest, and WIT together on every upgrade rather than treating either side as independently stable.

The current surface contract does not provide the complete required pointer-button, wheel, scale-factor, clipboard, focus/IME, and application-close lifecycle. Extend the pinned interface/host integration where necessary and prove real host events. Synthetic events alone do not verify the host-to-guest path. Task 5 must also complete a real ConnectRPC call from the same WASI 0.3 component that owns the graphics surface before the full UI depends on this stack.

`image-view` and `globe` own their 2D/3D rendering state. In debug they send latest RGBA frames over ConnectRPC and `data-browser` uploads and composites those frames. In the composed component they may share internal GPU resources without changing domain behavior.

The previous `master:veldmodules/ui-service` code is only a reference for surface lifecycle, glyph atlases, scissoring, and external-texture compositing. Do not restore its process, proto UI DSL, host ABI, resource-grant API, or unsafe thread-local implementation.

Both renderer services retain pure-Rust software fallbacks for headless verification. A fallback is not a substitute for proving the actual wgpu `data-browser` window, RPC, and input path.

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

Buffa views borrow request storage and must not be stored in domain state or other long-lived structures. Convert them to owned messages at the service boundary. `Bytes` provides cheap ownership, cloning, and slicing; it does not guarantee zero network copies or page alignment.

### 2.2 Error Semantics

There are two deliberately different error channels:

1. **Recoverable item failure:** remaining items are still meaningful. Return `common.ItemError` inside the response's `oneof`. A batch mutation returns `common.ItemResult` containing either `ItemSuccess` or `ItemError`. A finite data stream uses its service-specific result envelope. `ProduceEvent` has a dedicated error variant for a failed tile. These failures do not terminate the stream.
2. **RPC or stream failure:** return `ConnectError` and terminate the RPC. Use this for malformed requests, authentication failure, unavailable upstream services, source-wide failures, cancellation, deadlines, and failures after which later items cannot be trusted.

Finite streams end normally after their last item. Infinite subscriptions end on client cancellation, service shutdown, or a fatal RPC error. No contract claims that a stream can never terminate.

`ItemError.id` correlates the failure with the requested product, raster, object, view, or tile key. `message` is concise, English, safe to display, and contains no credentials or presigned URLs. `retryable` describes the specific item failure; clients still apply bounded retries with backoff.

Domain state such as an unviewable product or failed background download remains data in its status message. It is not converted into an RPC failure merely because the operation failed asynchronously.

### 2.3 `veldmap-rpc`

Keep this crate small, explicit, and independent of authoritative domain state:

- ConnectRPC server/client construction for loopback, route-specific message limits, and standard health checks. Static readiness requires `Health.Check`; `Health.Watch` may return `UNIMPLEMENTED` until a consumer requires it.
- Deadlines for finite calls and deliberately bounded operations. Intentional infinite subscriptions rely on explicit cancellation and shutdown rather than an arbitrary finite-request timeout.
- Bounded finite-stream helpers over `tokio::sync::mpsc`. Producers check cancellation between tiles, files, and frames. Ordinary work queues remain bounded and never silently drop tiles or mutations.
- Conversion between domain `thiserror` errors, recoverable `ItemError` values, and terminal `ConnectError` codes. Domain stores return domain errors; RPC adapters perform the conversion.
- Transport and stream helpers independent of images and other resource types. Resource validation belongs in shared domain libraries; extract only generic mechanisms with actual consumers.
- No domain values, mutation history, replay cursors, or authoritative subscription state in the RPC crate.

Configure 128 MiB only for routes that carry frames or large tile payloads. Keep smaller limits for catalogue, mutation, and control routes. Enable `TCP_NODELAY` on loopback sockets where the selected transport exposes it.

### 2.4 Snapshots and Invalidation

- `Library.WatchDownloads`, `Viewer.WatchViews`, and `Globe.WatchOverlays` stream complete snapshots of their current collections. Each stream sends an initial snapshot and later snapshots after domain invalidations; each snapshot replaces the preceding collection. There is no mutation-history view, replay cursor, or guarantee that intermediate states are delivered.
- Each service owns its current domain state and query behavior. Subscribe to invalidations before the initial snapshot. After a successful mutation, publish a latest-value invalidation; a change during snapshot construction or sending must trigger another refresh. Coalesce bursts so skipped intermediate invalidations are harmless.
- Notifications are transient wakeup hints, not authoritative data or durable events. Reconnects and missed subscriptions recover with a fresh snapshot. Outbound snapshots remain bounded; slow consumers may skip intermediate snapshots but not block unbounded production.
- Prefer stream-owned producers whose futures are dropped on cancellation. A spawned producer must also wait on outbound-channel closure so an idle subscription cannot leak a task.
- Keep high-frequency telemetry in ephemeral domain state and rate-limit its invalidations. Do not persist every download chunk or renderer progress tick. Lifecycle transitions and resume metadata retain the durability provided by the service's existing persistent representation.
- Implement the loop in the first real service and extract a shared helper only when later consumers prove duplication. Do not introduce a generic store, `Observed<T>`, SQLite, or an event-outbox framework preemptively.

### 2.5 Frames, Tiles, and Backpressure

- Live viewport frames use `PIXEL_FORMAT_RGBA8_SRGB`, `row_stride = width * 4`, and checked `pixels.len() == row_stride * height`. Implement shared validation in `veldmap-core` for both renderer services; do not add an aligned-`BytesMut` API without a real allocation mechanism and benchmark.
- Decoded pyramid tiles use `PIXEL_FORMAT_RGBA8_UNORM`; disk cache payloads use `PIXEL_FORMAT_QOI`.
- Cache tile keys include fingerprint, level, x, and y. `ProduceTilesRequest.fingerprint` must match the current `DescribeRasters` fingerprint before work starts.
- Dropping a ConnectRPC stream cancels work. A new camera/view request starts a new RPC; there is no epoch-kill side channel.
- `image-view` and `globe` own per-target `watch` channels with optional initial frames rather than fabricated empty images, and overwrite stale frames. Prove these semantics through renderer stream scenarios rather than isolated channel tests.
- `watch` limits queued frame count, not bandwidth. Measure debug frame throughput and memory instead of promising 60–144 fps.
- The composed release may replace frame bytes with internal GPU handles. Such handles are not added to protobuf and are never treated as portable WASI resources outside the composition boundary.

---

## 3. Protobuf Contract Governance

The files in `proto/` are the sole source of truth for schemas. Do not copy message or service definitions into this plan, documentation, or handwritten Rust. `crates/veldmap-proto` generates the only Rust contract API.

- Preserve proto3, versioned packages, declared idempotency, typed non-terminal item results, and the service boundaries represented by the eight schema files.
- Task 4a is a one-time coordinated pre-release revision of the v1 snapshot contracts before any service or client implementation consumes them. Remove `ReadOptions`, `EventMeta`, `Op`, replay options, and consumerless subscription filters; reserve every removed field number and name. This revision supersedes the earlier event/replay design and does not require preserving unused compatibility fixtures or creating parallel v2 packages.
- After Task 4a, evolve existing packages with wire-compatible additive changes. Never renumber or reuse tags; reserve removed field numbers and names. Treat changes to existing types, defaults, `oneof` membership, or JSON behavior as compatibility-sensitive. A later deliberate breaking change requires a new versioned package or explicitly versioned service and coordinated client migration.
- Preserve the error, snapshot-invalidation, frame, tile, and cancellation semantics in Section 2. A schema edit does not authorize changing those semantics silently.
- For every contract change, verify generated binary and JSON round trips plus old/new compatibility fixtures for all allowed additive reads. Keep descriptors and generated JSON support consistent with runtime features. Generated source remains in `OUT_DIR`, not the repository.

---

## 4. `data-browser` and Composed Release

The browser uses iced widgets for panes, tabs, tables, forms, selectors, progress, and viewport interaction. It subscribes to `Library.WatchDownloads`, `Viewer.WatchViews`, `Viewer.WatchFrames`, `Globe.WatchOverlays`, and `Globe.WatchFrames`. Each collection snapshot replaces the preceding collection while stable item IDs preserve selection, expansion, and scroll state. Latest frame messages update GPU resources; input updates camera RPCs and local pane state.

Task 5 proves the WASI 0.3 runtime, real RPC client, surface, text, host input, resize/HiDPI, and animated external image before full UI migration. Task 13 preserves the recursive pane layout and state persistence without restoring the UI proto DSL.

Task 15 builds one WASI 0.3 component from service libraries. Start with direct domain calls or standard ConnectRPC transport substitution while preserving request validation and error semantics. Benchmark RPC-heavy work, tile production, frame delivery, memory use, and startup. Add shared textures, binary rewriting, or socket replacement only after design review and measured benefit. The debug ConnectRPC configuration remains available for diagnostics.

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
    1. Create `rust-toolchain.toml`, `.cargo/config.toml`, and the root Rust 2024 workspace with resolver 3, MSRV 1.100, shared dependencies, and strict lints denying `unsafe_op_in_unsafe_fn`, `dbg_macro`, and `todo`.
    2. Create the seven application schemas governed by Section 3 with versioned packages and typed non-terminal item errors.
    3. Create `crates/veldmap-proto` with one `connectrpc-build` generation pass, generated views and service stubs, `BytesRepr::Bytes`, an `OUT_DIR` include file, and a descriptor set.
    4. Use a temporary `src/main.rs` to check success/error variants, event options, byte payload types, and Buffa encode/decode round trips. Run it with `cargo run -p veldmap-proto`, then delete the entry point; do not retain test scaffolding or generated source directories.
  - **Verification:** `cargo fmt --all -- --check`, the temporary contract-check scenario with its entry point removed afterward, native clippy, and `cargo clippy -p veldmap-proto --all-targets --target wasm32-wasip3 -- -D warnings` succeed.

- [x] **Task 4: Create `veldmap-rpc` and Prove WASI ConnectRPC Streaming**
  - **Action:**
    1. Implement the resource-agnostic helpers and error mappings in Section 2.3, including route-specific limits, health checks, event replay, deadlines, and cancellation.
    2. Add only essential native tests for custom event replay invariants and recoverable/terminal error mapping. Use the integration scenario for bounded streaming and cancellation instead of retesting Tokio channels.
    3. Add small example Cache client/server components and a host-driven integration test. Return a recoverable item error followed by a valid item, then cancel an active stream and observe producer termination.
  - **Verification:** `cargo test -p veldmap-rpc`, native/WASI clippy with `-D warnings`, both example components build for `wasm32-wasip3`, and the two-component Wasmtime 50 integration test passes its stream and cancellation assertions.

- [ ] **Task 4a: Replace Event Replay with Snapshot Watches**
  - **Action:**
    1. Remove `EventTable`, its replay tests, and newly unused crate/workspace/lockfile dependencies, including `ulid` and `buffa-types` from `veldmap-rpc`. Keep the transport, bounded-stream, health, deadline, cancellation, and error-mapping behavior that still has consumers.
    2. Replace `ListDownloads`, `ListViews`, and `ListOverlays` with the complete-snapshot watch contracts in Sections 2.4 and 3. Remove `ReadOptions`, `EventMeta`, `Op`, replay options, and subscription filters without a current consumer; reserve all removed protobuf field numbers and names.
    3. Add focused generated-contract binary/JSON round-trip checks for the revised snapshots. Preserve the existing recoverable/terminal error tests and Cache stream/cancellation scenario; do not invent a service implementation merely to test future watch behavior.
    4. Do not add SQLite, a generic store, `Observed<T>`, a snapshot-watch helper, or an event-outbox framework in this task. The first real service will implement the Section 2.4 loop before any extraction is considered.
  - **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace`, native clippy with `-D warnings`, and the existing WASI Cache integration pass on the pre-Task-5 baseline; the revised generated contracts pass their binary/JSON checks; and no nonhistorical source references to `EventTable` or replay contracts remain.

- [ ] **Task 5: Adopt the New Baseline and Prove the `wasi-gfx` Presentation Shell**
  - **Action:**
    1. Upgrade the existing workspace, toolchain, target configuration, and dependencies to Sections 1.3–1.4. Reprove the Task 4 Cache stream/cancellation scenario on WASI 0.3, explicitly including its Tokio timer path, then remove the old target configuration and superseded dependency entries under Section 0.
    2. Pin the exact graphics host/guest/WIT set and create minimal `services/data-browser` without migrating the legacy UI. Export `shell` and `frame` through `src/lib.rs`; keep `src/main.rs` as a thin WASI entry point.
    3. Implement the required native-async executor and wgpu/WASI integration. Render text and basic iced widgets through wgpu 30, present through `wasi-gfx`, and composite an animated external RGBA image without per-frame atlas churn.
    4. Handle real host pointer buttons/motion, wheel, keyboard text input, resize, scale factor, and clean shutdown. Add deterministic diagnostics triggered by observed host events; do not claim self-injected events as host integration verification.
    5. Complete a real ConnectRPC call from the graphics component to an existing WASI service while the surface remains operational. Record exact prerequisites, revisions, and verified launch commands in `docs/operations.md` within its 80-line limit.
  - **Verification:** existing workspace tests and the WASI 0.3 Cache integration pass; `cargo build -p veldmap-data-browser --target wasm32-wasip3` succeeds with zero warnings; and a real window smoke scenario passes rendering, animation, text entry, pointer/wheel, resize/HiDPI, concurrent RPC, and clean shutdown. Compilation or software rendering alone does not complete this task.

- [ ] **Task 6: Create `veldmap-core`**
  - **Action:**
    1. Migrate and modernize `pyramid`, `tile`, `geodesy`, `wheel`, `tiles`, `range`, `config`, and `gpu_fallback` from the legacy modules.
    2. Implement bounded VRAM-aware tile state, 64 KiB range-block pooling with readahead/prefetch, local and HTTP Range readers, `${VAR}` config expansion, and pure-Rust 2D/3D fallbacks.
    3. Keep remote signing behind a standard Rust trait so `range` does not depend on a concrete socket transport.
    4. Add shared RGBA frame-layout and pixel-format validation for renderer services, keeping resource policy out of `veldmap-rpc`.
    5. Add golden tests for geodesy/projection and focused tests for pyramid math, tile budgets, range reads, prefetch cancellation, and software rendering.
  - **Verification:** `cargo test -p veldmap-core`, native clippy with `-D warnings`, and `cargo clippy -p veldmap-core --all-targets --target wasm32-wasip3 -- -D warnings` succeed.

- [ ] **Task 7: Migrate `services/data-provider`**
  - **Action:**
    1. Move `veldmodules/data-provider` to `services/data-provider` as a library plus thin WASI binary and delete `veldmodules/data-provider` after verification.
    2. Implement `Provider` on `127.0.0.1:50051` using the result envelopes and error mapping from Section 2.
    3. Preserve OData search, `FRESH_DAYS` fallback widening, scene/part folding, S3 SigV4 signing, manifest parsing, and MGRS UTM math.
    4. Add fixture-based tests and a real WASI RPC smoke test against local CDSE/S3 fixtures; keep live CDSE verification separate from deterministic tests.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-provider --target wasm32-wasip3 --release`, the WASI RPC smoke scenario, and `! test -d veldmodules/data-provider` succeed.

- [ ] **Task 8: Migrate `services/tile-cache`**
  - **Action:**
    1. Move the cache to `services/tile-cache`, expose its domain library, serve `Cache` on `127.0.0.1:50054`, and delete `veldmodules/tile-cache` after verification.
    2. Read/write QOI tiles under `/runtime/data/tiles/` with atomic temporary writes, rename, and LRU eviction under `cache_limit_mb`.
    3. Distinguish misses, corrupt individual entries, and fatal store errors. Test atomic recovery, eviction, and non-terminal item errors.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-tile-cache --target wasm32-wasip3 --release`, a WASI place/query/eviction smoke scenario, and `! test -d veldmodules/tile-cache` succeed.

- [ ] **Task 9: Migrate `services/data-library`**
  - **Action:**
    1. Move the library to `services/data-library`, serve `Library` on `127.0.0.1:50052`, and delete `veldmodules/data-library` after verification.
    2. Implement `.part` resumable HTTP downloads, `.origin` sidecars, pause/remove behavior, and scene counts under `/runtime/data/dem/source`. Keep these files as the only persistent authorities; do not add SQLite or a duplicate metadata store without a new requirement and plan revision.
    3. Use generated Provider clients for signing/root resolution. Implement `WatchDownloads` directly over authoritative domain state with complete owned snapshots, post-mutation invalidation, coalesced ephemeral progress, and cancellation according to Section 2.4.
    4. Test interrupted ranges, resume validation, pause/cancel, initial/reconnect snapshots, invalidation during snapshot delivery, progress coalescing, and per-item failures against a local HTTP fixture and mock Provider.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-library --target wasm32-wasip3 --release`, a WASI download/snapshot-stream smoke scenario, and `! test -d veldmodules/data-library` succeed.

- [ ] **Task 10: Migrate `services/image-tiler`**
  - **Action:**
    1. Move the tiler to `services/image-tiler`, serve `Tiler` on `127.0.0.1:50053`, and delete `veldmodules/image-tiler` after verification.
    2. Preserve all TIFF, JP2/OpenJP2, excerpt, codec, JPEG, PNG, NetCDF/HDF5, grid, table, radiometry, resample, fingerprint, and budget adapters with typed errors.
    3. Open sources through `veldmap_core::range`, verify request fingerprints, stream tiles/progress/item errors, cache QOI asynchronously, and stop when clients cancel.
    4. Test every adapter on small fixtures, including actual WASI OpenJP2 decoding and a cancelled production pass.
  - **Verification:** native adapter tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-image-tiler --target wasm32-wasip3 --release`, the WASI describe/produce/cache/cancel scenario, and `! test -d veldmodules/image-tiler` succeed.

- [ ] **Task 11: Migrate `services/image-view`**
  - **Action:**
    1. Move the viewer to `services/image-view`, serve `Viewer` on `127.0.0.1:50055`, and delete `veldmodules/image-view` after verification.
    2. Use Tiler and Cache clients plus `veldmap_core::tiles` for the pyramid ladder.
    3. Render wgpu or software RGBA frames, validate dimensions/stride, and broadcast latest frames per view. Implement `WatchViews` with complete owned `ViewStatus` snapshots, post-mutation invalidation, progress coalescing, and cancellation according to Section 2.4.
    4. Test fit/zoom/pan, resize, variable selection, frame sequencing, snapshot refresh/reconnect, cancellation, and real `WatchViews` and `WatchFrames` streams.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-image-view --target wasm32-wasip3 --release`, a headless WASI frame/status smoke scenario, and `! test -d veldmodules/image-view` succeed.

- [ ] **Task 12: Migrate `services/globe`**
  - **Action:**
    1. Move the globe to `services/globe`, serve `Globe` on `127.0.0.1:50056`, and delete `veldmodules/globe` after verification.
    2. Preserve WGS84 geometry, `SURFACE_LIFT_M`, warp subdivision, culling, georeference ranks, Transverse Mercator/Pulkovo math, outlines/hatching, and quaternion flights.
    3. Render wgpu or software frames and broadcast latest frames with bounded queues and cancellation. Implement `WatchOverlays` with complete owned overlay-status snapshots, post-mutation invalidation, and progress coalescing according to Section 2.4.
    4. Add golden math/culling tests and a headless WASI scenario covering overlay snapshots/reconnects, camera control, surface probing, frames, and cancellation.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-globe --target wasm32-wasip3 --release`, the headless WASI smoke scenario, and `! test -d veldmodules/globe` succeed.

- [ ] **Task 13: Complete `services/data-browser`**
  - **Action:**
    1. Extend the Task 5 shell with the legacy browser behavior and delete `veldmodules/` completely after verification.
    2. Preserve panes/tabs, layout persistence, catalogue/search/download tables, row states, batch actions, NetCDF selectors, and camera interaction. Replace collections on each snapshot while using stable item IDs to preserve selection, expansion, and scroll state.
    3. Connect to all six backend/renderer services with generated ConnectRPC clients and iced subscriptions; composite latest viewer/globe frames through wgpu. Reconnect collection watches with fresh snapshots rather than replay cursors.
    4. Coalesce camera input with bounded pending state and at most one in-flight `ControlCamera` RPC per view or globe. Accumulate pan/orbit deltas, preserve zoom anchoring and discrete-action ordering, collapse resize to latest dimensions, and do not emit one RPC per raw pointer event.
    5. Exercise a complete user flow against local fixtures, including recoverable item errors, snapshot replacement/reconnects, live view resizing, runtime-supported clipboard/focus/IME behavior, and clean stream cancellation.
  - **Verification:** native tests where applicable, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-browser --target wasm32-wasip3 --release`, the real end-to-end window scenario, and `! test -d veldmodules` succeed.

- [ ] **Task 14: Orchestrate and Document the Debug System**
  - **Action:**
    1. Create `process-compose.yaml` for all seven WASI 0.3 components with least-privilege filesystem/network settings, readiness probes, and dependency ordering.
    2. Use standard Connect health checks for backend readiness and keep service logs under `runtime/logs/`.
    3. Update the 35-line README and create concise `docs/overview.md`, `services.md`, `imagery-and-globe.md`, and `operations.md` (each at most 80 lines).
    4. Document exact pinned prerequisites, debug launch, deterministic fixture run, and optional live CDSE run without exposing secrets.
    5. Measure and record representative debug `WatchFrames` throughput and memory at the configured window size, plus snapshot-refresh cost under representative progress updates, before choosing resolution or rate limits.
  - **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace`, native/WASI workspace clippy with `-D warnings`, `cargo build --workspace --target wasm32-wasip3 --release`, and a complete `process-compose` user-flow smoke scenario all pass.

- [ ] **Task 15: Build and Measure the Composed Release**
  - **Action:**
    1. Create `apps/veldmap` from existing domain libraries and produce one optimized WASI 0.3 component while keeping the Task 14 debug configuration available.
    2. Replace process boundaries first with direct domain calls or a standard ConnectRPC `ClientTransport` implementation, preserving validation, errors, cancellation, snapshot ordering, and invalidation coalescing.
    3. Benchmark startup, catalogue operations, tile production, 4K frame delivery, snapshot refresh, memory, and CPU against the debug baseline. Add GPU handle sharing or binary/socket rewriting only after a separate measured design review.
    4. Run semantic parity tests and the complete user flow on the composed component under its WASI runtime. Document measurements and optimizations not adopted; remove abandoned experimental implementations under Section 0.
  - **Verification:** the composed `wasm32-wasip3` release build, native/workspace tests, semantic parity checks, and complete release user-flow smoke scenario pass; benchmark results are recorded without claiming unmeasured zero-copy behavior.
