# A module and its wrap crate

What lies in `veldmodules/<name>/`, what `buildgen/generate.py` makes of it,
how a module reaches a foreign schema, what the wrap crate is for, and how a
module is added. The rules of the bus — topics, pairs, snapshots, what the
validator refuses — are on [the bus page](bus-and-schema.md); this page is
about the files.

## The directory

| File | Written by | What it is |
|---|---|---|
| `schema.yaml` | the author | the contract: `name` (equal to the directory name), `interface.inputs` and `interface.outputs` with their payload types, `dependencies` with the foreign topics it subscribes to (`subs`) and publishes into (`calls`), `hooks` |
| `config.yaml` | the author | the Cargo side: `package`, `language`, `rust.dependencies`, and `rust.wrap_dependencies` for the files shared with the wrap crate |
| `types.proto` | the author | the module's own messages, package `veldmap.<name>`, named `module/<Message>` in the schema; optional — `data-browser` has none |
| `src/module.rs` | the author | `Config`, `State`, `hook_init`, one handler per topic, `hook_event`; the other files of `src/` are its submodules |
| `wraps/rust/src/wrap.rs` | the author | the handwritten part of the wrap crate; optional |
| `generated/` | the generator | the module crate — `src/lib.rs`, `Cargo.toml`, `build.rs`, `rust-toolchain.toml`, `.cargo/config.toml` — and `wraps/rust/`, the wrap crate, when there is a `types.proto`; never edited, and a file is rewritten only when its content changed, because cargo compares mtimes |
| `runtime/config/<name>.json` | the author | the module's config, handed to `init` whole |

A directory is a module when it holds both `schema.yaml` and `config.yaml`
(`discover_modules` in `buildgen/build.py`); there is no list of modules
anywhere else. `package` names the crate and the artefact: `veldmap-globe`
becomes `build/plugins/veldmap_globe.wasm`. The name on the bus is `name` from
the schema, compiled in as `SERVICE_NAME` and returned by the export
`get_service_name`; the host reads it from there — not from the file name —
and looks up the config as `<name>.json` by it. The validator refuses a `name`
different from the directory: a consumer addresses `<directory>/<topic>`, the
producer publishes `<name>/<topic>`, and a mismatch is a lost event, not an
error.

## What the generator makes

`generated/src/lib.rs` (`buildgen/templates/lib.rs.j2`) is the glue between
the host and `src/module.rs`, which it includes through `#[path]`:

- `proto` — the module's own messages under `proto::<alias>`, the messages of
  every dependency under `proto::<dep alias>` re-exported from its wrap crate,
  and `proto::core`, `proto::app` from the SDK. The alias is the last segment
  of the proto `package`: `veldmap.image_tiler` → `image_tiler`.
- The exports the host calls: `init` decodes the config JSON into `Config`,
  calls `hook_init` and keeps its `Result<State>` (an empty config is refused,
  an unparsable one too, and a failed `hook_init` is reported by the first
  event); `handle_event` decodes the `EventEnvelope`, sets the event context —
  publisher, correlation, whether the reply is intermediate — and dispatches
  on `<service>/<topic>` to the handler, catching a panic into the log, since
  the bus expects no answer; `get_subscriptions` lists the inputs, every
  `subs`, and `app/on_ready` when `hook_event` is on; `get_service_name`.
- `emit::<output>(&Message[, correlation_id][, target])` for every
  `interface.outputs`: the correlation argument exists only on a topic with
  `replies_to`, the target only on a `targeted` one; the stub of a `snapshot`
  topic remembers the fingerprint of the last body and skips a repeat, and
  `emit::resend::<output>()` forgets it.
- `calls::<dep>::<input>(&Message[, correlation_id])` for every
  `dependencies.<dep>.calls`, by the same rules.
- `cancel::<dep>::<input>(&correlation_id)` for the calls whose producer
  declared the input `cancellable`.
- `flow::is_intermediate` — the intermediate replies among the subscriptions,
  read from the producers' schemas, by which the SDK warns a requester that
  settles on progress.

The handler of a topic is `crate::module::<topic>`, `fn(&mut State, Message)`,
the message being the type the producer's schema declares — the module's own
for `interface.inputs`, the dependency's for `dependencies.<dep>.subs`; a
missing handler or another type is a compile error. `hook_event(&State)` runs
after every handled event and once on `app/on_ready` when `hooks` lists it —
`data-browser` rebuilds its layout there.

