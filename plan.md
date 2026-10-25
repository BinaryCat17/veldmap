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
   - Compile every delivered component present at the current roadmap stage to `wasm32-wasip3`. Task 14 builds all seven reference components; Task 15 additionally builds the composed component. Use the runtime and WASI configuration from Section 1.3. Native binaries are permitted for tests, host tooling, and development tools, never for delivered services or applications.
   - Add only the minimum essential tests for externally observable behavior and non-obvious invariants. Do not test getters, trivial arithmetic or wiring, or dependency behavior, and do not duplicate integration coverage with unit tests. Geodesy, projection, codecs, cache eviction, resumable downloads, snapshot invalidation/coalescing, and stream cancellation require correctness tests; compilation alone is not migration verification.
   - Keep README at 35 lines or fewer and documents under `docs/` at 80 lines or fewer. Reference protobuf contracts instead of duplicating their field definitions.
4. A replacement may coexist with its predecessor only while being verified. After the replacement passes its complete behavioral and real runtime scenario, remove superseded source, dependencies, feature flags, compatibility shims, configuration, documentation, and unused lockfile entries in the same task. Update all call sites and rerun verification. Do not retain an unused compatibility layer or second implementation "just in case".
5. Run the complete verification for the current task, including its runtime scenario.
6. Only after verification passes, change that task's checkbox from `- [ ]` to `- [x]`. Do not add session notes or alter other task descriptions.
7. If an external runtime assumption fails, record the evidence and revise the roadmap before building later tasks on that assumption. Pin a verified replacement rather than silently falling back to an older architecture.

---

## 1. Target Architecture

### 1.1 Two Profiles, One Domain Implementation

The project has two profiles with the same observable service behavior:

- **Reference/debug profile:** exactly seven independently launched WASI 0.3 components orchestrated by `process-compose.yaml`. Every inter-component request and stream, including `Viewer.WatchFrames`, `Globe.WatchFrames`, and their RGBA payloads, uses generated ConnectRPC clients/servers over WASI loopback TCP sockets. Shared libraries provide code reuse, not a communication channel. In-process calls, shared memory, shared GPU resources, and custom host calls never replace a reference-profile edge.
- **Composed release profile:** one WASI 0.3 component statically linked from the same service libraries after the complete reference profile passes. Its baseline connects each generated ConnectRPC client to the corresponding `ConnectRpcService<Router>` through an in-memory `ClientTransport`, preserving HTTP envelopes, protobuf bodies, route limits, validation, errors, deadlines, cancellation, and stream behavior without opening internal TCP listeners. Clients never bypass this contract adapter to call domain stores directly. GPU sharing and further transport or binary optimizations are separate measured changes, not assumptions required by the baseline.

Every service crate exposes its domain logic through a library and keeps its WASI executable entry point thin. ConnectRPC handlers are adapters, not owners of business logic. No macro DSL, private host ABI, project-specific host import, or custom socket ABI is permitted. The delivered application never becomes a native binary. Composition receives the union of its required host capabilities; it does not preserve the reference profile's per-process sandbox isolation.

### 1.2 Repository Structure

