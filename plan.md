# plan.md — VeldMap Modular Monolith Migration Plan

## 0. Agent Execution Protocol

Each chat session executes one task from Section 5 and treats this file as the migration contract.

1. Locate the first unchecked task, or the task explicitly requested by the user.
2. Implement only that task. Do not jump ahead or silently shrink its verification.
3. Follow these rules in every task:
   - Write English only in source, comments, UI text, logs, documentation, and commit messages. Do not introduce Cyrillic characters (`[\u0400-\u04FF]`).
   - Use self-documenting names, small functions, strong domain types, and `thiserror` errors. Keep comments exceptional: unsafe invariants, external protocol or OS quirks, and one-line mathematical citations. Keep comments below 5% of lines in each Rust file.
   - Do not restore `veldcore/platform/host`, `veldsdk`, `buildgen/`, `schema.yaml`, legacy host configs, `generated/` directories, or the `ui-service` process/ABI.
   - Use stable Rust 1.99.1, Rust 2024, workspace dependency inheritance, workspace lints, and modern module paths (`src/foo.rs` plus `src/foo/bar.rs`, not `src/foo/mod.rs`). Do not use a beta toolchain or the compiler's `future` edition.
   - Native delivery uses `apps/veldmap`; browser delivery uses the same application compiled to `wasm32-unknown-unknown`. Do not introduce WASI, Wasmtime, internal ConnectRPC, or internal protobuf transport.
   - Add only essential tests for externally observable behavior and non-obvious invariants. Do not test getters, trivial wiring, or dependency behavior, and do not duplicate integration coverage with unit tests. Geodesy, projection, codecs, cache eviction, resumable downloads, snapshot invalidation/coalescing, cancellation, database migrations, and storage recovery require correctness tests.
   - Keep README at 35 lines or fewer and documents under `docs/` at 80 lines or fewer. Record exact tool pins and verified launch commands in `docs/operations.md`.
4. A replacement may coexist with its predecessor only while being verified. After the replacement passes its complete behavioral and real runtime scenario, remove superseded source, dependencies, feature flags, compatibility shims, configuration, documentation, and unused lockfile entries in the same task. Do not retain a second implementation "just in case".
5. Run the complete verification for the current task, including its real runtime scenario. Compilation alone is not migration verification.
6. Only after verification passes, change that task's checkbox from `- [ ]` to `- [x]`. Do not add session notes or alter other task descriptions.
7. If an external platform assumption fails, record the evidence and revise this plan before building later tasks on that assumption. Pin a verified replacement rather than silently accepting a reduced platform or an unverified fallback.
8. Start with the four packages in Section 1.2. A new crate requires a demonstrated dependency-isolation, packaging, or independent-reuse need; a domain name alone is not sufficient justification.

---

## 1. Target Architecture

### 1.1 Product Shape

VeldMap is one trusted, modular application with ordinary typed Rust calls between modules. There is no internal transport, service orchestration, component sandbox, or requirement to preserve protobuf contracts for internal calls.

The native application is the first complete product. The browser application reuses domain, rendering, and storage logic through thin platform adapters. Android and iOS are deferred until explicitly required; winit compatibility alone is not evidence of mobile readiness.

External CDSE/S3 HTTP remains a network boundary. Browser use does not imply a backend proxy, but a proxy may be reconsidered if real CORS, credential, or signing requirements cannot be satisfied safely.

### 1.2 Workspace Shape

The final workspace starts deliberately small:

```text
veldmap/
├── apps/
│   └── veldmap/
│       └── src/
│           ├── main.rs             # Native entry point
│           ├── lib.rs              # Shared UI and browser entry point
│           ├── app.rs              # Composition root
│           └── platform/
│               ├── native.rs
│               └── web.rs
├── crates/
│   ├── veldmap-core/
│   │   └── src/
│   │       ├── provider.rs
│   │       ├── library.rs
│   │       ├── tiler.rs
│   │       ├── cache.rs
│   │       ├── viewer.rs
│   │       └── globe.rs
│   ├── veldmap-render/
│   └── veldmap-storage/
├── runtime/
├── docs/
└── scripts/
```

`veldmodules/` is retained temporarily as migration input, outside the Cargo workspace and final architecture. The former protobuf, host, RPC, Buf, and WASI scaffolding was removed in Task 1.

Dependency direction:

```text
apps/veldmap → veldmap-core
apps/veldmap → veldmap-render  → veldmap-core
apps/veldmap → veldmap-storage → veldmap-core
```

