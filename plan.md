# plan.md — VeldMap Modular Monolith Migration Plan

## 0. Agent Execution Protocol

Each chat session executes one task from Section 5 and treats this file as the migration contract.

1. Locate the first unchecked task, or the task explicitly requested by the user.
2. Implement only that task. Do not jump ahead or silently shrink its verification.
3. Follow these rules in every task:
   - Write English only in source, comments, UI text, logs, documentation, and commit messages. Do not introduce Cyrillic characters (`[\u0400-\u04FF]`).
   - Use self-documenting names, small functions, strong domain types, and `thiserror` errors.
   - Do not restore `veldcore/platform/host`, `veldsdk`, `buildgen/`, `schema.yaml`, legacy host configs, `generated/` directories, or the `ui-service` process/ABI.
   - Use stable Rust 1.99.1, Rust 2024, workspace dependency inheritance, workspace lints, and modern module paths (`src/foo.rs` plus `src/foo/bar.rs`, not `src/foo/mod.rs`). Do not use a beta toolchain or the compiler's `future` edition.
   - Native delivery uses `apps/veldmap`; browser delivery uses the same application compiled to `wasm32-unknown-unknown`. Do not introduce WASI, Wasmtime, internal ConnectRPC, or internal protobuf transport.
   - Make logic easy to inspect: prefer pure functions, small modules, narrow public APIs, checked constructors, explicit state transitions, and diagnostic messages that identify the failed operation. Follow the test, comment, and documentation limits below.
4. A replacement may coexist with its predecessor only while being verified. After the replacement passes its complete behavioral and real runtime scenario, remove superseded source, dependencies, feature flags, compatibility shims, configuration, documentation, and unused lockfile entries in the same task. Do not retain a second implementation "just in case".
5. Run the complete verification for the current task, including its real runtime scenario. Compilation alone is not migration verification.
6. Only after verification passes, change that task's checkbox from `- [ ]` to `- [x]`. Do not add session notes or alter other task descriptions.
7. If an external platform assumption fails, record the evidence and revise this plan before building later tasks on that assumption. Pin a verified replacement rather than silently accepting a reduced platform or an unverified fallback.
8. Start with the four packages in Section 1.2. A new crate requires a demonstrated dependency-isolation, packaging, or independent-reuse need; a domain name alone is not sufficient justification.

### Clean Artifact Policy

**Tests are allowed only for a named user scenario or a reproduced historical bug:**

1. Native/browser startup smoke scenarios with positive readiness, expected initial state, clean shutdown, and complete-log checks.
2. Critical end-to-end flows using the production composition and deterministic external fixtures, with expected application outcomes and clean logs. Do not mock application internals.
3. Regression scenarios for bugs actually observed. Reproduce the bug first and demonstrate that the check fails before the fix and passes afterward. Prefer an application-level scenario; a focused lower-level regression check is allowed only when the external flow cannot reproduce it reliably.

Every test must identify the scenario or bug it protects and have a plausible failure mode; otherwise delete it or do not write it. Use one shared runtime runner and fixture set. No standing mathematical/application-logic suites, getter or trivial-wiring tests, implementation mirroring, dependency-behavior tests, duplicate coverage, coverage targets, or test-only architectural abstractions.

**Comments default to zero and must never exceed 5% of lines in a Rust file.** Allow only unsafe invariants inexpressible in types; non-obvious OS/browser/FFI/protocol behavior; coordinate, unit, or mathematical source citations; and non-obvious crash-recovery or ownership invariants. Do not narrate code, restate names/signatures, add section banners, retain commented-out code, or write changelogs, task notes, or AI explanations. Replace a comment with a better name or stronger type whenever possible.

**Only these permanent documentation files are allowed:**

| File | Maximum | Sole responsibility |
|---|---:|---|
| `README.md` | 35 lines | Product orientation, architecture map, essential build/run commands |
| `docs/operations.md` | 80 lines | Exact tool pins, setup, launch, build, dependency, and log commands |
| `docs/architecture.md` | 80 lines | Final pipeline, crate boundaries, domain contracts, ownership, and recovery |
| `docs/verification.md` | 80 lines | Support matrix, runtime scenarios, expected outcomes, and measured budgets |

Keep each fact in one authoritative location and link instead of repeating it. Do not create task reports, session summaries, review reports, implementation diaries, or other prose artifacts. Reaching a limit requires removing duplication or obsolete material, not adding another file. Update or remove stale documentation in the same task as the behavior change. Do not create additional documentation files without an explicit user request.