```text
veldmap/
├── plan.md
├── README.md
├── Cargo.toml
├── Cargo.lock
├── wkg.lock
├── buf.yaml
├── buf.lock
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
│   ├── veldmap-core/
│   └── veldmap-host/
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
├── scripts/
│   └── check_component_imports.py
├── runtime/
│   ├── config/
│   │   └── component-imports.toml
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

- Use Rust 1.100 with `rust-version = "1.100"`, Rust 2024, an exactly pinned `rust-toolchain.toml`, and a committed `Cargo.lock`. Until stable 1.100 is available, pin `1.100.0-beta.7`; move to stable 1.100.x only after the same workspace verification passes. Record the exact core WASI API version emitted by the toolchain; target WASI 0.3 through `wasm32-wasip3` without a second WASI target or long-term dual-target policy.
- Pin direct dependencies exactly and use `--locked` on Cargo verification commands. The verified starting set is Tokio 1.54.1, ConnectRPC 0.9.1 with a compatible exactly pinned `connectrpc-build`, and Buffa/`buffa-codegen` 0.9.2. Upgrade a dependency only with its compatible consumers, never by independently selecting the newest package.
- Configure WASI Tokio support explicitly in `.cargo/config.toml`:

```toml
[target.wasm32-wasip3]
rustflags = ["--cfg", "tokio_unstable"]
```

- Headless services use a current-thread Tokio runtime. The presentation component integrates WASI 0.3 native async with the component-model executor; do not assume Tokio's I/O driver can drive native WASI 0.3 sockets or graphics. Do not enable `rt-multi-thread` without verified cooperative-thread support.
- Disable ConnectRPC's native Zstd feature. Enable only the JSON, gzip, client, and server features required by each crate. Keep generated JSON support consistent with runtime features across the workspace.
- Use Wasmtime 50.0.0 initially and pin exact patch revisions of Wasmtime, `wasm-tools`, WIT tooling, `protoc`, Buf, `process-compose`, and host/guest graphics dependencies. Commit `wkg.lock` when WIT package dependencies are introduced; do not commit fetched or generated WIT dependencies. Record all revisions and launch commands in `docs/operations.md`.
- Maintain a per-component allowlist of public, versioned host interfaces. Validate every delivered artifact with pinned `wasm-tools`, inspect it using `wasm-tools component wit`, and reject unknown, legacy, project-specific, native-memory, or resource-grant imports. Repeat inspection after binary rewriting. Allowed packages are the required core WASI interfaces, `wasi:http` where used, `wasi:webgpu`, and approved `wasi-gfx` packages.
- Build one native `veldmap-host` embedder with shared headless and graphics paths so their WASI behavior cannot diverge. Configure only required directory preopens and `WasiCtxBuilder::socket_addr_check`; broad `wasmtime run -S inherit-network` access does not satisfy per-component network policy. Native host tooling implements standard interfaces and does not permit a private guest ABI.
- Use WASI TCP for internal plaintext ConnectRPC. Use `wasi:http` outgoing requests with host-provided TLS for external CDSE/S3 HTTP and Range traffic. Task 5a must prove the pinned runtime's p3 outgoing HTTP, TLS, Range, streaming-body, and cancellation support before later tasks depend on them. The host must not follow redirects and must enforce egress origins through its HTTP hooks; the guest rejects every 3xx response to a signed request and never forwards credentials across origins. Specify timeouts, certificate roots, and deterministic fixtures.
- Native test harnesses may exercise domain libraries and spawn WASI components. This does not permit native delivery artifacts.

### 1.4 Presentation and Rendering

`data-browser` owns the window/surface, input, clipboard and IME where supported by the approved public interfaces, fonts, ConnectRPC clients, and final presentation. Use iced development revision `cd04b74ca7073766f654535d6e72c452ef749d07` (`0.15.0-dev`, MSRV 1.93) and guest wgpu 30, pinned exactly with their compatible `iced_core`, `iced_runtime`, `iced_widget`, and renderer crates. OS accessibility remains unresolved until a public interface and real host implementation exist; iced's internal semantic tree alone is not accessibility verification.

There is no ready-made iced/WASI/wgpu shell. Implement and own the required wgpu dispatch/backend, compositor/surface integration, event translation, and native-async executor adapter. Keep this platform layer separate from application state and domain logic. `shell` and `frame` are Rust library items, not custom WIT callbacks. Use the standard `wasi:cli/command` entry point and ordinary WASI stdio. `veldmap-host` links CLI, clocks, filesystem, sockets, required HTTP support, `wasi:webgpu`, and `wasi-gfx` into the same Wasmtime `Linker`, with least-privilege grants.

Use `wasi-gfx-runtime` revision `9c79e69dbb00fd04f04eb7feee3804c8eec73392` as the initial host pin; it uses host wgpu 29. Guest wgpu 30 and host wgpu 29 are valid only when both speak the identical versioned WIT contract. The initial interface pins are `wasi:webgpu@0.3.0-rc.2` and `wasi-gfx:surface@0.2.0`. `wasi:webgpu` is an official pre-release WASI proposal; surface and frame-buffer belong to the independently governed public `wasi-gfx` project, not core WASI. Review and test host, guest, and WIT together on every upgrade.

The current surface contract provides pointer motion/up/down, keyboard text, resize, and frame events, but not pointer-button identity, wheel, scale-factor, clipboard, focus/IME, or application-close lifecycle. If still missing, create a minimal public fork of `wasi-gfx/wasi-gfx` and, where necessary, `wasi-gfx/wasi-gfx-runtime`. Keep changes generic and free of VeldMap concepts, use a distinct pre-release package version and immutable revisions/content locks, and submit an upstream pull request early. An approved staging package is identified by package name, pre-release version, source repository/revision, and content digest in the allowlist/provenance locks; artifact inspection checks its identity while the locks establish provenance. Record the pull request and staging revisions in `docs/operations.md`; never describe the staging fork as an accepted standard. Task completion requires real host-event verification but does not wait for upstream merger. Adopt equivalent upstream releases when available and remove the superseded fork under Section 0. Clipboard, IME, and accessibility are separate capabilities, not fields to accumulate indiscriminately in `surface`. Synthetic events alone never verify host integration.

`image-view` and `globe` own their 2D/3D rendering state. In the reference profile they send latest RGBA frames over ConnectRPC and `data-browser` uploads and composites those frames. Only the composed profile may introduce an internal GPU frame adapter backed by a verified shared `wgpu::Device`/`Queue`. It must preserve target IDs, sequencing, dimensions, formats, latest-frame overwrite, bounded lifetime, and cancellation. Keep deterministic GPU readback that produces valid RGBA `TextureFrame` values for parity checks; never add GPU handles to protobuf or a host interface. Deterministic software output uses exact comparisons; wgpu readback uses a documented per-channel and raster-edge tolerance rather than claiming bit-exact cross-driver output.

The previous `master:veldmodules/ui-service` code is only a reference for surface lifecycle, glyph atlases, scissoring, and external-texture compositing. Do not restore its process, proto UI DSL, host ABI, resource-grant API, or unsafe thread-local implementation.

Both renderer services retain pure-Rust software fallbacks for headless verification. A fallback is not a substitute for proving the actual wgpu window, RPC, and input path.

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

- `Library.WatchDownloads`, `Viewer.WatchViews`, and `Globe.WatchOverlays` stream complete snapshots of their current collections. Each stream sends an initial snapshot and later snapshots after domain invalidations; each snapshot replaces the preceding collection. Items have deterministic ordering by stable ID unless a service defines another explicit stable order. There is no mutation-history view, replay cursor, or guarantee that intermediate states are delivered.
- Each service owns its current domain state and query behavior. Subscribe to invalidations before the initial snapshot. After a successful mutation, publish a latest-value invalidation; a change during snapshot construction or sending must trigger another refresh. Coalesce bursts so skipped intermediate invalidations are harmless.
- Use `tokio::sync::watch` for collection invalidation. Publishers call `send_modify` only after a successful mutation. Each subscriber calls `subscribe()` followed by `mark_changed()`, then waits with `changed().await`. Changes occurring during snapshot construction or sending must remain unseen and trigger another iteration. Do not replace this with bare `Notify` without proving equivalent race-free behavior.
- Notifications are transient wakeup hints, not authoritative data or durable events. Reconnects and missed subscriptions recover with a fresh snapshot. Outbound snapshots remain bounded; slow consumers may skip intermediate snapshots but not block unbounded production.
- Prefer stream-owned producers whose futures are dropped on cancellation. A spawned producer must also wait on outbound-channel closure so an idle subscription cannot leak a task.
- Keep high-frequency telemetry in ephemeral domain state and rate-limit its invalidations. Do not persist every download chunk or renderer progress tick. Lifecycle transitions and resume metadata retain the durability provided by the service's existing persistent representation.
- Implement the loop in the first real service and extract a shared helper only when later consumers prove duplication. Do not introduce a generic store, `Observed<T>`, SQLite, or an event-outbox framework preemptively.

### 2.5 Frames, Tiles, and Backpressure

- Every RGBA `TextureFrame` uses `PIXEL_FORMAT_RGBA8_SRGB`, `row_stride = width * 4`, and checked `pixels.len() == row_stride * height`. Implement shared validation in `veldmap-core` for both renderer services; do not add an aligned-`BytesMut` API without a real allocation mechanism and benchmark.
- Decoded pyramid tiles use `PIXEL_FORMAT_RGBA8_UNORM`; disk cache payloads use `PIXEL_FORMAT_QOI`.
- Cache tile keys include fingerprint, level, x, and y. `ProduceTilesRequest.fingerprint` must match the current `DescribeRasters` fingerprint before work starts.
- Dropping a ConnectRPC stream cancels work. A new camera/view request starts a new RPC; there is no epoch-kill side channel.
- `image-view` and `globe` own per-target `watch` channels with optional initial frames rather than fabricated empty images, and overwrite stale frames. Prove these semantics through renderer stream scenarios rather than isolated channel tests.
- `watch` limits queued frame count, not bandwidth. Measure reference frame throughput and memory instead of promising 60–144 fps.
- The reference profile always transfers RGBA bytes through ConnectRPC. Only the composed GPU frame adapter may replace bytes with internal textures under Section 1.4; its readback path must produce valid RGBA `TextureFrame` values, and GPU handles never enter protobuf or become portable host resources.

---

## 3. Protobuf Contract Governance

The files in `proto/` are the sole source of truth for schemas. Do not copy message or service definitions into this plan, documentation, or handwritten Rust. `crates/veldmap-proto` generates the only Rust contract API.

- Preserve proto3, versioned packages, declared idempotency, typed non-terminal item results, and the service boundaries represented by the eight schema files.
- Task 4a's snapshot revision and Task 5a's storage-pagination correction are the only coordinated pre-release v1 revisions before service/client implementations consume the affected contracts. Remove `ReadOptions`, `EventMeta`, `Op`, consumerless subscription filters, and public client/page-token pagination as specified in those tasks; reserve every removed field number and name. These revisions do not require preserving unused compatibility fixtures or creating parallel v2 packages.
- After Task 5a, evolve existing packages with wire-compatible additive changes. Never renumber or reuse tags; reserve removed field numbers and names. Treat changes to existing types, defaults, `oneof` membership, or JSON behavior as compatibility-sensitive. A later deliberate breaking change requires a new versioned package or explicitly versioned service and coordinated client migration.
- Preserve the error, snapshot-invalidation, frame, tile, and cancellation semantics in Section 2. A schema edit does not authorize changing those semantics silently.
- For every contract change, verify generated binary and JSON round trips plus old/new compatibility fixtures for all allowed additive reads. Add pinned protobuf lint and breaking-change checks against the finalized Task 5a baseline. Keep descriptors and generated JSON support consistent with runtime features. Generated source remains in `OUT_DIR`, not the repository.

---

## 4. `data-browser` and Composed Release

The browser uses iced widgets for panes, tabs, tables, forms, selectors, progress, and viewport interaction. It subscribes to `Library.WatchDownloads`, `Viewer.WatchViews`, `Viewer.WatchFrames`, `Globe.WatchOverlays`, and `Globe.WatchFrames`. Each collection snapshot replaces the preceding collection while stable item IDs preserve selection, expansion, and scroll state. Latest frame messages update GPU resources; input updates camera RPCs and local pane state.

Task 5a proves the WASI 0.3 baseline, Task 5b proves basic graphics with concurrent RPC before renderer migration, and Task 5c completes host input and lifecycle behavior before full UI migration. Task 13 preserves the recursive pane layout and state persistence without restoring the UI proto DSL.

Task 15 statically links service libraries into one WASI 0.3 component. Its baseline is in-memory HTTP ConnectRPC through `ClientTransport` and `ConnectRpcService<Router>`, not direct domain-store calls or WAC composition of command components. Benchmark RPC-heavy work, tile production, frame delivery, memory use, and startup. Add shared textures or further transport/binary rewriting only after design review, measured benefit, and unchanged semantic-parity results. The reference ConnectRPC configuration remains available as the behavioral authority and for diagnostics.

---

## 5. Sequential Work Plan

Execute tasks in their listed order. Tasks 5b and 5c intentionally appear at their dependency points rather than beside Task 5a.

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
    2. Create the seven application schemas governed by Section 3 with versioned packages and typed non-terminal item errors.
    3. Create `crates/veldmap-proto` with one `connectrpc-build` generation pass, generated views and service stubs, `BytesRepr::Bytes`, an `OUT_DIR` include file, and a descriptor set.
    4. Use a temporary `src/main.rs` to check success/error variants, event options, byte payload types, and Buffa encode/decode round trips. Run it with `cargo run -p veldmap-proto`, then delete the entry point; do not retain test scaffolding or generated source directories.
  - **Verification:** `cargo fmt --all -- --check`, the temporary contract-check scenario with its entry point removed afterward, native clippy, and `cargo clippy -p veldmap-proto --all-targets --target wasm32-wasip2 -- -D warnings` succeed on the pre-Task-5a baseline.

- [x] **Task 4: Create `veldmap-rpc` and Prove WASI ConnectRPC Streaming**
  - **Action:**
    1. Implement the resource-agnostic helpers and error mappings in Section 2.3, including route-specific limits, health checks, event replay, deadlines, and cancellation.
    2. Add only essential native tests for custom event replay invariants and recoverable/terminal error mapping. Use the integration scenario for bounded streaming and cancellation instead of retesting Tokio channels.
    3. Add small example Cache client/server components and a host-driven integration test. Return a recoverable item error followed by a valid item, then cancel an active stream and observe producer termination.
  - **Verification:** `cargo test -p veldmap-rpc`, native/WASI clippy with `-D warnings`, both example components build for `wasm32-wasip2`, and the two-component Wasmtime integration passes its stream and cancellation assertions on the pre-Task-5a baseline.

- [x] **Task 4a: Replace Event Replay with Snapshot Watches**
  - **Action:**
    1. Remove `EventTable`, its replay tests, and newly unused crate/workspace/lockfile dependencies, including `ulid` and `buffa-types` from `veldmap-rpc`. Keep the transport, bounded-stream, health, deadline, cancellation, and error-mapping behavior that still has consumers.
    2. Replace `ListDownloads`, `ListViews`, and `ListOverlays` with the complete-snapshot watch contracts in Sections 2.4 and 3. Remove `ReadOptions`, `EventMeta`, `Op`, replay options, and subscription filters without a current consumer; reserve all removed protobuf field numbers and names.
    3. Add focused generated-contract binary/JSON round-trip checks for the revised snapshots. Preserve the existing recoverable/terminal error tests and Cache stream/cancellation scenario; do not invent a service implementation merely to test future watch behavior.
    4. Do not add SQLite, a generic store, `Observed<T>`, a snapshot-watch helper, or an event-outbox framework in this task. The first real service will implement the Section 2.4 loop before any extraction is considered.
  - **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace`, native clippy with `-D warnings`, and the existing WASI Cache integration pass on the pre-Task-5a baseline; the revised generated contracts pass their binary/JSON checks; and no nonhistorical source references to `EventTable` or replay contracts remain.

