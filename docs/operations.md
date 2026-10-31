# Operations

## Toolchain and packages

- Rust `1.99.1`, Rust 2024, MSRV 1.99; browser target is `wasm32-unknown-unknown`.
- Workspace packages are `veldmap`, `veldmap-core`, `veldmap-render`, and `veldmap-storage`.
- `veldmodules/` is retained outside the workspace as migration input.
- eframe, egui, and egui-wgpu are exactly `0.37.0`; unified wgpu is `30.0.1`.
- eframe resolves winit `0.30.13`; no independent winit pin or rendering shell is used.
- wasm-bindgen and its CLI are exactly `0.2.129`; browser libraries use `0.3.106`.
- CI installs Chromium through Playwright `1.58.2`, resolved to Chrome for Testing
  `145.0.7632.6`. Direct Rust dependencies use exact pins in `Cargo.toml` and `Cargo.lock`.

All Cargo verification uses `--locked`.

## Build and dependency checks

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --target wasm32-unknown-unknown --locked -- -D warnings
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --locked
cargo build -p veldmap -p veldmap-core -p veldmap-render -p veldmap-storage --target wasm32-unknown-unknown --locked
cargo tree --locked -e normal -p veldmap
cargo deny check all
```

Install tools with `cargo install wasm-bindgen-cli --version 0.2.129 --locked` and
`cargo install cargo-deny --locked`. CI runs the commands above, dependency checks,
and both real smoke scenarios. Native Linux CI needs the development libraries,
`mesa-vulkan-drivers`, `xauth`, and `xvfb` installed.

## Native launch and smoke

```bash
cargo run -p veldmap --locked
python3 scripts/render_smoke.py native
xvfb-run -a -s "-screen 0 1600x1200x24" python3 scripts/render_smoke.py native
```

Native rendering requests a high-performance Vulkan adapter and permits noncompliant
drivers so Dozen can be verified. CPU adapters remain available but produce a visible
warning. The smoke runner uses X11/XWayland and requires Python 3 and an X display.

## Browser package and smoke

```bash
cargo install wasm-bindgen-cli --version 0.2.129 --locked
npx --yes playwright@1.58.2 install chromium
python3 scripts/render_smoke.py browser
xvfb-run -a -s "-screen 0 1600x1200x24" python3 scripts/render_smoke.py browser
```

Browser packaging writes `target/web`. Set `CHROME_BIN` to override browser discovery.
The default `VELDMAP_BROWSER_GPU=native` requests the hardware-backed browser path.
Use `VELDMAP_BROWSER_GPU=swiftshader` only for explicit software-WebGPU verification;
it prints a warning and records `software_gpu: true`. Use a native Windows checkout
for Windows Chrome; cross-WSL browser profiles are not supported.

Logs, screenshots, and result JSON are written under `runtime/logs`. See
`docs/verification.md` for the exercised matrix and measured one-tile bounds.
