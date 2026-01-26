# Technology Stack

## Core Technologies
- **Language:** Rust (edition 2021)
- **Graphics API:** `wgpu` (v24.0.0) — Modern, cross-platform graphics and compute API.
- **Windowing & Events:** `winit` (v0.29) — Cross-platform window creation and management.

## Frameworks & Libraries
- **Asynchronous Runtime:** `tokio` (v1.37) & `pollster` — For concurrent data loading and async initialization.
- **Math:** `glam` (v0.29) — Simple and fast linear algebra library.
- **Serialization/Data:** `bytemuck` — For casting between Rust structures and byte buffers for GPU usage.
- **Image Handling:** `image` & `tiff` — For processing textures and Digital Elevation Model (DEM) data.
- **Error Management:** `anyhow` — Flexible error handling.
- **Logging:** `log` & `env_logger` — Standard logging infrastructure.

## Infrastructure
- **Build System:** Cargo (Rust's package manager).
- **Shader Language:** WGSL (WebGPU Shading Language).
