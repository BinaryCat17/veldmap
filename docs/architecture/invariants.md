# Invariants and their holders

What must stay true across the tree, and what holds it. A holder is one of:
a **type** (the compiler refuses the violation), the **validator** of the
schema, a **test** by name, a **shared constant** (one file included on both
sides through `#[path]` or a wrap crate), a **pytest pair**
(`buildgen/tests/test_wire_pairs.py`, for values on two sides of the host that
no crate can join), or a **comment** — which is a debt, listed so that it can
be paid. The list of comment-held rows must shrink with every step of
[the roadmap](../roadmap.md).

| Invariant | Holder |
|---|---|
| A topic exists only if `schema.yaml` declares it; a foreign topic needs a declared dependency; a cancellable input has a reply; a request with several replies has one terminal | validator (`buildgen/generate.py`), `buildgen/tests/test_project.py` |
| The FLOW table the host searches is sorted and complete | test `test_the_flow_table_reaches_the_host` |
| The terminal reply comes for a dead executor and for a missing subscriber | tests in `veldcore/platform/host/core/src/dispatcher.rs` and `tasks.rs` |
| A host-settled reply is read as a refusal, never as success | `veldsdk::reply::undelivered`, test in `veldcore/sdk/rust/src/reply.rs` |
| The answer of a synchronous ABI call is encoded the same by host, fake host and SDK | shared file `veldcore/sdk/rust/src/abi/wire.rs` |
| Log levels mean the same on both sides of the wire | shared file `veldcore/sdk/rust/src/abi/log_level.rs` |
| The wheel notch of the runner equals the raw notch of `ui-service` | pytest pair |
| The `.part` suffix of network equals the library's | pytest pair |
| The tiler's `budget::INSTANCE` equals the host's `INSTANCE_MEMORY_LIMIT` | pytest pair |
| Zoom per wheel click is one number for the globe and the canvas | shared constant `veldmodules/globe/src/wheel.rs` through the wrap |
| Producer and consumers compute the pyramid alike | shared file `veldmodules/image-tiler/src/pyramid.rs` through the wrap |
| The level table on the wire (`Described.levels`) is the table `produce` reads its branch from | one `Info::levels()` (`veldmodules/image-tiler/src/adapters/table.rs`) fills both; test `the_level_table_and_the_produce_branch_agree` in `adapters/reads.rs` holds it against the driver's window rule |
| The window rule promises nothing the direct read refuses | tests `окно_не_обещает_того_чего_проход_не_отдаст`, `задетые_чанки_сходятся_с_перебором_смещений` in `adapters/tiff.rs` |
| The memory of every tiler path is a `budget::Peak` of named terms summed against `budget::free()`; a decoder's own ceiling is named in the level table | `budget::Peak`, `Info::levels`; tests in `adapters/table.rs` on real sizes |
| Cached tiles are keyed by the decoding rules | one constant `DECODE_REV` in `veldmodules/image-tiler/src/fingerprint.rs` |
| The consumer's tile cap does not exceed the cache's `MAX_QUERY_TILES` | shared constant `veldmodules/tile-cache/src/tile.rs`, included by the tiler's wrap; test `потолок_аппетита_не_выше_потолка_кэша` in `wraps/rust/src/tiles.rs` |
| The ripple of the globe's shader equals the Rust side | comment (WGSL imports no constant) |
| A shown scene is not also an overlay | comment in `veldmodules/data-browser/src/state/mod.rs` |
