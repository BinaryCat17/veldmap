# VeldMap Development Guidelines

This document outlines the architectural principles and coding standards for the VeldMap project.

## 1. Interface-First Architecture

The core philosophy of VeldMap is strict separation of interfaces and implementations.

- **`veldmap-core`** is the only crate that defines public traits and shared data structures.
- Other crates (`render`, `data`, `geo-math`, `server`) **implement** these traits but do not expose their internal structures.
- All inter-module communication must happen through the traits defined in `core`.

## 2. Module Factory Pattern

Each implementation module must expose exactly one (or a few) public "Factory Functions" that return a trait object (usually wrapped in `Arc`).

**Example:**
```rust
// In veldmap-render/src/lib.rs
pub async fn create_renderer(...) -> Arc<dyn Renderer> { ... }
```

**Why?**
- It prevents leaking implementation details (like `wgpu` types from `render` or `axum` types from `server`) into the rest of the workspace.
- It makes it trivial to swap implementations (e.g., a Mock renderer for tests).

## 3. UniFFI Support

VeldMap is designed to be cross-platform and cross-language. We use **UniFFI** to generate bindings.

- All public traits in `veldmap-core` should be marked with `#[uniffi::export(callback_interface)]`.
- Data structures should be `uniffi::Record` or `uniffi::Object`.
- Avoid using complex Rust types (like `HashSet`, `HashMap` with custom keys, or generic types) in the public interfaces of `veldmap-core` as they might not be supported by UniFFI.

## 4. Concurrency and Async

- Use `async-trait` for traits that require asynchronous operations.
- Prefer `Arc<dyn Trait>` for shared ownership across threads.
- In `veldmap-render`, use `Mutex` or `RwLock` internally to handle `winit` events and internal state updates, as the `Renderer` trait is `Send + Sync`.

## 5. Coding Style

- Follow standard Rust naming conventions (`snake_case` for functions/variables, `PascalCase` for types).
- Documentation comments (`///`) are encouraged for public interfaces in `veldmap-core`.
- Keep implementation modules private (`pub(crate)`) except for the factory function in `lib.rs`.

## 6. Rendering Principles (veldmap-render)

- We use **Ray-Marching** for terrain rendering to avoid large vertex buffers.
- Terrain data is stored in a **Storage Buffer** (acting as a virtual texture cache).
- Use **Indirection Textures** to map geographic tiles to physical slots in the storage buffer.
