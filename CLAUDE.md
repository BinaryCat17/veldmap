# Working on the project

A memo on the process. The design of the project is described in
[README.md](README.md) and in `docs/architecture/`, the terms — in
[docs/glossary.md](docs/glossary.md). An edit made without this will diverge
from the architecture, not from the compiler.

**The work queue is [docs/roadmap.md](docs/roadmap.md).** A step is taken as a
whole, with its own check and its own docs/ pages; the "What not to do" section
there is the boundary that an edit does not cross.

## Build and check

```bash
python3 buildgen/build.py          # the single build and check command
python3 buildgen/run-native.py     # run
```

`build.py` without arguments is the release build, and every edit is checked
with exactly it: the buildgen tests, the code generation, all wasm modules, the
host, the native unit tests. What it does step by step, how to run the tests
individually and what the two compiler messages mean —
[docs/operations/build.md](docs/operations/build.md).

**This command only.** `cargo build` and `cargo check` on individual crates skip
the code generation and build part of the tree: such a check will not catch a
divergence of the schema and the code. There is no need to add `--debug`.

**What the tests cover.** Unit tests cover the pure logic: a function over its
own arguments, or a pair of functions obliged to agree mechanically. The tests
live in the files themselves (`#[cfg(test)]`) and run natively on the SDK
stubs; an edit to such logic comes with a test next to it. Quantities on the
different sides of the wire are reconciled by a shared file through a wrap
crate, or by the `buildgen/tests/test_wire_pairs.py` table. The interface —
rendering, the bus, the input — is checked only by running it: "the build
passed" does not mean "the interface works". The repeatable run-based check is
a scenario run, [docs/operations/ui-tests.md](docs/operations/ui-tests.md):

```bash
python3 buildgen/run-uitests.py            # all scenarios
python3 buildgen/run-uitests.py tabs menus # the named ones
```

Whether the tests actually hold the code is checked manually with mutations:
`buildgen/mutate.py`.

## What not to do

| Do not | Why |
|---|---|
| `build.py clean`, `cargo clean`, deleting `target/` or `generated/` | the next build starts from scratch — wasm, host and the native test targets — and that is incomparably longer than any edit |
| Change `[profile.*]` in `veldcore/Cargo.toml` or the rustflags in `veldcore/.cargo/config.toml` | it changes the cargo fingerprint — the whole workspace is rebuilt from scratch |
| Run `cargo` by hand from a directory other than the manifest directory | cargo looks for the toolchain and rustflags from the cwd; from another directory you get a different fingerprint and a full rebuild |
| Edit files under `*/generated/` | that is the code generator output, overwritten on every build |
| Introduce a string topic in a module's code | topics are declared only in `schema.yaml`; they are accessed through `crate::emit::*` and `crate::calls::*` |
| Put secrets into `runtime/config/*.json` | the configs are in git; secrets are substituted as `${VAR}` from `.env` (which is in `.gitignore`) |
| Commit anything downloaded under `runtime/data/` | the directory is in `.gitignore`, the test data lives locally |

## Diagnostics

The logs of every run lie in `runtime/logs/`: `host.log` is the same as the
console, `trace.log` is the full stream. The performance counters and the rules
for reading them — [docs/operations/diagnostics.md](docs/operations/diagnostics.md).

## Style

Code comments are written in Russian; they explain the meaning and the design,
not the line below; the rules are in
[ADR 0001](docs/decisions/0001-documentation-and-comments.md). Keep the
naming, the comment density and the file structure the same as in the
neighbouring code.

**No history at all.** A comment describes the code as it is, not the path by
which it became so: "it used to be X, now it is Y", "moved here", "restored as
it was" get deleted rather than appended — the code changes while such a note
stays and starts to lie; what was before is remembered by `git log`. There is
one exception: if an old decision explains a live constraint — why the obvious
way is not allowed — the reason stays, but in the present tense and as a rule,
not as a chronicle. The check is simple: if mentioning the past is removed and
the comment stops forbidding or explaining anything — that was history. The
same applies to README.md and to this file.