- [ ] **Task 5a: Adopt the WASI 0.3 Baseline and Finalize Pre-Service Contracts**
  - **Action:**
    1. Upgrade the existing workspace, Rust 1.100 beta/stable toolchain under the Section 1.3 fallback, target configuration, exact dependency pins, and committed locks. Reprove the Task 4 Cache stream/cancellation scenario on WASI 0.3, explicitly including its Tokio timer path, then remove the old target configuration and superseded dependency entries under Section 0.
    2. Create the shared native `veldmap-host` embedder with its headless path, per-component allowlist in `runtime/config/component-imports.toml`, `scripts/check_component_imports.py`, and pinned Buf configuration. Update the README within its 35-line limit to describe the actual WASI 0.3 roadmap state.
    3. Before `veldmap-core` depends on external HTTP, prove a minimal WASI 0.3 outgoing `wasi:http` scenario through `veldmap-host`: host TLS, a Range response, streaming-body reads, cancellation by dropping the body, and no redirect following. Verify the guest rejects 3xx on signed requests and the host enforces egress origins. If the pinned p3 implementation cannot pass, record evidence and revise the roadmap under Section 0 instead of silently using guest TLS or an older target.
    4. Before Provider has an implementation or consumer, remove public request/page-token pagination. Add a bounded client limit to the request; the server follows S3 continuation internally and streams entries up to that limit or cancellation. Add a terminal metadata stream item carrying `truncated`, including for an empty listing. Reserve both removed token field numbers/names and verify generated binary/JSON round trips.
    5. Record exact prerequisites, revisions, any WIT locks, and verified launch commands in `docs/operations.md` within its 80-line limit. Tasks 6–10 depend on this task, not on graphics completion.
  - **Verification:** existing workspace tests and the WASI 0.3 Cache integration pass; the outgoing HTTP/TLS/Range/cancellation/redirect scenario passes; exact pins work with `--locked` on Cargo verification commands; all current artifacts pass interface inspection; empty, truncated, and complete listing contracts pass their checks; and no superseded WASI target or dependency configuration remains.