- `veldmap-core` owns domain types, application behavior, and consumer-defined ports. Provider, library, tiler, persistent cache, viewer state, and globe state are modules, not separate crates.
- `veldmap-render` owns wgpu pipelines, GPU resources, the bounded GPU tile cache, and viewer/globe draw behavior. Camera math and tile selection remain in core.
- `veldmap-storage` owns SQL migrations, synchronous repositories, the database owner, and native/browser SQLite drivers.
- `apps/veldmap` owns Iced presentation, subscriptions, native/web HTTP and blob adapters, runtime selection, and composition. Only the composition root knows every concrete implementation.
- Core does not depend on Iced, wgpu, winit, rusqlite, Tokio, wasm-bindgen, or browser JavaScript APIs.

Do not create separate `ports`, `app`, `ui`, `platform-native`, or `platform-web` crates preemptively. Extract an imagery/codec crate later only if its native and browser dependency sets or compile boundaries justify it.

### 1.3 In-Process APIs and Ports

Application modules expose typed operations such as `search`, `place_downloads`, `open_raster`, `produce_tiles`, and `control_camera`. They do not expose RPC DTOs, worker messages, SQL, connection types, or transport errors.

Use ports only at real platform boundaries:

- HTTP responses and streaming Range bodies.
- Blob storage and local files.
- Clocks and timers.
- Background execution and browser workers.
- SQLite drivers where their APIs genuinely differ.

Define each port beside its consumer. Do not wrap every domain object in a trait and do not create a universal platform abstraction.

Native scheduling may use Tokio. Browser scheduling uses `wasm-bindgen-futures` and workers. Domain code is runtime-independent, and CPU-heavy decoding never blocks the UI executor.

### 1.4 State, Snapshots, and Cancellation

- Collection subscriptions publish complete current snapshots, not durable mutation history. Stable IDs preserve selection, expansion, and scroll state.
- Notifications are ephemeral invalidations, not authoritative data. Reconnection obtains a fresh snapshot.
- Subscribe before constructing the initial snapshot, invalidate after successful mutations, and coalesce bursts without losing the latest state.
- Download progress is high-frequency ephemeral state. Persist lifecycle transitions and throttled resume checkpoints, not every network chunk.
- Dropping an operation or subscription cancels its work. Bounded queues never grow without limit; ordinary mutations are not silently dropped.
- Camera input is coalesced before changing application state or starting work. Resize collapses to the latest dimensions.

No event log, replay cursor, event-outbox framework, or generic observable-store framework is introduced without a new synchronization requirement.

---

## 2. Dependency Baseline

### 2.1 Toolchain and Reproducibility

Use Rust `1.99.1`, edition 2024, and `rust-version = "1.99"`. Commit `Cargo.lock` and use `--locked` for verification. Exact direct-dependency pins are allowed for this application, but they do not establish compatibility by themselves.

The starting candidates are:

| Component | Initial pin or policy |
|---|---|
| iced | Git revision `cd04b74ca7073766f654535d6e72c452ef749d07` (`0.15.0-dev`) until an equivalent release passes the same gates |
| wgpu | `=30.0.1`, unified with Iced |
| winit | No independent crates.io pin; use Iced's integration or its exact winit Git revision if direct types are necessary |
| rusqlite | `=0.41.0`, `bundled`, native only |
| sqlite-wasm-rs | `=0.6.2`, `wasm-bindgen`, browser spike candidate |
| sqlite-wasm-vfs | `=0.3.0`, initial `sahpool` candidate subject to the multi-tab decision |
| wasm-bindgen | `=0.2.129` |
| wasm-bindgen-cli | Exactly matches the resolved wasm-bindgen crate; installed as a tool, not added as a library dependency |
| wasm-bindgen-futures | `=0.4.79` |
| futures | `=0.3.34` |
| tokio | `=1.54.1`, native adapters and entry point only |
| bytes | `=1.12.1` |
| serde | `=1.0.229`, derive where required |
| thiserror | `=2.0.21` |
| anyhow | `=1.0.104`, entry points and composition only |

The pinned Iced revision uses wgpu 30 and its own winit Git revision. A crates.io winit package with the same version number is not necessarily the same crate instance and must not be mixed into Iced window/event types.

Target-specific dependencies belong in the relevant app, render, or storage crate. Do not put native and browser driver dependencies indiscriminately in the root manifest.