`docs/verification.md` is created in Task 2 when real scenarios and support data exist; `docs/architecture.md` is created in Task 14 from the delivered implementation. `plan.md` is the temporary migration contract, not permanent history. Its candidate/target details are replaced by the permanent documents at completion rather than copied into them.

---

## 1. Target Architecture

### 1.1 Product Shape and Organizing Idea

VeldMap is one trusted, modular application with ordinary typed Rust calls between modules. There is no internal transport, service orchestration, component sandbox, or requirement to preserve protobuf contracts for internal calls.

The organizing pipeline is: discover a remote scene, identify an exact source version, derive bounded raster tiles, cache them, and share them between 2D and globe views. Persistent current state is authoritative; collection snapshots represent that state, while progress and invalidations are ephemeral. Crates, workers, and platform adapters exist to support this pipeline, not as product boundaries of their own.

The native application is the first complete product. The browser application reuses domain, rendering, and storage logic through thin platform adapters. Android and iOS are deferred until explicitly required; winit compatibility alone is not evidence of mobile readiness.

External CDSE/S3 HTTP remains a network boundary. Browser use does not imply a backend proxy, but a proxy may be reconsidered if real CORS, credential, or signing requirements cannot be satisfied safely. A browser-only service limitation may reduce the supported browser release scope without blocking an otherwise verified native release.

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

This tree is schematic. Use nested modules for substantial areas, keep public exports narrow, and avoid recreating large legacy dispatcher files.

`veldmodules/` is retained temporarily as migration input, outside the Cargo workspace and final architecture. The former protobuf, host, RPC, Buf, and WASI scaffolding was removed in Task 1.

Dependency direction:

```text
apps/veldmap → veldmap-core
apps/veldmap → veldmap-render  → veldmap-core
apps/veldmap → veldmap-storage → veldmap-core
```

- `veldmap-core` owns domain types, application behavior, consumer-defined ports, pure raster transformations, tile planning, and persistent-cache policy. Provider, library, tiler, cache, viewer state, and globe state are modules, not separate crates.
- `veldmap-render` owns wgpu pipelines, GPU resources, the bounded GPU tile cache, and viewer/globe draw behavior. Camera math and tile selection remain in core.
- `veldmap-storage` owns SQL migrations, synchronous repositories, the database owner, and native/browser SQLite drivers.
- `apps/veldmap` owns eframe presentation, repaint scheduling, native/web HTTP and blob adapters, native codec FFI, browser codec workers, runtime selection, and composition. Only the composition root knows every concrete implementation.
- Core does not depend on eframe, wgpu, winit, rusqlite, Tokio, wasm-bindgen, browser JavaScript APIs, or native codec FFI. Once FFI is outside core, forbid unsafe code in core.

Do not create separate `ports`, `app`, `ui`, `platform-native`, or `platform-web` crates preemptively. Extract an imagery/codec crate when native/browser dependency sets, unsafe isolation, packaging, or compile boundaries justify it. Portable pure-Rust codecs may remain in core; platform-specific implementations belong at the platform edge.

### 1.3 In-Process APIs and Ports

Application modules expose typed operations such as `search`, `place_downloads`, `open_raster`, `produce_tiles`, and `control_camera`. They do not expose RPC DTOs, worker messages, SQL, connection types, or transport errors.

Use ports only at real platform boundaries:

- HTTP responses and streaming Range bodies.
- Blob storage and local files.
- Clocks and timers.
- Background execution and browser workers.
- Native/browser decoder implementations where their APIs or runtimes differ.
- SQLite drivers where their APIs genuinely differ.

Define each port beside its consumer. Do not wrap every domain object in a trait and do not create a universal platform abstraction.

Native scheduling may use Tokio. Browser scheduling uses `wasm-bindgen-futures` and workers. Domain code is runtime-independent, and CPU-heavy decoding never blocks the UI executor.

### 1.4 Source Identity and Tile Contracts

- `SourceId` identifies the logical source. `SourceVersion` identifies exact content through a verified immutable remote version or a collision-resistant content identity for mutable/local content. Signed URLs and expiring credentials are not identities.
- Select an established content hash such as BLAKE3 or SHA-256 when content hashing is required. Detect same-length mid-file local edits; do not extend the legacy FNV/head/tail fingerprint.
- `TileKey` includes `SourceVersion`, variable, decoder/render recipe, and pyramid address. A cache hit must identify the same complete recipe, not merely the same file name.
- `DecodedTile` contains validated dimensions, row stride, explicit pixel format and color semantics, and `Bytes` pixels. QOI persists render-ready RGBA tiles only, never scientific raster values.
- Externally derived file and blob names use a validated type that confines access to the intended storage root and rejects traversal or ambiguous path components.

