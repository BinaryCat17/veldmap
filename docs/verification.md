# Verification

## Scope and resolved rendering dependencies

These are Task 2 one-tile spike measurements, not total GPU-memory figures or future
production-cache budgets. `cargo tree --locked -e normal -p veldmap` records eframe,
egui, and egui-wgpu `0.37.0`, unified wgpu `30.0.1`, and eframe-managed winit
`0.30.13`. `cargo deny check all` passes advisories, licenses, bans, and sources.

## Exercised support matrix

| Target | Windowing and browser | GPU path | Result |
|---|---|---|---|
| Native on Ubuntu 24.04/WSL2 | X11/XWayland | Vulkan Dozen 26.0.1, AMD RX 5700 XT | Pass; hardware |
| Browser on Ubuntu 24.04/WSL2 | Chrome for Testing 145.0.7632.6 | WebGPU, bundled SwiftShader | Pass; explicit software warning |

Native and browser adapter requests are high-performance and hardware-first. Software
mode is opt-in and visible in the UI, logs, and result JSON. Linux/Dozen browser
hardware is not declared: Dozen 26.0.1 and 26.2.6 both report
`fullDrawIndexUint32 = false`. Wayland and hardware-backed browser runs were not
verified. Resize is not presented as device-loss recovery; no WebGL fallback is used.

CI runs the same native and software-browser scenarios on Ubuntu 24.04/Xvfb.

## Real application scenarios

```bash
python3 scripts/render_smoke.py native
VELDMAP_BROWSER_GPU=swiftshader python3 scripts/render_smoke.py browser
```

Each run builds/packages the application, requires positive readiness, exercises real
widget and pointer input, both clip modes, resize from 1000x720 to 900x650, native
scale 1.25 and total scale 1.5625, visible fixture colors, idle stability, and clean
shutdown. Browser shutdown calls `WebRunner::destroy`; native shutdown uses
`WM_DELETE_WINDOW`. Screenshots and complete logs are retained in `runtime/logs`.

Chrome background DBus/UPower and GCM errors are retained as `environment_notes`;
every other Chrome `ERROR:` line fails the run. Application panics, exceptions,
unhandled rejections, and GPU validation failures fail both runs.

## Measured one-tile results

| Metric | Native | Browser |
|---|---:|---:|
| Tile uploads / uploaded RGBA bytes | 1 / 256 | 1 / 256 |
| Application-owned tile cache entries | 1 | 1 |
| Retained CPU tile pixels | 0 bytes | 0 bytes |
| Custom non-pixel uploads for full scenario | 2912 bytes | 1216 bytes |
| Viewport resize observations | 2 | 2 |
| Maximum action response, excluding idle | 633.1 ms | 2174.3 ms |
| Quiet initial and final idle intervals | 1000 ms each | 1000 ms each |
| Total scenario time | 7289.8 ms | 8545.8 ms |

Each custom callback uploads only a 16-byte transform per painted frame: 32 bytes for
the two views. Scenario upload totals include startup and input frames; egui mesh and
font traffic are measured separately by eframe and are not included in that figure.
Maximum action includes the diagnostic screenshot. Idle checks require unchanged
application draw counters; there is no application-requested periodic redraw.

One eframe-owned device/queue serves both geometries and ordinary widgets. There is no
second GPU device, retained duplicate CPU tile, or per-frame CPU pixel transport.
Readback occurs only for the native Capture button or the external browser screenshot.