### 2.2 HTTP and Decoder Selection

Task 3 selects and exactly pins the native HTTP implementation after proving streaming Range bodies, timeouts, cancellation, redirect control, and TLS. Prefer a high-level client such as reqwest if it passes; do not assume its browser implementation satisfies VeldMap's streaming requirements.

The browser HTTP adapter uses the Fetch API through project-owned code. Native and browser adapters implement the same consumer-owned HTTP port, but need not share a client crate.

OpenJPEG retains its native implementation initially. Its browser Emscripten build, worker protocol, cancellation, and representative Sentinel decoding are a separate gate. Do not assume native and browser codec builds produce identical output without fixtures and tolerances.

### 2.3 Exclusions

- No WASI target, Wasmtime host, ConnectRPC, Buffa, or internal protobuf.
- No ORM or generic database framework.
- No PGlite or parallel browser storage architecture.
- No second UI toolkit. Egui is not introduced during this migration.
- No custom rendering shell while Iced's shader-primitive integration satisfies the requirements.

---

## 3. Rendering, Storage, and Data Ownership

### 3.1 Iced and Shared GPU Resources

Iced owns the application window/canvas and its wgpu device, queue, encoder, and submission. Viewer and globe integrate through Iced shader widgets and custom primitives. Their pipelines receive Iced's device and queue and share GPU textures through application-owned resource state.

Do not create an independent `RenderContext` in `App::new`, a second surface, or a parallel command-submission shell unless a measured limitation is demonstrated and this plan is revised.

- `DecodedTile` belongs to core and contains validated dimensions, row stride, and `Bytes` pixels.
- `GpuTile` belongs to render and is shared as `Arc<GpuTile>` between viewer and globe.
- GPU upload consumes or borrows the decoded payload once and does not retain a duplicate CPU copy without a documented reason.
- The persistent QOI cache and the GPU tile cache are distinct types with separate byte budgets.
- No RGBA frame streams, production GPU readbacks, or per-frame CPU pixel transport are permitted. Diagnostic readbacks are allowed only in tests and explicit screenshot/debug operations.
- QOI is for persistent-cache serialization, not draw calls or inter-module frame transport.
- GPU uploads still copy. "Zero avoidable copies" means eliminating application-level duplication, not claiming impossible end-to-end zero-copy.

### 3.2 SQLite Scope and Schema Style

SQLite stores current-state metadata, not entity history. Initial candidates are `downloads`, `sources` where query requirements justify them, and `settings`. A `tile_entries` index is added only after measuring a need; otherwise the existing file-based cache remains authoritative.

Keep downloaded imagery, `.part` files, and QOI payloads in file/blob storage. SQLite stores metadata and references, not large raster blobs.

Use small numbered SQL migrations with explicit foreign keys, checks, and purposeful indexes. Each migration runs in a transaction and is tested from every released schema version. Prefer ordinary tables and repository SQL over trigger-based APIs, soft-delete lifecycles, replay cursors, and append-only state snapshots.

- Native has one database owner thread; browser has one logical database owner worker according to the selected multi-tab policy.
- Repository implementations are synchronous inside the owner. Public application operations may be asynchronous channels or messages.
- Every repository operation is transaction-complete. Never hold a transaction across `.await`, a worker message, file/network I/O, or a render operation.
- SQLite integers are signed 64-bit values. IDs and unsigned byte counters require checked conversion and an explicit upper bound.
- Set connection pragmas on every connection and test required SQLite features plus `sqlite_version()` on each driver.
- Keep a private driver-compatibility trait only if real native/browser differences require one. Do not build a public generic database abstraction.

### 3.3 Files, Downloads, and Cache Recovery

Downloads retain `.part` files, Range resume validation, signature refresh, pause/remove behavior, and atomic completion renames. Database metadata is reconciled with actual files at startup and after interrupted writes.

When SQLite replaces `.origin` sidecars, import existing metadata without deleting user imagery, verify the imported state, and remove the old writer and duplicate authority in the same migration task. Do not maintain indefinite dual writes.

Persistent tile writes use temporary files and atomic rename where the platform supports it. Eviction is bounded and tested for corrupt entries, interrupted writes, active sources, and cache-budget headroom. Database and filesystem state must not be assumed to commit atomically together.

### 3.4 Browser Persistence and Multiple Tabs

A dedicated worker in every tab is not one origin-wide owner. Task 4 must choose and verify one policy:

- Explicit single-tab behavior with a clear second-tab response.
- A shared owner or coordinated ownership mechanism.
- A VFS that supports the required multi-tab behavior directly.

`sahpool` is acceptable only if its concurrency model matches that policy. Do not assume WAL provides OPFS concurrency. Test migrations, short transactions, reload persistence, quota/permission failures, blob references, busy handling, and the selected second-tab behavior in real browsers.

---

## 4. Cross-Cutting Verification

The following are architecture gates, not optional cleanup:

1. One Iced-owned wgpu device/queue serves UI, viewer, and globe without per-frame CPU pixel transport.
2. Real CDSE/S3 requests pass native and browser signing, CORS where applicable, redirect, cancellation, and streaming Range scenarios.
3. Representative Sentinel JP2 imagery decodes in a browser worker with bounded memory and responsive cancellation.
4. Native and browser repositories pass shared contract tests, migrations, restart persistence, corruption handling, and the chosen multi-tab scenario.
5. CPU and GPU caches have measured aggregate budgets; skipped frames/snapshots cannot grow queues or retain unbounded textures.
6. The complete native user flow and the supported browser user flow run against deterministic fixtures, with separately identified live-CDSE smoke verification.
7. No production path depends on GPU readback, internal RPC, WASI, `veldsdk`, or legacy generated contracts.

---

## 5. Sequential Work Plan

### Task 1: Establish the Stable Modular Workspace

- [x] **Action:**
  1. Pin stable Rust `1.99.1`, add `wasm32-unknown-unknown`, and create `apps/veldmap`, `crates/veldmap-core`, `crates/veldmap-render`, and `crates/veldmap-storage` with the dependency direction in Section 1.2.
  2. Remove `proto/`, `host/`, `crates/veldmap-proto`, `crates/veldmap-rpc`, Buf/WASI configuration and scripts, the legacy host policy, and their dependencies. Retain `veldmodules/` outside the workspace as migration input.
  3. Establish shared lints, exact candidate pins, lockfile verification, and minimal native/browser entry points. Do not migrate domain modules or create additional crates.
  4. Update README and operations documentation to describe the transitional architecture and actual commands.
- **Verification:** `cargo fmt --all -- --check`, workspace tests, native clippy with `-D warnings`, and native plus `wasm32-unknown-unknown` builds of the new packages succeed with `--locked`; legacy-reference scans confirm that only `veldmodules/` and migration documentation mention superseded transport architecture.

### Task 2: Prove Shared Iced/wgpu Rendering on Native and Browser

- [ ] **Action:**
  1. Build a minimal Iced application with custom shader primitives, two viewport types, and one shared GPU tile representation. Use Iced's device/queue rather than an external rendering shell.
  2. Render the same uploaded tile in viewer-like and globe-like geometry while ordinary Iced widgets remain interactive. Exercise input, clipping, resize, scale factor, and surface recreation where supported.
  3. Build the same path for a browser canvas. Confirm wgpu/Iced/winit source unification with `cargo tree` and resolve type/source duplication rather than adding compatibility wrappers.
  4. Measure tile upload count, CPU pixel traffic, frame memory, and behavior under bounded cache pressure. Keep diagnostic readback out of production.
- **Verification:** real native-window and supported-browser scenarios pass rendering, input, resize, shared-texture, and memory assertions; native/wasm builds and clippy pass; no per-frame CPU pixel transport or second GPU device exists. Failure blocks further rendering migration and requires plan revision.

### Task 3: Prove External HTTP and Browser OpenJPEG

- [ ] **Action:**
  1. Implement the minimal consumer-owned streaming HTTP port and select/pin native and browser adapters. Prove bounded Range reads, cancellation, timeouts, redirect rejection for signed requests, and no credential forwarding across origins.
  2. Exercise deterministic local CDSE/S3 fixtures, including expired-signature refresh and continuation. Run separate live CDSE/S3 checks for native and browser CORS/signing without recording secrets.
  3. Build the browser OpenJPEG worker spike and decode representative Sentinel JP2 inputs. Define bounded worker messages, ownership transfer, cancellation, error reporting, and feature/version diagnostics.
  4. Record exact selected dependencies, browser headers, codec revisions, and reproducible commands. Remove superseded spike implementations.
- **Verification:** native and browser HTTP fixture scenarios plus live smoke checks pass; representative browser decoding passes content and memory checks and cancels promptly; no UI-thread decode or unbounded response buffering occurs. Failure requires revising the affected adapter before full provider/tiler migration.