### 1.5 State, Snapshots, and Cancellation

- Collection subscriptions publish complete current snapshots, not durable mutation history. Stable IDs preserve selection, expansion, and scroll state.
- Notifications are ephemeral invalidations, not authoritative data. Reconnection obtains a fresh snapshot.
- Subscribe before constructing the initial snapshot, invalidate after successful mutations, and coalesce bursts without losing the latest state.
- Download progress is high-frequency ephemeral state. Persist lifecycle transitions and throttled resume checkpoints, not every network chunk.
- Every spawned operation has an owner. Dropping that owner signals cancellation; dropping a Tokio `JoinHandle` alone is not cancellation. Blocking native work checks cancellation cooperatively, and browser workers have explicit cancellation/termination behavior.
- Define `Send`/`!Send` and `'static` requirements at each port rather than imposing native threading bounds on browser adapters.
- Bounded queues coalesce replaceable progress and snapshots. Reliable mutations apply backpressure or return an explicit overload error; they are never silently dropped.
- Camera input is coalesced before changing application state or starting work. Resize collapses to the latest dimensions.

No event log, replay cursor, event-outbox framework, or generic observable-store framework is introduced without a new synchronization requirement.

---

## 2. Dependency Baseline

### 2.1 Toolchain and Reproducibility

Use Rust `1.99.1`, edition 2024, and `rust-version = "1.99"`. Commit `Cargo.lock` and use `--locked` for verification. Exact direct-dependency pins are allowed for this application, but they do not establish compatibility by themselves. Update direct pins and the lockfile deliberately, review the resulting dependency changes, and rerun the runtime gates.

The starting candidates are:

| Component | Initial pin or policy |
|---|---|
| eframe | `=0.37.0` |
| egui | `=0.37.0`, direct only where its types are needed |
| egui-wgpu | `=0.37.0` |
| wgpu | `=30.0.1`, unified with eframe; advance only with verified eframe/wgpu compatibility |
| winit | No independent crates.io pin; use eframe's integration or its exact resolved winit version if direct types are necessary |
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

eframe 0.37 uses wgpu 30 and crates.io winit. Do not mix an independently pinned winit package into eframe window/event types.

The former Iced candidate was rejected after its unmodified counter example panicked in `iced_wgpu::quad::Layer::update` during browser WebGPU staging-buffer mapping. An isolated eframe 0.37/wgpu 30 spike rendered custom paint content natively and in Windows Chrome/WebGPU; complete Task 2 verification is still required.

Target-specific dependencies belong in the relevant app, render, storage, or justified codec crate. Do not put native and browser driver dependencies indiscriminately in the root manifest.

Task 2 establishes CI for formatting, workspace integration tests, native/WASM builds and clippy, and dependency checks. Use `cargo deny` or an equivalent for advisories, licenses, duplicate sources, and allowed Git sources. CI does not replace real native-window and browser scenarios.

### 2.2 HTTP and Decoder Selection

Task 3 selects and exactly pins the native HTTP implementation after proving streaming Range bodies, timeouts, cancellation, redirect control, and TLS. Prefer a high-level client such as reqwest if it passes; do not assume its browser implementation satisfies VeldMap's streaming requirements.

The browser HTTP adapter uses the Fetch API through project-owned code. Native and browser adapters implement the same consumer-owned HTTP port, but need not share a client crate.

OpenJPEG retains its native implementation initially. Its browser Emscripten build, worker protocol, cancellation, and representative Sentinel decoding are a separate gate. Do not assume native and browser codec builds produce equivalent displayed output; open the same representative fixture through both application flows and record any product-relevant differences.

### 2.3 Exclusions

- No WASI target, Wasmtime host, ConnectRPC, Buffa, or internal protobuf.
- No ORM or generic database framework.
- No PGlite or parallel browser storage architecture.
- Use exactly one UI toolkit, egui/eframe; do not reintroduce Iced or add another toolkit.
- No custom rendering shell while eframe's `egui_wgpu::CallbackTrait` integration satisfies the requirements.

---

## 3. Rendering, Storage, and Data Ownership

### 3.1 eframe and Shared GPU Resources

