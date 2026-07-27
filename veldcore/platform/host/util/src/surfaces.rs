//! Фасад поверхностей окон — зеркало Tasks для контракта app. Голая очередь
//! SurfaceQueue из core модулям не экспортируется: аттач без проверки прав
//! позволил бы любому модулю подменить чужое окно.

use std::sync::Arc;
use veldmap_host_core::registry::Access;
use crate::HostContext;

pub struct Surfaces {
    ctx: Arc<HostContext>,
}

impl Surfaces {
    /// Привязка фасада к сервису — в init() модуля, как Tasks::new.
    pub fn new(ctx: &Arc<HostContext>) -> Self {
        Self { ctx: ctx.clone() }
    }

    /// Аттач с проверкой capability: отправитель обязан владеть окном
    /// (plugin_id резолвится в его instance id) и иметь право писать в текстуру.
    /// Хост (id 0) не проверяется — у него нет модульной идентичности.
    /// Отказ логируется и игнорируется: шина fire-and-forget, отвечать некому.
    pub fn attach(&self, plugin_id: &str, texture_id: u64, requestor_id: u32) {
        if requestor_id != 0 {
            if self.ctx.dispatcher.instance_of(plugin_id) != Some(requestor_id) {
                log::warn!(target: "surfaces",
                    "set_surface rejected: requestor {} is not the owner of window '{}'",
                    requestor_id, plugin_id);
                return;
            }
            if !self.ctx.registry.check_access(texture_id, requestor_id, Access::Write) {
                log::warn!(target: "surfaces",
                    "set_surface rejected: requestor {} has no write access to texture {}",
                    requestor_id, texture_id);
                return;
            }
        }
        self.ctx.surfaces.attach(plugin_id, texture_id);
    }
}