### Task 4: Establish SQLite Ownership and Browser Persistence Policy

- [ ] **Action:**
  1. Create versioned SQL migrations and the native database owner in `veldmap-storage`. Implement a small current-state download repository with checked integer conversions and transaction-complete operations.
  2. Implement the browser owner and driver behind the same repository contract. Select the VFS and explicit single-tab, shared-owner, or multi-tab policy from Section 3.4.
  3. Add only driver-compatibility abstraction demonstrated by the two implementations. Test connection pragmas, migration transactions, rollback, restart persistence, busy/error handling, and `sqlite_version()`/required features.
  4. Exercise metadata references to external blobs and interrupted/reconciled writes. Do not put raster payloads or an event log in SQLite.
- **Verification:** shared native/browser repository contract tests and real reload/second-tab scenarios pass; migrations from an empty and prior test schema pass; native/wasm clippy and builds pass; no transaction crosses asynchronous or external I/O boundaries.

### Task 5: Migrate Pure Domain and Application Foundations

- [ ] **Action:**
  1. Migrate pyramid/tile math, geodesy/projection, camera state, range readers, source identity, byte budgets, configuration expansion, and frame/pixel validation from legacy modules into `veldmap-core`.
  2. Replace `veldsdk` resources and protobuf values with owned domain types, `Bytes`, typed errors, and consumer-owned ports. Keep GPU textures in render and SQL in storage.
  3. Implement collision-resistant source identity, including detection of same-length mid-file local edits. Preserve verified immutable remote version identities where hashing is unnecessary.
  4. Implement bounded range-block pooling/readahead and cancellation without tying domain code to Tokio or browser APIs.
- **Verification:** golden geodesy/projection tests, pyramid/budget/range/source-mutation tests, native tests, native/wasm clippy, and real local/HTTP range scenarios pass; new core has no Iced/wgpu/rusqlite/Tokio/wasm-bindgen dependency.

### Task 6: Migrate Provider as Typed In-Process Logic

- [ ] **Action:**
  1. Migrate OData search, fresh-window widening, scene/part folding, S3 listing and SigV4 signing, manifest parsing, imagery selection, and MGRS/UTM behavior into `core::provider`.
  2. Connect the Task 3 HTTP port directly through typed operations. Preserve bounded internal continuation and terminal truncation information without exposing transport DTOs.
  3. Preserve credential redaction, expiry refresh, redirect policy, and deterministic errors. Remove the legacy Provider RPC boundary after equivalent scenarios pass.
- **Verification:** fixture tests cover search, empty/complete/truncated listings, scene folding, manifests, signing, and errors; a real typed native catalogue search and the supported browser search pass; live CDSE smoke remains separate and secret-free.

### Task 7: Migrate Downloads and Library Persistence

- [ ] **Action:**
  1. Implement library queueing, concurrent-download limits, Range resume, pause/remove, scene counts, signature refresh, and atomic completion through typed provider/HTTP/blob operations.
  2. Import existing `.origin` metadata and actual file state into SQLite without deleting user imagery. Reconcile interrupted downloads and remove the superseded sidecar writer after verification.
  3. Publish complete download snapshots with stable IDs, post-mutation invalidation, race-free initial subscription, coalesced ephemeral progress, and cancellation.
  4. Persist lifecycle transitions and bounded resume checkpoints rather than per-chunk progress. Keep `.part` payloads outside SQLite.
- **Verification:** interrupted ranges, expiry/re-signing, resume validation, pause/cancel/remove, startup reconciliation, legacy-sidecar import, initial/reconnect snapshots, invalidation during delivery, and per-item failures pass against local fixtures; real native download/restart scenarios pass without dual metadata authorities.

### Task 8: Migrate Tiler and Persistent Tile Cache

- [ ] **Action:**
  1. Migrate TIFF, JP2/OpenJP2, excerpt, codec, JPEG, PNG, NetCDF/HDF5, grid, table, radiometry, resampling, and budget adapters into core with typed errors and background execution.
  2. Produce validated `DecodedTile` values and stream progress/item errors through typed operations. Verify source fingerprints before work and stop promptly on cancellation.
  3. Migrate QOI file persistence, atomic recovery, source protection, and bounded LRU eviction. Keep the persistent cache distinct from the GPU cache; add a SQLite tile index only if measurements justify it.
  4. Connect native OpenJPEG and the verified browser decoder worker without duplicating domain decoder logic.
