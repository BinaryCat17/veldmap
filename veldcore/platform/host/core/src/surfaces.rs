//! Очередь поверхностей окон: владелец аттачит текстуру, кадровый цикл раннера
//! её забирает. Как реестр задач (tasks.rs) — голое состояние без политики;
//! права проверяет фасад Surfaces (host-util).

use dashmap::DashMap;

/// Ключ — имя владельца окна (оно же plugin_id в контракте app),
/// значение — id региона текстуры, ожидающей аттача.
#[derive(Default)]
pub struct SurfaceQueue {
    pending: DashMap<String, u64>,
}

impl SurfaceQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Аттач вытесняет предыдущий, если кадровый цикл не успел его забрать:
    /// показать всё равно можно только последнюю поверхность.
    pub fn attach(&self, owner: &str, texture_id: u64) {
        self.pending.insert(owner.to_string(), texture_id);
    }

    /// Забирает ожидающую поверхность владельца — раз в кадр, из цикла раннера.
    pub fn take(&self, owner: &str) -> Option<u64> {
        self.pending.remove(owner).map(|(_, id)| id)
    }
}