- [ ] **Task 6: Create `veldmap-core`**
  - **Action:**
    1. Migrate and modernize `pyramid`, `tile`, `geodesy`, `wheel`, `tiles`, `range`, `config`, and `gpu_fallback` from the legacy modules.
    2. Implement bounded VRAM-aware tile state, 64 KiB range-block pooling with readahead/prefetch, local readers, streaming HTTP Range readers over `wasi:http`, `${VAR}` config expansion, and pure-Rust 2D/3D fallbacks. Keep signing and HTTP behind standard Rust traits so domain logic does not depend on a concrete host transport. Re-sign expired credentials before new Range requests and permit one bounded retry after a signature-expiry response; do not require re-signing an already accepted response body mid-stream.
    3. Define collision-resistant source identity for cache correctness. Mutable local sources require content identity that detects same-length mid-file edits; sampled FNV over the head and tail is insufficient. Verified immutable remote version identities may avoid rehashing immutable content.
    4. Add shared RGBA frame-layout and pixel-format validation for renderer services, keeping resource policy out of `veldmap-rpc`.
    5. Add golden tests for geodesy/projection and focused tests for pyramid math, tile budgets, local/HTTP range reads, re-signing/expiry, redirect/credential policy, prefetch cancellation, software rendering, and same-length mid-file local mutation.
  - **Verification:** `cargo test -p veldmap-core`, native clippy with `-D warnings`, `cargo clippy -p veldmap-core --all-targets --target wasm32-wasip3 -- -D warnings`, and host-driven WASI HTTP Range/cancellation/re-signing fixture scenarios succeed.

