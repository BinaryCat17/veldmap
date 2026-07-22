//! Рантайм модуля: хранение состояния между вызовами хоста и диспетчеризация
//! обработчиков. Используется сгенерированным lib.rs модуля (buildgen).
//!
//! Wasm однопоточный: состояние живёт в thread_local RefCell — никаких
//! Arc/Mutex и Send+Sync-ограничений на пользовательский State.

use std::any::Any;
use std::cell::RefCell;

thread_local! {
    static MODULE_STATE: RefCell<Option<anyhow::Result<Box<dyn Any>>>> = RefCell::new(None);
}

/// Сохраняет состояние модуля (вызывается из init сгенерированного кода).
/// Ошибка инициализации тоже сохраняется: handle_event сообщит о ней.
pub fn set_state<S: 'static>(state: anyhow::Result<S>) {
    MODULE_STATE.with(|slot| {
        *slot.borrow_mut() = Some(state.map(|s| Box::new(s) as Box<dyn Any>));
    });
}

/// Выполняет замыкание с &mut на состояние модуля.
pub fn with_state<S: 'static, R>(f: impl FnOnce(&mut S) -> R) -> Result<R, String> {
    MODULE_STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let cell = slot.as_mut().ok_or_else(|| "Module not initialized".to_string())?;
        let boxed = cell.as_mut().map_err(|e| format!("Module initialization failed: {}", e))?;
        let state = boxed.downcast_mut::<S>().expect("Failed to downcast state");
        Ok(f(state))
    })
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
