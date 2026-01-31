# VeldMap

A modern, decentralized P2P terrain visualization platform.

## Architecture

VeldMap is built as a microservice orchestration system using WebAssembly (WASM) and P2P networking:

- **`proto/`**: Single Source of Truth. Defines all service interfaces using Protocol Buffers.
- **`veldmap-rust-rpc`**: Generated Rust bindings for Protobuf messages.
- **`veldmap-core`**: The Orchestrator (Host). Manages WASM plugins via **Extism** and P2P communication via **Iroh**.
- **WASM Microservices**:
    - **`data-provider`**: External data sources (e.g., Copernicus CDSE).
    - **`local-storage`**: Local DEM data management.
    - **`tile-server`**: Map tiling logic.

## Communication

All modules communicate exclusively via **Protobuf**. 
- Local modules are called via **Extism Host-to-WASM** calls.
- Remote modules communicate over **Iroh** P2P connections.
- State synchronization is handled via **iroh-gossip** and CRDTs.

## Development

### Building Plugins
To build a plugin as a WASM module:
```bash
cd veldmap-data-provider
cargo build --target wasm32-wasip1
```

### Running the Core
The core loads services defined in `config/services.json`. 
WASM modules should be placed in the `plugins/` directory.