- [ ] **Task 7: Migrate `services/data-provider`**
  - **Action:**
    1. Move `veldmodules/data-provider` to `services/data-provider` as a library plus thin WASI binary and delete `veldmodules/data-provider` after verification.
    2. Implement `Provider` on `127.0.0.1:50051` using the result envelopes and error mapping from Section 2.
    3. Preserve OData search, `FRESH_DAYS` fallback widening, scene/part folding, S3 SigV4 signing, manifest parsing, and MGRS UTM math. Access external HTTP/S through `wasi:http` with host TLS, bounded streaming, and the redirect/credential policy in Section 1.3.
    4. Add fixture-based tests and a real WASI RPC smoke test against local CDSE/S3 fixtures, including internal continuation, nonempty and empty listings, and terminal `truncated` metadata; keep live CDSE verification separate from deterministic tests.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-provider --target wasm32-wasip3 --release`, artifact interface inspection, the WASI RPC and outbound HTTP scenarios, and `! test -d veldmodules/data-provider` succeed.

- [ ] **Task 8: Migrate `services/tile-cache`**
  - **Action:**
    1. Move the cache to `services/tile-cache`, expose its domain library, serve `Cache` on `127.0.0.1:50054`, and delete `veldmodules/tile-cache` after verification.
    2. Read/write QOI tiles under `/runtime/data/tiles/` with atomic temporary writes, rename, and LRU eviction under `cache_limit_mb`. Classify both `PlaceTiles` and `QueryTiles` as `Bulk`, enforce per-tile and total encoded request limits, and reject batches exceeding the 128 MiB route ceiling rather than relying on the example's 4 MiB mutation classification.
    3. Distinguish misses, corrupt individual entries, and fatal store errors. Test atomic recovery, eviction, non-terminal item errors, maximum tile payloads, and oversized batches.
    4. Start the reusable transport-parameterized RPC conformance harness from the existing Cache WASI scenario. Cover route limits, recoverable/terminal errors, bounded streams, cancellation, and producer termination; later tasks add their contracts without creating a second harness.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-tile-cache --target wasm32-wasip3 --release`, artifact interface inspection, the WASI place/query/eviction/limit/cancellation scenario, and `! test -d veldmodules/tile-cache` succeed.