eframe owns the application window/canvas and its wgpu device, queue, encoder, and submission. Viewer and globe integrate through egui paint callbacks and `egui_wgpu::CallbackTrait`. Their pipelines receive eframe's device and queue and share GPU textures through application-owned callback resources.

Do not create an independent `RenderContext` in `App::new`, a second surface, or a parallel command-submission shell unless a measured limitation is demonstrated and this plan is revised.

- `DecodedTile` belongs to core and follows the complete contract in Section 1.4.
- `GpuTile` belongs to render and is shared as `Arc<GpuTile>` between viewer and globe.
- GPU upload consumes or borrows the decoded payload once and does not retain a duplicate CPU copy without a documented reason.
- The persistent QOI cache and the GPU tile cache are distinct types with separate byte budgets.
- No RGBA frame streams, production GPU readbacks, or per-frame CPU pixel transport are permitted. Diagnostic readbacks are allowed only in explicit screenshot/debug operations.
- QOI is for persistent-cache serialization of render-ready RGBA tiles, not draw calls, scientific values, or inter-module frame transport.
- GPU uploads still copy. "Zero avoidable copies" means eliminating application-level duplication, not claiming impossible end-to-end zero-copy.

### 3.2 SQLite Scope and Schema Style

SQLite stores current-state metadata, not entity history. Initial candidates are `downloads`, `sources` where query requirements justify them, and `settings`. A `tile_entries` index is added only after measuring a need; otherwise the existing file-based cache remains authoritative.

Keep downloaded imagery, `.part` files, and QOI payloads in file/blob storage. SQLite stores metadata and references, not large raster blobs.

Use small numbered SQL migrations with explicit foreign keys, checks, and purposeful indexes. Each migration runs in a transaction and is exercised through application upgrade scenarios from every released schema version. Prefer ordinary tables and repository SQL over trigger-based APIs, soft-delete lifecycles, replay cursors, and append-only state snapshots.

- Native has one database owner thread; browser has one logical database owner worker according to the selected multi-tab policy.
- Repository implementations are synchronous inside the owner. Public application operations may be asynchronous channels or messages.
- Every repository operation is transaction-complete. Never hold a transaction across `.await`, a worker message, file/network I/O, or a render operation.
- SQLite integers are signed 64-bit values. IDs and unsigned byte counters require checked conversion and an explicit upper bound.
- Set connection pragmas on every connection and verify required SQLite features plus `sqlite_version()` through runtime diagnostics on each driver.
- Keep a private driver-compatibility trait only if real native/browser differences require one. Do not build a public generic database abstraction.

### 3.3 Files, Downloads, and Cache Recovery

Downloads retain `.part` files, Range resume validation, signature refresh, pause/remove behavior, and atomic completion renames where supported. Where atomic rename is unavailable, use an explicit crash-safe completion protocol and reconcile its state at startup. Database metadata is reconciled with actual files at startup and after interrupted writes.

When SQLite replaces `.origin` sidecars, import existing metadata without deleting user imagery, verify the imported state through the application, and remove the old writer and duplicate authority in the same migration task. Do not maintain indefinite dual writes.

Persistent tile writes use temporary files and atomic rename where the platform supports it. Exercise corrupt entries, interrupted writes, active sources, eviction, and cache-budget headroom through application runtime scenarios. Database and filesystem state must not be assumed to commit atomically together.

### 3.4 Browser Persistence and Multiple Tabs

A dedicated worker in every tab is not one origin-wide owner. Task 4 must choose and verify one policy:

- Explicit single-tab behavior with a clear second-tab response.
- A shared owner or coordinated ownership mechanism.
- A VFS that supports the required multi-tab behavior directly.

`sahpool` is acceptable only if its concurrency model matches that policy. Do not assume WAL provides OPFS concurrency. Exercise migrations, short transactions, reload persistence, quota/permission failures, blob references, busy handling, and the selected second-tab behavior through real browser application scenarios.

Initialization failure must not wipe existing metadata or imagery. Verify the selected wrapper's non-destructive failure behavior explicitly. Exercise origin-storage eviction recovery and persistent-storage requests where supported, and make clear when browser data is best-effort. Browser storage must not silently become the only copy of irreplaceable user imagery.

---

## 4. Cross-Cutting Verification

Every real runtime scenario requires a positive readiness or expected application outcome and clean logs. Scan complete native/browser logs for panics, runtime errors, unhandled promise rejections, GPU validation failures, and unexpected SQLite/codec errors. Do not suppress diagnostics or treat absence of errors alone as a successful scenario.