`generated/Cargo.toml` is a standalone workspace with a `cdylib`, `veldsdk` by
path, `rust.dependencies` from `config.yaml`, and a path dependency
`<package>-wrap` on the wrap crate of every dependency that has a
`types.proto`. `build.rs` compiles `types.proto` with prost, with the
repository root and `veldcore/interface/` as import roots.

The build (`buildgen/build.py`) runs the generator for every module in
dependency order, builds the module crate for `wasm32-wasip1` from inside
`generated/`, checks the wrap crate for the same target, and copies the
`.wasm` into `build/plugins/`; the host, then the unit tests, follow
([build](../operations/build.md)).

## A foreign schema

A module names another service only in `dependencies`. `subs` are the
producer's outputs it wants delivered — each becomes a subscription and needs
a handler of that name; `calls` are the producer's inputs it publishes into —
each becomes a `calls::<dep>::` stub. The producer is found as a sibling
`veldmodules/<dep>/schema.yaml` or as a platform service
`veldcore/interface/modules/<dep>/<dep>.schema.yaml`; there is no third
place. The consumer never redeclares a payload type: the validator reads it
from the producer's schema, where `module/` means the producer's package.
Platform services are declared the same way (`app: subs: [on_ui_event]`); their
types come from `veldsdk::proto::<alias>`, and the SDK carries no stubs for
them — an undeclared call would be an edge missing from the schemas.

The build order follows the same block: `buildgen/schema_deps.py` reads every
`dependencies`, and `build.py` sorts the modules so that a producer is built
before its consumers, whose crates depend on its wrap crate by path.

A dependency is permission as well: a schema may name a foreign package in its
own `type:` only if that module is declared, and modules that know nothing of
each other do not share a type — `data-provider` and `globe` each declare
their own `GeoPoint`, and `data-browser`, which talks to both, translates at
the border.

## The wrap crate

A wrap crate exists for every module with a `types.proto`:
`generated/wraps/rust/`, package `<package>-wrap`, built once per producer and
shared by all its consumers. Its generated `lib.rs` holds the prost types of
`types.proto` under `proto` and, when the module has `wraps/rust/src/wrap.rs`,
includes that file through `#[path]` and re-exports it; without a handwritten
file the crate is the messages and nothing else (`data-library`,
`data-provider`, `image-view`).

The handwritten part has two purposes, and no third.

**An API where the generated one is not enough.** `ui-service`
(`veldmodules/ui-service/wraps/rust/src/`): `widgets.rs` — typed builders of
the layout tree (`column`, `row`, `text`, `container`, …), `style.rs` — colours,
lengths and paddings as Rust types over the proto ones, `render.rs` — sending
a layout. The raw messages stay reachable through `crate::proto`.

**One code for the producer and its consumers.** A file of the module is
included into the wrap through `#[path]`, so both sides compile the same
source:

| Shared file | Included by | What must not diverge |
|---|---|---|
| `veldmodules/image-tiler/src/pyramid.rs` | the tiler, its wrap | `TILE`, level sizes, the level count |
| `veldmodules/tile-cache/src/tile.rs` | the cache, its wrap, the tiler's wrap | `TILE_FORMAT`, `MAX_QUERY_TILES` — the cache's ceiling and the consumer's appetite |
| `veldmodules/globe/src/geodesy.rs` | the globe, its wrap | the one transition from geographic to Cartesian coordinates: an outline is drawn and a click is tested by the same arithmetic |
| `veldmodules/globe/src/wheel.rs` | the globe, its wrap | the zoom per wheel click, one number for the globe and the canvas |
| `veldmodules/ui-service/src/typography.rs` | ui-service, its wrap | the font names and the default text size; a private `mod`, its names re-exported from `style.rs` |

The consumers' mechanics of the pyramid — the store of tiles in video memory,
what to want for the current view, what is already asked —
`veldmodules/image-tiler/wraps/rust/src/tiles.rs` — live in the tiler's wrap
itself, one implementation for the canvas and the globe, built on `pyramid`
and `tile` from the same crate ([viewing pipeline](viewing-pipeline.md)).

A shared file is compiled twice, in two crates with two manifests; hence:

- A crate it uses is declared in both `rust.dependencies` and
  `rust.wrap_dependencies`, in one version. `buildgen/tests/test_wrap_deps.py`
  finds the `#[path]` includes, checks every `use` of theirs against both
  lists, and holds a crate that reaches consumers through a wrap to one
  version across the tree (`glam` for the globe). The lists are separate on
  purpose: the wrap must not carry everything the producer uses for itself.