- [ ] **Task 9: Migrate `services/data-library`**
  - **Action:**
    1. Move the library to `services/data-library`, serve `Library` on `127.0.0.1:50052`, and delete `veldmodules/data-library` after verification.
    2. Implement `.part` resumable downloads through `wasi:http` with host TLS, `.origin` sidecars, pause/remove behavior, and scene counts under `/runtime/data/dem/source`. Keep these files as the only persistent authorities; do not add SQLite or a duplicate metadata store without a new requirement and plan revision.
    3. Use generated Provider clients for signing/root resolution and re-sign expired credentials before new or resumed requests. Implement `WatchDownloads` directly over authoritative domain state with complete owned snapshots, post-mutation invalidation, coalesced ephemeral progress, and cancellation according to Section 2.4.
    4. Test interrupted ranges, signature expiry/re-signing, resume validation, pause/cancel, redirect and cross-origin credential rejection, initial/reconnect snapshots, invalidation during snapshot delivery, progress coalescing, and per-item failures against a local HTTP fixture and mock Provider.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-library --target wasm32-wasip3 --release`, artifact interface inspection, a real WASI outbound download plus RPC snapshot-stream smoke scenario, and `! test -d veldmodules/data-library` succeed.

- [ ] **Task 10: Migrate `services/image-tiler`**
  - **Action:**
    1. Move the tiler to `services/image-tiler`, serve `Tiler` on `127.0.0.1:50053`, and delete `veldmodules/image-tiler` after verification.
    2. Preserve all TIFF, JP2/OpenJP2, excerpt, codec, JPEG, PNG, NetCDF/HDF5, grid, table, radiometry, resample, and budget adapters with typed errors. Replace sampled-FNV cache identity with the Task 6 source-identity semantics rather than preserving stale-cache behavior.
    3. Open sources through `veldmap_core::range`, verify request fingerprints, stream tiles/progress/item errors, cache QOI asynchronously, and stop when clients cancel.
    4. Test every adapter on small fixtures, actual WASI OpenJP2 decoding and HTTP Range reads, a cancelled production pass, and stale-cache rejection after a same-length mid-file local edit.
  - **Verification:** native adapter tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-image-tiler --target wasm32-wasip3 --release`, artifact interface inspection, the WASI describe/produce/cache/cancel/mutation scenario, and `! test -d veldmodules/image-tiler` succeed.

- [ ] **Task 5b: Prove the Basic `wasi-gfx` Presentation and RPC Stack**
  - **Action:**
    1. Pin the exact graphics host/guest/WIT set and create minimal `services/data-browser` without migrating the legacy UI. Expose `shell` and `frame` as Rust library items through `src/lib.rs`; keep `src/main.rs` as a thin standard `wasi:cli/command` entry point, not a custom WIT runner.
    2. Implement the native-async executor and wgpu/WASI integration in the graphics path of the shared `veldmap-host`. Link the complete required WASI and graphics host interfaces, including sockets, clocks, filesystem, CLI, WebGPU, and surface. Prove real font/config reads and least-privilege loopback access.
    3. Render text and basic iced widgets through wgpu 30, present through `wasi-gfx`, and composite an animated external RGBA image without per-frame atlas churn. Distinguish logical coordinates from physical framebuffer pixels in the event layer from the start; document the initial scale represented by `surface@0.2.0` rather than hardcoding an assumed `ui_scale = 2.0`. Exercise the real pointer, keyboard text, resize, and frame events already present in the pinned public surface contract.
    4. Complete a real ConnectRPC call from the graphics component to an existing WASI service while the surface remains operational. Record verified host launch commands and interface locks in `docs/operations.md`.
  - **Verification:** existing workspace tests pass; `cargo build -p veldmap-data-browser --target wasm32-wasip3` succeeds with zero warnings; artifact interface inspection passes; and a real window smoke scenario passes rendering, animation, text entry, currently contracted pointer/keyboard/resize events, filesystem reads, and concurrent RPC. Compilation, self-injected events, or software rendering alone does not complete this task.

