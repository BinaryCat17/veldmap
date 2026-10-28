# Operations

## Toolchain and packages

- Rust `1.99.1`, Rust 2024, MSRV 1.99; browser target is `wasm32-unknown-unknown`.
- Workspace packages are `veldmap`, `veldmap-core`, `veldmap-render`, and `veldmap-storage`.
- `veldmodules/` is retained outside the workspace as migration input.
- Iced's candidate is `cd04b74ca7073766f654535d6e72c452ef749d07`; wgpu is `30.0.1` and wasm-bindgen is `0.2.129`.
- Direct dependencies use exact pins in `Cargo.toml`; the complete candidate baseline is in `plan.md`.

All Cargo verification uses the committed lockfile and `--locked`.

## Build and test

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --target wasm32-unknown-unknown --locked
```

## Launch

```bash
cargo run -p veldmap --locked
```

The native command and browser start export are minimal task 1 entry points, not
product user flows. Later tasks add the Iced application and browser packaging.
