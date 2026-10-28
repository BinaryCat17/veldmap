mod http;
mod policy;

use std::path::PathBuf;

use anyhow::anyhow;
use policy::Policy;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::error::Context as _;
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p3::bindings::Command;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

struct Host {
    table: ResourceTable,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    hooks: http::EgressHooks,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for Host {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.hooks,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config_path, component_path, arguments) = arguments()?;
    let component_name = component_path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("component path has no UTF-8 file stem"))?;
    let policy = Policy::load(&config_path, component_name)?;
    let hooks = http::EgressHooks::new(&policy.component, &policy.directory)?;
    let sockets = policy.sockets;
    let mut wasi = WasiCtx::builder();
    wasi.inherit_stdio()
        .allow_tcp(true)
        .socket_addr_check(move |address, usage| {
            let implicit_bind = !sockets.is_empty()
                && matches!(usage, wasmtime_wasi::sockets::SocketAddrUse::TcpBind)
                && address.ip().is_unspecified()
                && address.port() == 0;
            let allowed = implicit_bind || sockets.iter().any(|allowed| allowed.matches(address));
            if !allowed {
                eprintln!("denied socket access to {address}");
            }
            Box::pin(std::future::ready(allowed))
        });
    wasi.arg(component_path.to_string_lossy());
    for argument in arguments {
        wasi.arg(argument);
    }

    let mut config = Config::new();
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, &component_path)
        .with_context(|| format!("failed to load {}", component_path.display()))?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    let host = Host {
        table: ResourceTable::new(),
        wasi: wasi.build(),
        http: WasiHttpCtx::new(),
        hooks,
    };
    let mut store = Store::new(&engine, host);
    let command = Command::instantiate_async(&mut store, &component, &linker).await?;
    store
        .run_concurrent(async move |store| command.wasi_cli_run().call_run(store).await)
        .await??
        .map_err(|()| anyhow!("component returned a failure status"))
}

fn arguments() -> anyhow::Result<(PathBuf, PathBuf, Vec<String>)> {
    let mut arguments = std::env::args().skip(1);
    let mut config = PathBuf::from("runtime/config/component-imports.toml");
    let first = arguments.next().ok_or_else(usage)?;
    let component = if first == "--config" {
        config = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        arguments.next().map(PathBuf::from).ok_or_else(usage)?
    } else {
        PathBuf::from(first)
    };
    Ok((config, component, arguments.collect()))
}

fn usage() -> anyhow::Error {
    anyhow!("usage: veldmap-host [--config POLICY] COMPONENT [GUEST_ARGS...]")
}