- [ ] **Task 11: Migrate `services/image-view`**
  - **Action:**
    1. Move the viewer to `services/image-view`, serve `Viewer` on `127.0.0.1:50055`, and delete `veldmodules/image-view` after verification.
    2. Use Tiler and Cache clients plus `veldmap_core::tiles` for the pyramid ladder.
    3. Render wgpu or software RGBA frames, validate dimensions/stride, and broadcast latest frames per view. Implement `WatchViews` with complete owned `ViewStatus` snapshots, post-mutation invalidation, progress coalescing, and cancellation according to Section 2.4.
    4. Test fit/zoom/pan, resize, variable selection, frame sequencing, snapshot refresh/reconnect, cancellation, and real `WatchViews` and `WatchFrames` streams.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-image-view --target wasm32-wasip3 --release`, artifact interface inspection, the transport-parameterized contract scenarios, and a headless WASI frame/status smoke scenario pass; `! test -d veldmodules/image-view` succeeds.

- [ ] **Task 12: Migrate `services/globe`**
  - **Action:**
    1. Move the globe to `services/globe`, serve `Globe` on `127.0.0.1:50056`, and delete `veldmodules/globe` after verification.
    2. Preserve WGS84 geometry, `SURFACE_LIFT_M`, warp subdivision, culling, georeference ranks, Transverse Mercator/Pulkovo math, outlines/hatching, and quaternion flights.
    3. Render wgpu or software frames and broadcast latest frames with bounded queues and cancellation. Implement `WatchOverlays` with complete owned overlay-status snapshots, post-mutation invalidation, and progress coalescing according to Section 2.4.
    4. Add golden math/culling tests and a headless WASI scenario covering overlay snapshots/reconnects, camera control, surface probing, frames, and cancellation.
  - **Verification:** native tests, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-globe --target wasm32-wasip3 --release`, artifact interface inspection, the transport-parameterized contract scenarios, and the headless WASI smoke scenario pass; `! test -d veldmodules/globe` succeeds.

- [ ] **Task 5c: Complete Public Surface Input and Lifecycle Contracts**
  - **Action:**
    1. Compare the pinned public surface with the required desktop events. If still incomplete, make the minimal generic `wasi-gfx` and runtime forks described in Section 1.4, submit the upstream pull request, and pin the staging WIT and host revisions without introducing VeldMap-specific host interfaces.
    2. Use or add pointer-button identity, wheel, scale-factor/HiDPI, focus/IME, clipboard, and application-close integration with explicitly defined units, coordinates, and lifecycle semantics. Keep clipboard and IME capabilities separate from basic surface events; do not claim OS accessibility without a separate public contract and host implementation.
    3. Add deterministic diagnostics triggered by observed real host events and prove clean stream cancellation and shutdown. Automate pointer, wheel, keyboard, and resize scenarios with OS-level injection such as `xdotool` under WSLg where supported, and verify clipboard content with a native clipboard tool; guest-self-injected events do not count. Record a manual host-to-guest checklist for IME and any HiDPI/focus cases the host cannot automate, including observed evidence rather than marking them skipped.
    4. Upstream merger is not a completion prerequisite, but an undocumented private fork or self-injected-only verification is insufficient.
  - **Verification:** the automated and documented manual real-window scenarios pass pointer buttons/motion, wheel, keyboard text, resize/HiDPI, focus/IME, clipboard, concurrent RPC, and clean shutdown; any staging revisions and pull request, or evidence that no fork was needed, are recorded; and the component passes artifact interface inspection.

