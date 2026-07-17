pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod host;

pub static MODULE_STATE: once_cell::sync::OnceCell<
    anyhow::Result<std::sync::Arc<std::sync::Mutex<Box<dyn std::any::Any + Send + Sync>>>>,
> = once_cell::sync::OnceCell::new();

/// Состояние сервиса
pub struct ServiceState<S> {
    pub state: std::sync::Arc<std::sync::Mutex<S>>,
}

// Топики сообщений объявляются только в schema.yaml: кодоген создаёт
// типизированные стабы crate::emit::* (interface.outputs) и crate::calls::*
// (dependencies.*.calls). Строковые топики в коде модулей запрещены.

/// Вспомогательная функция для вызова хэндлера
pub fn call_handler<S, Req, F>(
    func: F,
    state: &mut S,
    payload: &[u8],
) -> anyhow::Result<()>
where
    Req: prost::Message + Default,
    F: Fn(&mut S, Req),
{
    let req = Req::decode(payload).map_err(|e| anyhow::anyhow!("Decode error: {}", e))?;
    func(state, req);
    Ok(())
}