The following are architecture gates, not optional cleanup:

1. Tasks 2–4 record the supported native OS/GPU and browser matrix, native/browser format scope, WebGPU or WebGL policy, representative raster/download sizes, CPU/GPU and queue budgets, cancellation deadlines, and UI-responsiveness thresholds. Values come from measured scenarios rather than arbitrary defaults.
2. One eframe-owned wgpu device/queue serves UI, viewer, and globe without per-frame CPU pixel transport.
3. Real CDSE/S3 requests pass native and browser signing, CORS where applicable, redirect, cancellation, and streaming Range scenarios.
4. Representative Sentinel JP2 imagery decodes and displays in a browser worker with bounded memory and responsive cancellation.
5. Native and browser application scenarios cover repository startup, migrations, restart persistence, corruption handling, and the chosen multi-tab policy.
6. CPU and GPU caches remain within their measured aggregate budgets; skipped frames/snapshots cannot grow queues or retain unbounded textures.
7. The complete native user flow and the supported browser user flow run against deterministic fixtures, with separately identified live-CDSE smoke verification.
8. No production path depends on GPU readback, internal RPC, WASI, `veldsdk`, or legacy generated contracts.

---

## 5. Sequential Work Plan

### Task 1: Establish the Stable Modular Workspace

- [x] **Action:**
  1. Pin stable Rust `1.99.1`, add `wasm32-unknown-unknown`, and create `apps/veldmap`, `crates/veldmap-core`, `crates/veldmap-render`, and `crates/veldmap-storage` with the dependency direction in Section 1.2.
  2. Remove `proto/`, `host/`, `crates/veldmap-proto`, `crates/veldmap-rpc`, Buf/WASI configuration and scripts, the legacy host policy, and their dependencies. Retain `veldmodules/` outside the workspace as migration input.
  3. Establish shared lints, exact candidate pins, lockfile verification, and minimal native/browser entry points. Do not migrate domain modules or create additional crates.
  4. Update README and operations documentation to describe the transitional architecture and actual commands.
- **Verification:** `cargo fmt --all -- --check`, workspace tests, native clippy with `-D warnings`, and native plus `wasm32-unknown-unknown` builds of the new packages succeed with `--locked`; legacy-reference scans confirm that only `veldmodules/` and migration documentation mention superseded transport architecture.

### Task 2: Prove Shared eframe/wgpu Rendering on Native and Browser

- [ ] **Action:**
  1. Pin eframe/egui/egui-wgpu `=0.37.0` and unified wgpu `=30.0.1`, using eframe's winit integration. Record exact versions and `cargo tree` evidence; later upgrades remain deliberate and require the same runtime gates.
  2. Build a minimal eframe application with `egui_wgpu::CallbackTrait` paint callbacks, two viewport types, and one shared `Arc<GpuTile>`. Upload one deterministic RGBA fixture once through eframe's device/queue, retain no duplicate CPU pixel payload, and do not create an external rendering shell.
  3. Render that tile in viewer-like and globe-like geometry while ordinary egui widgets remain interactive. Exercise input, clipping, resize, scale factor, and the surface reconfiguration/recreation paths exposed on each target, then build the same path for a browser canvas. Only browsers exercised with WebGPU enter the initial support matrix; do not describe resize testing as device-loss recovery.
  4. Establish one reusable application smoke runner with explicit readiness, expected-outcome, and complete-log checks on native and browser. Use tile identity/upload counters and diagnostic screenshots to prove resource sharing and visible output; keep readback out of normal rendering. Prove that idle draw counters remain unchanged without input after initial readiness. Add CI and dependency checks from Section 2.1.
  5. Create `docs/verification.md` within its 80-line limit. Record the exercised native/browser rendering matrix, smoke commands, expected outcomes, and measured tile-upload, non-pixel CPU-traffic, application-owned tile-memory, one-entry cache, responsiveness, shutdown, and idle-repaint bounds. Identify these as one-tile spike measurements rather than total GPU-memory or future production-cache budgets.
- **Verification:** real native-window and each declared browser application start, render both geometries, accept widget and pointer input, clip, resize, observe scale factor where supported, share one uploaded texture, remain within the recorded spike bounds, perform no periodic redraw while idle, and shut down with clean logs; native/wasm builds and clippy pass; no per-frame CPU pixel transport or second GPU device exists. Failure blocks further rendering migration and requires plan revision.

