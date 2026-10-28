# Operations

## Verified baseline

- Rust `1.100.0-beta.7`, Rust 2024, MSRV 1.100; only `wasm32-wasip3` is configured.
- Core WASI interfaces and `wasi:http/client` are version `0.3.0` in inspected artifacts.
- Wasmtime, `wasmtime-wasi`, and `wasmtime-wasi-http` are exactly `50.0.0`.
- Guest HTTP bindings are `wasip3 0.9.0+wasi-0.3.0`; Tokio is `1.54.1`.
- ConnectRPC is `0.9.1`, `connectrpc-build` is `0.9.0`, and Buffa is `0.9.2`.
- `wasm-tools` is `1.262.0`; `protoc` is `36.2`.
- Buf is `1.73.0` (Linux x86_64 SHA-256 `8f2986298ad08f0cc1bf999b9797b7c383adf32d7edf0f73d6f1e1a701baeac1`).
- The optional diagnostic `wasmtime` binary is `50.0.0`; delivered components use `veldmap-host`.
- No external WIT or Buf dependencies exist, so there is no `wkg.lock` or `buf.lock`.

Install the pinned Buf binary as `target/tools/buf` from the `v1.73.0` release.
All Cargo verification uses the committed lockfile and `--locked`.

## Build and test

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p veldmap-proto -p veldmap-rpc --all-targets --target wasm32-wasip3 --features veldmap-rpc/http-example --locked -- -D warnings
cargo build -p veldmap-rpc --target wasm32-wasip3 --examples --features http-example --locked
python3 scripts/check_component_imports.py target/wasm32-wasip3/debug/examples/*.wasm
target/tools/buf lint
target/tools/buf build
```

`cargo test -p veldmap-rpc --test wasi_cache --locked` runs two real WASI 0.3
components and checks finite streaming, Tokio timers, cancellation, and producer exit.
`cargo test -p veldmap-host --test wasi_http --locked` starts a local HTTPS fixture
and checks TLS, Range, streamed reads, a Tokio spawn/timer, body-drop cancellation,
redirect rejection, and egress denial. Generated fixtures remain under `target/tmp/`.
Cold debug component compilation can take minutes; the tests allow a 120-second start.

After the Task 5a baseline is merged, check additive schema changes with:

```bash
target/tools/buf breaking --against '.git#branch=master'
```

## Launch and policy

```bash
cargo build -p veldmap-host --locked
target/debug/veldmap-host [--config POLICY] COMPONENT [GUEST_ARGS...]
```

The default policy is `runtime/config/component-imports.toml`; the component file
stem selects its entry. Interface inspection requires exactly `wasm-tools 1.262.0`.
TCP checks allow only listed literal addresses plus the unspecified ephemeral bind
required before a client connect. There is no inherited-network launch mode.
HTTP origins match exact scheme and authority; redirects are never followed.
The guest rejects 3xx responses to signed requests and must not forward credentials.

Ordinary HTTP uses Wasmtime's sender and public Mozilla roots, with its 600-second
defaults unless guest request options set timeouts. Explicit additional roots select
the verified custom-root adapter, whose default connect/first-byte/between-byte
timeouts are 10 seconds. The HTTP fixture sets all three to 5 seconds. Certificate
verification is never disabled; additional roots are explicit policy inputs.

The example scopes `wit_bindgen::block_on` around individual WASI operations inside
outer current-thread Tokio. Tokio pauses during each WASI wait and resumes between
chunks; this is cooperative interleaving, not shared wait-level concurrency.
