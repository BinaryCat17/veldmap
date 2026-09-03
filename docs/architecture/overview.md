# Overview

VeldMap is a desktop application for satellite imagery: it walks and searches
the catalogue of a provider (Copernicus Data Space), downloads products into a
local library and shows rasters — of downloaded scenes and of scenes that
lie in the remote storage, read in place. Terms are in
[the glossary](../glossary.md); how to build, run and check is
[build](../operations/build.md).

## Microkernel

The application is a native host with a WebAssembly runtime, and the
application logic is isolated wasm modules that talk only through the host's
event bus. The host owns what a module cannot: the window and the frame loop,
the registry of resources and their leases, the dispatcher with one actor per
subscriber, the accounting of operations in flight, the GPU. A module owns its
`State` and its handlers: everything it learns from the platform arrives as an
event, and everything it changes in the host goes through a synchronous ABI
call (`veldsdk::abi`, implemented in `veldcore/platform/host/core/src/abi.rs`).

Three services are native, inside the host: `app` — the window, input and
frame ticks, delegation of the window surface, "where is this widget" for a
scenario run; `fs` — read, write, list and delete under the runtime directory;
`network` — downloads, HTTP, opening a remote object as a byte resource. Their
contracts lie in `veldcore/interface/modules/`, their implementations in
`veldcore/platform/host/modules/`, and which of them a runner composes is the
`modules` list of its `runner.yaml`
(`veldcore/platform/host/runners/desktop/runner.yaml`). Everything else is a
wasm module under `veldmodules/`.

**The schema is the source of truth.** A topic exists only if a `schema.yaml`
declares it — a module's own in `veldmodules/<name>/schema.yaml`, a native
service's in `veldcore/interface/modules/<name>/<name>.schema.yaml`. The list
of topics lives only there and is not repeated in `docs/`. From the schemas
`buildgen/generate.py` builds the `generated/` crates — the dispatch, the
subscriptions, typed stubs for every declared output and call — and validates
the schemas first; a string topic in module code does not exist. The rules of
the bus are [bus-and-schema](bus-and-schema.md); the files of a module and
its wrap crate are [modules](modules.md).

## Repository map

```
veldcore/
  interface/              the platform's protocol: .proto and .schema.yaml
    core.proto              ResourceHandle, ResourceOpened, SurfaceDelegated, EventEnvelope
    graphics.proto          the arguments of the graphics ABI calls
    modules/<name>/         the contract of a native service: <name>.proto, <name>.schema.yaml
  sdk/rust/               veldsdk — the module's SDK: ABI, resources, graphics, replies,
                          snapshots, surface delegation, the fake host for native tests
  platform/host/
    core/                   the runtime: registry, memory, dispatcher, tasks, graphics,
                            ABI, plugin loading
    util/                   the API for authors of native modules
    generated/              bindings of the platform contracts (generated)
    modules/<name>/         a native service: config.yaml, src/, generated/
    runners/desktop/        the OS event loop, the window, the frame loop, scenario replay
    host.yaml               what the bindings are generated for
veldmodules/<name>/       a wasm module: schema.yaml, config.yaml, types.proto, src/,
                          wraps/rust/, generated/
buildgen/                 the build and the code generation
  build.py                  the one build command
  run-native.py             run
  run-uitests.py            replay the scenarios of uitests/
  generate.py               schemas → generated/ crates
  schema_deps.py            the build order from the dependencies in the schemas
  mutate.py, mutations/     the mutation check, run by hand
  templates/                jinja templates of the generated crates
  tests/                    buildgen tests (pytest), the first step of every build
uitests/                  scenarios for the runner
runtime/                  config/ (json per module), assets/ (fonts), data/ (downloads,
                          the tile cache), state/ (the window layout), logs/
build/plugins/            the built .wasm
workspace.yaml            modules_dir, sdk, plugins_dir
```

Every `generated/` directory is generator output and is never edited. The
Rust configuration — `veldcore/rust-toolchain.toml`, `.cargo/config.toml` —
lies inside the Rust parts, not at the root, and cargo is always invoked from
the manifest's directory ([build](../operations/build.md)).

## Services

Native, inside the host: `app`, `fs`, `network` (above). Wasm:

| Module | What it owns |
|---|---|
| `data-browser` | the screen: a tree of panes with tabs — catalogue, search, downloads, the globe, the layers on view, image previews; owns the window and its layout file; has no inputs of its own and gets its widget events addressed from `ui-service` |
| `ui-service` | lays out the layout it is sent (iced), shapes text (cosmic-text), draws into the delegated texture, returns widget events to the layout's owner |
| `globe` | the three-dimensional view: the WGS84 ellipsoid with a graticule, a camera as a frame above the surface, outlines and overlays of scenes as tiles; answers what place lies under a point of the frame |
| `image-view` | the preview canvas: a camera over one raster, tiles from the cache and the tiler, a frame into a delegated texture |
| `image-tiler` | a raster behind a byte resource → tiles of a pyramid: a fast describe and a long, cancellable produce; a memo of the parsed source between calls, no state an answer depends on |
| `tile-cache` | the tile cache on disk under `runtime/data/tiles/`: serves what it has, stores what is produced, evicts by budget; the only owner of that layout |
| `data-provider` | Copernicus: catalogue search and storage walk, signing addresses, opening a product as a resource without downloading, the rasters of a product and their roles, a product by storage key, the scene root of a key |
| `data-library` | the register of what is on disk: downloads and their state, which scene a file belongs to, the storage layout; the only module that knows that layout |

## Pages

- [bus-and-schema](bus-and-schema.md) — topics, correlation, one actor per subscriber, exchanges and the terminal reply, snapshot and targeted topics, what the validator holds.
- [modules](modules.md) — the files of a module, what the generator makes, the wrap crate and its two purposes, adding a module.
- [resources](resources.md) — `ResourceHandle`, byte and opaque resources, reader windows and the block pool, the lease, "open me this".
- [tasks](tasks.md) — killing an operation, what a trap costs, what the host does for a dead executor, who may be cancellable.
- [window-and-render](window-and-render.md) — the runner, the frame loop, surface delegation, `Viewport`, the layout cycle of `data-browser`.
- [screen](screen.md) — panes and tabs, what is saved, how a message finds its tab, one highlight and one menu.
- [imagery](imagery.md) — product, scene, part; the topics of one show; what is read per format; memory; fingerprint and cache.
- [viewing-pipeline](viewing-pipeline.md) — the consumer's side of the pyramid: the level table, the ladder, the store, passes and refusals.
- [georeference](georeference.md) — the four kinds of binding and their rank; what GeoTIFF and NetCDF carry, the lattice of tie points and its seating, the coordinate file beside a Sentinel-3 raster; projections and the datum; when a binding cannot be read.
- [globe](globe.md) — the geometry of the globe: the lift above the surface, the warp mesh, which cells are visible, the camera and its floor.
- [download](download.md) — the pipeline, `.part`, the sidecar, one file under one name.
- [invariants](invariants.md) — what must stay true across the tree and what holds it.
- [limitations](../limitations.md) — what the application does not do, with the cause.
- Operations: [build](../operations/build.md), [configuration](../operations/configuration.md), [scenario runs](../operations/ui-tests.md), [diagnostics](../operations/diagnostics.md); decisions in [docs/decisions/](../decisions/README.md).
