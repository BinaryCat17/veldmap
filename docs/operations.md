# Operations

## Toolchain and packages

- Rust `1.99.1`, Rust 2024, MSRV 1.99; browser target is `wasm32-unknown-unknown`.
- Workspace packages are `veldmap`, `veldmap-core`, `veldmap-render`, and `veldmap-storage`.
- `veldmodules/` is retained outside the workspace as migration input.
- Task 2 selects eframe, egui, and egui-wgpu `0.37.0` with unified wgpu
  `30.0.1` and eframe-managed winit; wasm-bindgen is `0.2.129`.
- Selected direct dependencies use exact pins in `Cargo.toml`.

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
product user flows. Task 2 adds the eframe application and browser packaging.