### Task 3: Prove External HTTP and Browser OpenJPEG

- [ ] **Action:**
  1. Implement the minimal consumer-owned streaming HTTP port and select/pin native and browser adapters. Prove bounded Range reads, cooperative cancellation, timeouts, redirect rejection for signed requests, and no credential forwarding across origins.
  2. Exercise deterministic local CDSE/S3 fixtures through the application composition, including expired-signature refresh and continuation. Run separate live CDSE/S3 checks for native and browser CORS/signing without recording secrets.
  3. Build the browser OpenJPEG worker spike and decode representative Sentinel JP2 inputs. Define bounded worker messages, ownership transfer, cancellation/termination, error reporting, and feature/version diagnostics.
  4. Record exact selected dependencies, browser headers, codec revisions, representative input sizes, memory/cancellation thresholds, and reproducible commands. Remove superseded spike implementations.
- **Verification:** native and browser application HTTP scenarios plus live smoke checks complete with expected outcomes and clean logs; representative browser decoding displays the fixture, stays within its measured memory/cancellation budgets, and leaves the UI responsive; no UI-thread decode or unbounded response buffering occurs. Failure requires revising the affected adapter before full provider/tiler migration.

### Task 4: Establish SQLite Ownership and Browser Persistence Policy

- [ ] **Action:**
  1. Create migration infrastructure and the native database owner in `veldmap-storage`. Use only a minimal provisional current-state repository to exercise the drivers; the download domain schema and production repository are finalized in Task 8.
  2. Implement the browser owner and driver behind the same provisional operations. Select the VFS and explicit single-tab, shared-owner, or multi-tab policy from Section 3.4.
  3. Add only driver-compatibility abstraction demonstrated by the two implementations. Verify connection pragmas, migration transactions, rollback, restart persistence, busy/error handling, and `sqlite_version()`/required features through application runtime diagnostics.
  4. Exercise metadata references to external blobs, interrupted/reconciled writes, non-destructive initialization failure, quota/eviction recovery, and crash-safe completion. Do not put raster payloads or an event log in SQLite.
  5. Record representative storage sizes, transaction-duration limits, second-tab behavior, and recovery expectations in the support matrix.
- **Verification:** real native/browser application scenarios cover startup, empty/provisional migrations, rollback, reload, interrupted writes, non-destructive recovery, and the selected second-tab policy with expected state and clean logs; native/wasm clippy and builds pass; no transaction crosses asynchronous or external I/O boundaries.

### Task 5: Migrate Pure Domain and Application Foundations

- [ ] **Action:**
  1. Migrate pyramid/tile math, geodesy/projection, camera state, range readers, source identity, byte budgets, configuration expansion, and frame/pixel validation from legacy modules into `veldmap-core` using small, inspectable pure functions.
  2. Implement the `SourceId`, `SourceVersion`, `TileKey`, `DecodedTile`, and validated storage-path contracts from Section 1.4. Replace `veldsdk` resources and protobuf values with owned domain types, `Bytes`, typed errors, and consumer-owned ports.
  3. Select and pin an established content hash. Detect same-length mid-file local edits and preserve verified immutable remote version identities where hashing is unnecessary.
  4. Implement bounded range-block pooling/readahead and explicit operation-owner cancellation without tying domain code to Tokio or browser APIs. Forbid unsafe code and platform-specific dependencies in core.
  5. Collect representative legacy configurations and deterministic fixtures for subsequent application runtime scenarios; do not build parallel mathematical or application-logic test harnesses.
- **Verification:** native application startup and real local/HTTP range and source-change scenarios complete with expected application outcomes and clean logs; native/wasm clippy and builds pass; dependency scans confirm that new core has no eframe/wgpu/rusqlite/Tokio/wasm-bindgen/native-FFI dependency and no unsafe code.

### Task 6: Build the Thin Native Raster Vertical Slice

- [ ] **Action:**
  1. Implement only the portable TIFF or JP2 path needed to open one representative local fixture. Reuse the Task 3 decoder work and keep native codec FFI at the platform edge.
  2. Produce validated `DecodedTile` values and display them through a minimal viewer using Task 2's eframe integration and production composition root. Implement only the camera control needed to inspect the fixture.
  3. Connect source identity, bounded tile production, upload, display, resize, cancellation, and clean shutdown without adding the full provider, persistent cache, or remaining codecs.
  4. Retain the fixture and application smoke scenario for later migration tasks. Do not create an alternative demo architecture that will be discarded.