- Nothing from `crate::` — inside the wrap, `crate` is the wrap.
- `pub` means visible to consumers; `pub(crate)` means visible within
  whichever crate is compiling the file. An item the module's other files
  call but nothing in the wrap reaches cannot be narrowed: the wrap build
  would report it unused (`parts` in `geodesy.rs`), while `FLATTENING` there
  is `pub(crate)` because the public `intersect_at` uses it. Some `pub` in a
  shared file is the price of a single copy, not a promise of API.

**The wrap never publishes.** It is one crate for every consumer and cannot
know which of them declared `calls: [on_set_view]`; a publication from inside
it would be an edge missing from the schemas, invisible to the validator and
to the build order. A helper that ends in a publication takes the consumer's
stub as an argument — `render::render(root, crate::calls::ui_service::on_set_view)`,
`veldsdk::surface::delegate` likewise.

## The ABI

What a module changes in the host goes through synchronous calls, not the
bus (`veldsdk::abi`, implemented in `veldcore/platform/host/core/src/abi.rs`):

| Group | Calls |
|---|---|
| bus and log | `veld_host_publish`, `veld_host_log` |
| system | `veld_random_bytes` |
| creating resources | `veld_resource_alloc_cpu`, `veld_resource_alloc_buffer`, `veld_resource_alloc_texture`, `veld_resource_create` |
| resource data | `veld_resource_read`, `veld_resource_write`, `veld_resource_upload_image`, `veld_resource_texture_size` |
| ownership | `veld_resource_transfer`, `veld_resource_grant_read`, `veld_resource_grant_write`, `veld_resource_free` |
| graphics | `veld_graphics_execute` |
| tasks | `veld_task_kill` |
| the call's context | `veld_input_len`, `veld_input_copy`, `veld_output_set` |

Four calls create a resource and all put a record into one registry: the
`alloc_*` calls take scalars (size; width, height, format, usage),
`veld_resource_create` takes a nested descriptor (`graphics.ResourceRequest`:
shader, pipeline, sampler, view, bind group and its layout). The line between
them runs by the shape of the arguments, not by the kind of resource, and one
`veld_resource_free` frees them all. Writing into a resource and uploading an
image are different calls: a texture has neither an offset nor a partial
update, so nothing of `write(id, offset, data)` applies to it. A depth buffer
is allocated by the same `alloc_texture` with `TEX_DEPTH32_FLOAT`; what that
format may and may not do is one predicate, `format::is_depth`
([window-and-render](window-and-render.md)).

A refusal is reported in one of two ways, chosen by whether the caller tells
the causes apart. Several causes — a tagged answer: the first byte is `0`
(success) or `1` followed by the reason; so answer `read`, `write`,
`upload_image`, `resource_create`, `graphics_execute`, whose refusals are "no
right", "no such resource", "read-only carrier" or "wrong kind". One cause — a
number, `0` for "no": so answer the `alloc_*` calls, `texture_size`, `free`,
`transfer`, `grant_*`, `task_kill`; a reason would add nothing there, and a
tagged answer would cost an allocation per call. The encoding of the tagged
answer is one file for the host, the fake host and the SDK
(`veldcore/sdk/rust/src/abi/wire.rs`, [invariants](invariants.md)).

A module exports `init`, `handle_event`, `get_subscriptions`,
`get_service_name`, `veld_alloc` and `veld_free_wasm`; the host takes the
service's name on the bus from `get_service_name` of the binary itself.

## Adding a module

1. `veldmodules/<name>/schema.yaml` with `name: <name>`, and `config.yaml` with
   `package` and `rust.dependencies`.
2. `types.proto` (package `veldmap.<name>`) when the module has messages of
   its own; the schema names them `module/<Message>`.
3. `src/module.rs`: `Config` (serde `Deserialize`), `State`,
   `hook_init(Config) -> anyhow::Result<State>`, a handler per input and per
   subscribed topic, `hook_event` if listed in `hooks`.
4. `runtime/config/<name>.json`. When it is absent the host warns and passes
   `{}`, so `Config` must then accept an empty object.
5. `python3 buildgen/build.py`.

Nothing is registered anywhere: the host loads every `.wasm` from the plugins
directory (`plugins_dir` in `runtime/config/services.json`, held equal to
`workspace.yaml` by the build), asks each binary its name, and refuses a
duplicate. A native service is added differently — a contract under
`veldcore/interface/modules/<name>/`, an implementation under
`veldcore/platform/host/modules/<name>/` with `config.yaml` and
`src/module.rs`, and a line in the runner's `runner.yaml`.
