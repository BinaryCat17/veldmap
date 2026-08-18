//! Рантайм модуля: хранение состояния между вызовами хоста и диспетчеризация
//! обработчиков. Используется сгенерированным lib.rs модуля (buildgen).
//!
//! Wasm однопоточный: состояние живёт в обычной ячейке — никаких Arc/Mutex и
//! Send+Sync-ограничений на пользовательский State.

use std::any::Any;
use std::cell::RefCell;

/// Ячейка состояния модуля.
///
/// Это `static`, а не `thread_local`: у инстанса wasm один поток и собственная
/// линейная память, так что «глобальная» переменная здесь и есть переменная
/// инстанса — разделять её не с кем. TLS в этой роли дал бы ровно то же
/// хранилище, но потребовал бы регистрации деструктора потока, а у
/// wasm32-wasip1 без потоков такого механизма нет: первое же обращение к
/// `thread_local` с droppable-значением уходит в бесконечный цикл.
struct ModuleState(RefCell<Option<anyhow::Result<Box<dyn Any>>>>);

// SAFETY: параллельного доступа к ячейке не бывает — инстанс модуля вызывается
// строго по одному вызову за раз (актор владеет своим Store единолично).
// `Sync` нужен только затем, чтобы значение вообще можно было положить в
// `static`.
unsafe impl Sync for ModuleState {}

static MODULE_STATE: ModuleState = ModuleState(RefCell::new(None));

/// Сохраняет состояние модуля (вызывается из init сгенерированного кода).
/// Ошибка инициализации тоже сохраняется: handle_event сообщит о ней.
pub fn set_state<S: 'static>(state: anyhow::Result<S>) {
    *MODULE_STATE.0.borrow_mut() = Some(state.map(|s| Box::new(s) as Box<dyn Any>));
}

/// Выполняет замыкание с &mut на состояние модуля.
pub fn with_state<S: 'static, R>(f: impl FnOnce(&mut S) -> R) -> Result<R, String> {
    let mut slot = MODULE_STATE.0.borrow_mut();
    let cell = slot.as_mut().ok_or_else(|| "модуль не инициализирован".to_string())?;
    let boxed = cell.as_mut().map_err(|e| format!("инициализация модуля не удалась: {}", e))?;
    let state = boxed.downcast_mut::<S>().expect("состояние модуля не того типа");
    Ok(f(state))
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
    let req = Req::decode(payload).map_err(|e| anyhow::anyhow!("сообщение не разобрано: {}", e))?;
    func(state, req);
    Ok(())
}