- **Verification:** the real native application starts, opens and displays the representative raster, supports the included navigation/resize flow, cancels an in-flight open, and closes with the expected outcome and clean logs; native/wasm builds and clippy pass.

### Task 7: Migrate Provider as Typed In-Process Logic

- [ ] **Action:**
  1. Migrate OData search, fresh-window widening, scene/part folding, S3 listing and SigV4 signing, manifest parsing, imagery selection, and MGRS/UTM behavior into `core::provider`.
  2. Connect the Task 3 HTTP port directly through typed operations. Preserve bounded internal continuation and terminal truncation information without exposing transport DTOs.
  3. Preserve credential redaction, expiry refresh, redirect policy, and deterministic errors. Remove the legacy Provider RPC boundary after equivalent application scenarios pass.
- **Verification:** native and supported-browser application scenarios exercise search, empty/complete/truncated listings, scene folding, manifests, signing, and error presentation against deterministic fixtures with expected screen states and clean logs; live CDSE smoke remains separate and secret-free.

### Task 8: Migrate Downloads and Library Persistence

- [ ] **Action:**
  1. Implement library queueing, concurrent-download limits, Range resume, pause/remove, scene counts, signature refresh, and crash-safe completion through typed provider/HTTP/blob operations.
  2. Finalize the download domain model, SQL schema, and production repository using Task 4's verified ownership policy. Import existing `.origin` metadata and actual file state without deleting user imagery, reconcile interrupted downloads, and remove the superseded sidecar writer after verification.
  3. Publish complete download snapshots with stable IDs, post-mutation invalidation, race-free initial subscription, coalesced ephemeral progress, and owner-driven cancellation.
  4. Persist lifecycle transitions and bounded resume checkpoints rather than per-chunk progress. Keep `.part` payloads outside SQLite.
- **Verification:** real application scenarios cover interrupted ranges, expiry/re-signing, resume validation, pause/cancel/remove, startup reconciliation, legacy-sidecar import, initial/reconnect snapshots, invalidation during delivery, per-item failures, and native download/restart with expected visible/persisted outcomes and clean logs; no dual metadata authority remains.

### Task 9: Complete Core Tiling and the Persistent Tile Cache

- [ ] **Action:**
  1. Complete TIFF, JP2/OpenJP2, excerpt, common codec, radiometry, resampling, and budget behavior. Keep portable transformations and tile planning in core, with native FFI and browser workers at the platform edge.
  2. Produce validated `DecodedTile` values and stream progress/item errors through typed operations. Verify `SourceVersion` before work and stop within the recorded cancellation deadline.
  3. Migrate QOI file persistence, atomic/crash-safe recovery, source protection, and bounded LRU eviction. Keep the persistent cache distinct from the GPU cache; add a SQLite tile index only if measurements justify it.
  4. Connect native OpenJPEG and the verified browser decoder worker without duplicating domain orchestration.
- **Verification:** applications open and display representative TIFF/JP2 fixtures on supported targets; runtime scenarios exercise HTTP Range reads, cancellation, corrupt/interrupted QOI recovery, eviction, budget limits, and stale-cache rejection after same-length mutation with expected outcomes and clean logs; aggregate CPU memory remains within its recorded budget.

### Task 10: Migrate the Remaining Raster Adapters

- [ ] **Action:**
  1. Migrate JPEG, PNG, NetCDF/HDF5, grid, table, format-specific radiometry, resampling, and geolocation behavior in small independently verified groups.
  2. Keep portable adapters in core and move native/browser-specific dependencies to the platform edge or a justified codec crate. Reuse the established `DecodedTile` and background-cancellation contracts.
  3. Add only the variable/geolocation selection needed by the runtime scenarios. Update the native/browser format support matrix and document every intentional limitation.
- **Verification:** the real applications start and open a representative fixture through every supported adapter with the expected visible result and clean logs on each declared target; unsupported format/target combinations produce an explicit application error rather than a panic or silent fallback; builds and clippy pass.

### Task 11: Complete the Native 2D Workflow

- [ ] **Action:**
  1. Complete viewer camera control, fit/zoom/pan, resize, variable selection, tile ladder, and typed tile production in core.
  2. Complete the viewer pipeline and bounded `Arc<GpuTile>` cache in render using Task 2's `egui_wgpu` paint-callback integration. Share device/queue with the UI and upload each decoded tile without a duplicate retained pixel payload.
  3. Build the complete eframe catalogue/search/download/open workflow and download subscriptions. Coalesce pointer input and preserve view state across snapshot replacement.
  4. Remove superseded image-view and tile-cache RPC paths after equivalent application scenarios pass.