- [ ] **Task 13: Complete `services/data-browser`**
  - **Action:**
    1. Extend the Task 5c shell with the legacy browser behavior and delete `veldmodules/` completely after verification.
    2. Preserve panes/tabs, layout persistence, catalogue/search/download tables, row states, batch actions, NetCDF selectors, and camera interaction. Replace collections on each snapshot while using stable item IDs to preserve selection, expansion, and scroll state.
    3. Connect to all six backend/renderer services with generated ConnectRPC clients over WASI TCP and iced subscriptions; composite latest viewer/globe RGBA frames received through ConnectRPC. Reconnect collection watches with fresh snapshots rather than replay cursors.
    4. Coalesce camera input with bounded pending state and at most one in-flight `ControlCamera` RPC per view or globe. Accumulate pan/orbit deltas, preserve zoom anchoring and discrete-action ordering, collapse resize to latest dimensions, and do not emit one RPC per raw pointer event.
    5. Replace Russian three-form plural selection and forced comma decimals with English formatting, remove guillemets, and cover 1, 2, 11, 21, and 101 plus decimal units with focused tests.
    6. Remove the legacy `Reveal` message and “Show in the file manager” action because WASI has no approved reveal capability. Do not recreate that private host call; offer “Copy path” through the approved clipboard interface only if useful.
    7. Exercise a complete user flow against local fixtures, including recoverable item errors, snapshot replacement/reconnects, live view resizing, clipboard/focus/IME through approved public interfaces, and clean stream cancellation. Do not claim OS accessibility from internal widget semantics alone.
  - **Verification:** native tests where applicable, native/WASI clippy with `-D warnings`, `cargo build -p veldmap-data-browser --target wasm32-wasip3 --release`, artifact interface inspection, the real end-to-end window scenario, and `! test -d veldmodules` succeed.

- [ ] **Task 14: Orchestrate and Certify the Reference System**
  - **Action:**
    1. Create `process-compose.yaml` for exactly seven WASI 0.3 components using the shared `veldmap-host` embedder with directory preopens, per-address socket checks, readiness probes, and dependency ordering.
    2. Use standard Connect health checks for backend readiness and keep service logs under `runtime/logs/`.
    3. Complete the conformance harness started in Task 8. Run transport-parameterized scenarios for every service edge and both frame streams, covering validation, route/body limits, deadlines, `ItemError` versus `ConnectError`, cancellation and producer termination, finite/infinite stream termination, snapshot ordering/replacement/reconnect/coalescing, bounded queues, and latest-frame overwrite.
    4. Freeze a deterministic end-to-end user-flow and RGBA frame-content oracle: exact comparisons for deterministic software output and the documented per-channel/raster-edge tolerance for wgpu readback. Separately run reference-only topology checks: real socket cancellation/failure, backend process termination observed as RPC failure, per-process filesystem/network denial, and exactly seven delivered artifacts. These process and capability checks are not requirements for the single-component profile.
    5. Update the 35-line README and concise `docs/overview.md`, `services.md`, `imagery-and-globe.md`, and `operations.md` (each at most 80 lines). Document exact pins, reference launch, deterministic fixtures, and optional live CDSE without exposing secrets.
    6. Measure and record representative reference `WatchFrames` throughput and memory at the configured window size, plus snapshot-refresh cost under representative progress updates, before choosing resolution or rate limits.
  - **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace --locked`, native/WASI workspace clippy with `-D warnings`, and `cargo build --workspace --target wasm32-wasip3 --release --locked` pass; all seven artifacts pass interface inspection; and the complete contract, topology, and `process-compose` user-flow scenarios pass.

- [ ] **Task 15: Build and Measure the Composed Release**
  - **Action:**
    1. Create `apps/veldmap` by statically linking the existing service libraries into one WASI 0.3 component. Connect generated clients to their existing routers through in-memory `ClientTransport` and `ConnectRpcService`, preserving HTTP/protobuf bodies and the complete contract-adapter boundary; do not call domain stores directly from clients or open internal loopback listeners.
    2. Run the same transport-parameterized contract scenarios and deterministic user/RGBA oracle used by Task 14. Use transport-level fault injection for composed failure cases. Document the union of host capabilities rather than claiming per-service process isolation.
    3. Benchmark startup, catalogue operations, tile production, 4K frame delivery, snapshot refresh, memory, CPU, p99 input-to-present latency, and p99 event-loop stall against the reference baseline. Introduce only one measured optimization at a time and rerun parity after each; a failed parity result blocks adoption regardless of speed.
    4. Add cooperative yield points to long decode/production loops so CPU-heavy work cannot indefinitely stall the UI or other services on the current-thread executor. Replace independent legacy allowances with a checked aggregate linear-memory, frame-buffer, tile-cache, and VRAM budget sized for one wasm32 component well below its address-space ceiling.
    5. If GPU texture sharing is justified, use the Section 1.4 shared-device frame adapter with bounded latest-frame lifetime and deterministic RGBA readback. Further transport or binary rewriting requires separate design review and repeats artifact inspection and parity.
    6. Record measurements and optimizations not adopted, and remove abandoned experiments under Section 0.
  - **Verification:** exactly one composed `wasm32-wasip3` release artifact builds and passes interface inspection after every adopted transformation; native/workspace tests, shared contract parity, composed fault-injection checks, exact software and tolerance-based wgpu RGBA parity, aggregate budget checks, and the complete release user-flow smoke scenario pass; benchmark results include p99 latency/stalls without claiming unmeasured zero-copy behavior.
