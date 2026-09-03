# Build and check

One command builds and checks everything; a second one runs the application:

```bash
python3 buildgen/build.py          # release build — the only accepted check
python3 buildgen/run-native.py     # run
```

`build.py` without arguments is the release build, and every change is checked
with it. In order: the buildgen tests (pytest, fractions of a second), code
generation from the schemas, every wasm module, the native host, then the
native unit tests; the total time is printed at the end. With a warm `target/`
an unchanged tree builds in seconds, a changed module in tens of seconds, and
a change in the host core is the slowest.

Cargo commands are not echoed; a failing one is printed in full, and
`--verbose` echoes them all. `--debug` is not needed: a debug build checks
something other than what runs, and its wasm modules instantiate noticeably
slower. `--windows --dist-dir <dir>` cross-builds the host for
`x86_64-pc-windows-gnu` (`x86_64-w64-mingw32-gcc` on the machine) and lays the
binary, the plugins and `runtime/` out in `<dir>`; `run-native.py --debug` and
`--config <dir>` are in [configuration](configuration.md).

**Only this command.** `cargo build` or `cargo check` on a single crate skips
code generation and builds part of the tree, so a schema that drifted from the
code goes unnoticed.

## What the tests cover

The buildgen tests (`buildgen/tests/`) pin what the compiler cannot: the
schema validator's rules — the one thing that keeps a schema from drifting
from the code — and the regressions on the live schemas (which exchanges are
killable, the sorted FLOW table the host searches); `.proto` tracking in the
generated `build.rs`; that rewriting an unchanged file is not a change for
cargo; wrap-crate dependencies; TIFF compressions against the crate's
features; the words by which the scenario runner recognises a failure in
`host.log`; environment variable names in the config and in the provider's
advice; values that live on both sides of the wire
(`buildgen/tests/test_wire_pairs.py`: one table, one parser); the
documentation rules (`buildgen/tests/test_docs.py`); the warning parser and
the workspace listing of the unit-test step. What they have in common: a
weakened check does not break the build, so a change to `generate.py` comes
with a test, and so does a change to the failure words in the host. They run
first in the build and can be run alone:

```bash
buildgen/.venv/bin/python -m pytest buildgen/tests
```

The venv is described by `buildgen/requirements.txt` and set up by the build.

Rust unit tests cover pure logic and nothing that needs a screen or a network:
a function over its arguments, or two functions that must agree mechanically
(a projection and its inverse, the encoder and decoder of a message). The
desktop runner is no exception: its scenario parser and its clock are pure
logic and are tested; its test target builds because it pulls winit and wgpu
in the ordinary build anyway.
They live in the files themselves under `#[cfg(test)]` and run natively: the
host ABI is stubbed by the SDK (`veldsdk::abi`, the `not(target_arch =
"wasm32")` branch). The list of what is covered is
`grep -rl '#\[cfg(test)\]' veldcore veldmodules`; a change to such logic comes
with a test beside it.

The last step of the build runs `cargo test --workspace` for `veldcore` (SDK,
host core and its `util` crate, host modules, desktop runner), then, module by
module, every module that declares tests and every wrap crate that does — the
wrap of `image-tiler` holds the tile mechanics shared by the canvas and the
globe. Compiler warnings from those
test targets are collected and printed after the summary: an orphan without
`#[test]`, a duplicated attribute or an unused helper is something the compiler
sees and the test runner does not.

`ui-service` has no native tests: its test target would pull the windowing
backend of iced, which does not build for the host here. Pure logic that needs
a test belongs in its wrap crate, which does not pull iced; today that wrap
holds no tests.

Tests can be run separately; the directory matters (see below):

```bash
cd veldcore && cargo test --release -q --workspace
cd veldmodules/<name>/generated && cargo test --release -q
cd veldmodules/<name>/generated/wraps/rust && cargo test --release -q
```

## Mutation check

`buildgen/mutate.py` breaks the code in a named way and expects red; it is run
by hand, not by the build. Mutations live in `buildgen/mutations/`, one file
per crate, one block per mutation; a new test seam is accepted together with a
mutation named there. The script's header gives the format and the command.

## What not to do

| Do not | Why |
|---|---|
| `build.py clean`, `cargo clean`, deleting `target/` or `generated/` | the next build starts from nothing — wasm, host and the native test targets — and that is incomparably longer than any change |
| change `[profile.*]` in `veldcore/Cargo.toml` or the rustflags in `veldcore/.cargo/config.toml` | changes cargo's fingerprint, and the whole workspace rebuilds |
| run `cargo` by hand from any directory but the manifest's | cargo looks for the toolchain and the rustflags from the current directory; another directory means another fingerprint and a full rebuild |
| edit files under `*/generated/` | generator output, overwritten by every build |

Two compiler messages are worth knowing. `rustc interrupted by SIGSEGV,
printing backtrace` is not a failure: rustc prints `resuming signal` and
finishes; the cause is the parallel front end enabled in
`veldcore/.cargo/config.toml`. `internal compiler error: encountered
incremental compilation error` is a failure with one cure:

```bash
rm -rf veldcore/target/release/incremental
```

It has followed a change to a `.proto` under `veldcore/interface/` — the
generated bindings crate changes and the host's incremental cache is left from
the old one — and, twice on the same day, a change inside the network module
with the same query named in the panic. Only `incremental/` goes; it rebuilds
in minutes, the whole `target/` does not.
