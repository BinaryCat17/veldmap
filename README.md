# VeldMap

A desktop application for working with satellite imagery: walking and searching
the provider catalogue (Copernicus Data Space), downloading products into a
local library, and viewing them — including imagery that has not been
downloaded, straight from remote storage. Built as a microkernel: a native Rust
host with a WebAssembly runtime, while all application logic lives in isolated
wasm modules that communicate over an event bus.

## Requirements

| | |
|---|---|
| Rust | nightly (pinned in `veldcore/rust-toolchain.toml`) — for `-Zthreads` in `veldcore/.cargo/config.toml`; the code uses no unstable language features |
| Build targets | `wasm32-wasip1` for the modules, `x86_64-unknown-linux-gnu` for the host |
| Python | 3.10+ (code generation and build scripts; `buildgen/.venv` is created automatically) |
| GPU | Vulkan |
| Linking (Linux) | `clang` + `mold` |
| Cross-compiling for Windows | `x86_64-w64-mingw32-gcc` |

## Build and run

```bash
python3 buildgen/build.py          # the single build and check command
python3 buildgen/run-native.py     # run
```

What the build does step by step, how to run the tests individually, how to run
the interface scenarios — [docs/operations/](docs/operations/build.md). The
development process — [CLAUDE.md](CLAUDE.md).

## Copernicus access

Catalogue browsing and downloading require S3 keys for Copernicus Data Space:
copy [`.env.example`](.env.example) to `.env` and fill in your own
(`COPERNICUS_ACCESS_KEY`, `COPERNICUS_ACCESS_SECRET`). Without them the build
and the run still pass and catalogue search works, but storage listing, viewing
over the network and downloading answer with a refusal. How the values reach
the configs — [docs/operations/configuration.md](docs/operations/configuration.md).

## Documentation

- [docs/architecture/overview.md](docs/architecture/overview.md) — the map of
  the repository, the microkernel, the services and pointers to the other pages.
- [docs/glossary.md](docs/glossary.md) — terms; [docs/limitations.md](docs/limitations.md)
  — what the application does not do and why; [docs/decisions/](docs/decisions/README.md)
  — accepted decisions with their measurements.

## License

See [LICENSE](LICENSE).
