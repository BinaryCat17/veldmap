//! Фоновая работа нативного модуля хоста: `spawn` запускает её и делает
//! убиваемой. Учёт операций ведёт диспетчер — см. `core::dispatcher::account`,
//! он же их и убивает.

use std::future::Future;
use std::sync::Arc;
use veldmap_host_core::dispatcher::Caller;
use crate::HostContext;

pub struct Tasks {
    ctx: Arc<HostContext>,
}

impl Tasks {
    /// Привязка фасада к контексту — в init() модуля, как State::new у wasm.
    pub fn new(ctx: &Arc<HostContext>) -> Self {
        Self { ctx: ctx.clone() }
    }

    /// Запускает фоновую работу и отдаёт платформе то, чем её снять.
    /// Работа по неучтённому топику (`!caller.accounted`) просто выполняется
    /// до конца: убивать её некому и нечем.
    ///
    /// Итог уезжает заказчику терминальным ответом самого исполнителя — он же
    /// снимает операцию с учёта. У убитой такого ответа нет, и за неё отвечает
    /// хост (`Dispatcher::kill`), так что заказчик в обоих исходах получает
    /// ровно один конец операции.
    pub fn spawn<G, F>(&self, caller: &Caller, make: G)
    where
        G: FnOnce(String) -> F + Send + 'static,
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let id = caller.correlation.clone();
        let join = tokio::spawn(async move { let _ = make(id).await; });

        if !caller.accounted {
            return;
        }
        // Учёт открыт — значит запись была на момент публикации запроса, и её
        // отсутствие сейчас означает ровно одно: операцию успели убить в окне
        // между публикацией и этим моментом. Тогда abort'им сами, чтобы работа
        // не пережила собственное убийство.
        let abort = join.abort_handle();
        if !self.ctx.tasks.arm(&caller.correlation, |victim| victim.abort = Some(abort)) {
            join.abort();
        }
    }
}