- **Verification:** every adapter passes small-fixture tests; actual native and browser JP2 decoding, HTTP Range reads, cancellation, QOI recovery/eviction, budget limits, and stale-cache rejection after same-length mutation pass; aggregate CPU memory remains bounded.

### Task 9: Complete the Native 2D Vertical Slice

- [ ] **Action:**
  1. Implement viewer camera control, fit/zoom/pan, resize, variable selection, tile ladder, and typed tile production in core.
  2. Implement the viewer pipeline and bounded `Arc<GpuTile>` cache in render using Task 2's Iced primitive integration. Share device/queue with the UI and upload each decoded tile without a duplicate retained pixel payload.
  3. Build the initial Iced catalogue/search/download/open workflow and download subscriptions. Coalesce pointer input and preserve view state across snapshot replacement.
  4. Remove superseded image-view and tile-cache RPC paths after behavioral parity.
- **Verification:** a complete native flow searches a fixture catalogue, downloads/opens a raster, changes variables, navigates/resizes the view, and survives restart; tile content and cancellation tests pass; profiling shows no per-frame CPU pixel transport and bounded CPU/GPU memory.

### Task 10: Complete the Supported Browser Application

- [ ] **Action:**
  1. Compose provider, library, tiler, viewer, storage, and rendering with browser Fetch, OPFS/blob, database-owner, and decoder-worker adapters.
  2. Implement browser-specific path, quota, permission, reload, cancellation, and second-tab behavior without leaking platform types into core.
  3. Reuse the native Iced workflow and verify responsive UI during network, database, and decode work. Keep worker payloads bounded and ownership transfers explicit.
  4. Update operations documentation with required headers, browser support, worker startup, storage policy, and live-CDSE limitations.
- **Verification:** the complete supported browser flow passes against deterministic fixtures in each supported browser, including reload persistence and the chosen multi-tab scenario; live CDSE/S3 CORS smoke passes or the plan is revised; no UI-thread blocking or per-frame CPU pixel transport occurs.

### Task 11: Migrate Globe and Shared Overlays

- [ ] **Action:**
  1. Migrate WGS84 geometry, surface lifting, warp subdivision, culling, georeference ranks, Transverse Mercator/Pulkovo math, outlines/hatching, overlays, and quaternion flights into core.
  2. Implement globe rendering through Iced primitives with the same GPU resources and `Arc<GpuTile>` values used by the viewer. Publish overlay snapshots rather than frames or event history.
  3. Implement overlay ordering, opacity, visibility, variable selection, surface probing, camera flights, and coalesced orbit/zoom/resize input in the shared UI.
  4. Remove legacy globe RPC/GPU resource code after native and browser parity scenarios pass.
- **Verification:** golden math/culling tests and real native/browser globe scenarios pass overlay snapshots/reconnects, camera control, probing, resize, cancellation, shared-tile reuse, and bounded memory; no RGBA frame stream or production readback exists.

### Task 12: Complete Presentation and Remove Superseded Architecture

- [ ] **Action:**
  1. Complete panes/tabs, layout persistence, catalogue/search/download tables, row states, batch actions, NetCDF selectors, settings, and viewer/globe interactions using Iced and current-state storage.
  2. Preserve English plural/decimal formatting, accessible error presentation, stable selection across snapshots, bounded input coalescing, and clean cancellation/shutdown. Do not claim OS accessibility or mobile support without separate verification.
  3. Remove `veldmodules/` and any remaining migration-only references or unused lockfile entries after all replacement scenarios pass.
  4. Update README and concise operations/architecture documentation to match the delivered native/browser application and exact tool pins.
- **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace --locked`, native and `wasm32-unknown-unknown` clippy with `-D warnings`, release native/browser builds, complete native and supported-browser user flows, restart/migration scenarios, memory measurements, and legacy-reference scans all pass. Exactly the modular application architecture remains; no internal transport or WASI artifact is produced.

---

## 6. Deferred Decisions

- Android/iOS lifecycle, surface recreation, IME, safe areas, accessibility, and storage paths require explicit product requirements and separate tasks.
- A custom rendering shell requires evidence that Iced primitives cannot meet a measured rendering requirement.
- Additional crates require demonstrated packaging, dependency-isolation, or reuse benefits.
- Durable event history, replay, synchronization, and audit storage are excluded until a concrete requirement exists.
- Mobile, multi-tab coordination, and live-service support must never be inferred from successful compilation alone.