- **Verification:** the complete native application flow searches a fixture catalogue, downloads/opens a raster, changes variables, navigates/resizes, cancels work, and survives restart with expected visible/persisted outcomes and clean logs; profiling shows no per-frame CPU pixel transport and CPU/GPU memory remains within budget.

### Task 12: Complete the Supported Browser Application

- [ ] **Action:**
  1. Compose provider, library, tiler, viewer, storage, and rendering with browser Fetch, OPFS/blob, database-owner, and decoder-worker adapters.
  2. Implement browser-specific path, quota, permission, reload, eviction recovery, cancellation, and second-tab behavior without leaking platform types into core.
  3. Reuse the native eframe workflow and verify responsive UI during network, database, and decode work. Keep worker payloads bounded and ownership transfers explicit.
  4. Update `docs/operations.md` with required headers, worker startup, and log commands. Update `docs/verification.md` with browser/format support, persistence/eviction behavior, live-CDSE limitations, expected outcomes, and measured budgets.
- **Verification:** the complete supported browser flow passes against deterministic fixtures in each supported browser, including reload persistence, non-destructive recovery, and the chosen multi-tab scenario, with expected outcomes and clean logs; live CDSE/S3 CORS smoke passes or the browser scope/plan is revised; no UI-thread blocking or per-frame CPU pixel transport occurs.

### Task 13: Migrate Globe and Shared Overlays

- [ ] **Action:**
  1. Migrate WGS84 geometry, surface lifting, warp subdivision, culling, georeference ranks, Transverse Mercator/Pulkovo math, outlines/hatching, overlays, and quaternion flights into small inspectable core modules.
  2. Implement globe rendering through `egui_wgpu` paint callbacks with the same GPU resources and `Arc<GpuTile>` values used by the viewer. Publish overlay snapshots rather than frames or event history.
  3. Implement overlay ordering, opacity, visibility, variable selection, surface probing, camera flights, and coalesced orbit/zoom/resize input in the shared UI.
  4. Remove legacy globe RPC/GPU resource code after native and browser application parity scenarios pass.
- **Verification:** real native/browser globe scenarios exercise overlay snapshots/reconnects, camera control, probing, resize, cancellation, shared-tile reuse, and bounded memory with expected visible outcomes and clean logs; no RGBA frame stream or production readback exists.

### Task 14: Complete Presentation and Remove Superseded Architecture

- [ ] **Action:**
  1. Complete panes/tabs, layout persistence, catalogue/search/download tables, row states, batch actions, NetCDF selectors, settings, and viewer/globe interactions using eframe and current-state storage.
  2. Preserve English plural/decimal formatting, accessible error presentation, stable selection across snapshots, bounded input coalescing, and clean cancellation/shutdown. Do not claim OS accessibility or mobile support without separate verification.
  3. Remove `veldmodules/` and any remaining migration-only references or unused lockfile entries after all replacement application scenarios pass.
  4. Create `docs/architecture.md` from the delivered implementation, not by copying this plan. Update `README.md`, `docs/operations.md`, and `docs/verification.md` to their assigned responsibilities and line limits; remove duplicated or historical material.
  5. After all verification passes and this task is checked, delete this temporary migration plan rather than archiving it or copying it into permanent documentation.
- **Verification:** `cargo fmt --all -- --check`, `cargo test --workspace --locked`, native and `wasm32-unknown-unknown` clippy with `-D warnings`, dependency checks, release native/browser builds, complete native and supported-browser user flows, restart/migration scenarios, memory measurements, complete-log scans, documentation ownership/line-limit scans, and legacy-reference scans all pass. Exactly the modular application architecture remains; no internal transport or WASI artifact is produced.

---

## 6. Deferred Decisions

- Android/iOS lifecycle, surface recreation, IME, safe areas, accessibility, and storage paths require explicit product requirements and separate tasks.
- A custom rendering shell requires evidence that `egui_wgpu` paint callbacks cannot meet a measured rendering requirement.
- Additional crates require demonstrated packaging, dependency-isolation, or reuse benefits.
- Durable event history, replay, synchronization, and audit storage are excluded until a concrete requirement exists.
- Mobile, multi-tab coordination, and live-service support must never be inferred from successful compilation alone.
