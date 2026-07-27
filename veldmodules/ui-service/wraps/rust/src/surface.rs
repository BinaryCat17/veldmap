//! Делегирование поверхности окна.
//!
//! Реакция владельца окна на app/window_resized: ритуал
//! «alloc → grant_write → attach хосту → delegate рендереру» одной функцией.

use veldsdk::abi;

// COPY_DST | TEXTURE_BINDING | RENDER_ATTACHMENT
const RENDER_TARGET_USAGE: u32 = 2 | 4 | 16;

/// Выделяет render-таргет под окно, делегирует его ui-service и аттачит
/// хосту. Старая текстура освобождается (хост блитит её до свапа — wgpu
/// держит её живой через view в bind group). Возвращает id новой текстуры.
///
/// Оба топика публикует потребитель, своими сгенерированными стабами:
/// `surface::delegate(&ev, old, crate::calls::ui_service::on_set_surface,
/// crate::calls::app::on_set_surface)`. Wrap-крейт не публикует ни в один из
/// них сам — ни в чужой `app`, ни даже в вход собственного сервиса: он один на
/// всех потребителей и не знает, кто из них объявил эту связь у себя в
/// schema.yaml. Объявить `ui-service: calls: [on_set_surface]` и
/// `app: calls: [on_set_surface]` обязана схема владельца окна, иначе граф
/// связей, по которому идут валидация и порядок сборки, будет неполным.
pub fn delegate(
    ev: &veldsdk::proto::app::WindowResized,
    old_texture: Option<u64>,
    set_surface: impl FnOnce(&crate::proto::SetSurfaceRequest),
    attach: impl FnOnce(&veldsdk::proto::app::SetSurface),
) -> Option<u64> {
    let texture_id = abi::arena_alloc_texture(ev.width, ev.height, ev.format, RENDER_TARGET_USAGE)?;

    if !abi::arena_grant_write(texture_id, "ui-service") {
        veldsdk::log::error!(target: "sdk", "[SURFACE] grant_write to ui-service failed for texture {}", texture_id);
        abi::arena_free(texture_id);
        return None;
    }

    let handle = veldsdk::proto::core::ResourceHandle { id: texture_id, size: 0 };

    set_surface(&crate::proto::SetSurfaceRequest {
        plugin_id: ev.plugin_id.clone(),
        surface: Some(handle.clone()),
        width: ev.width,
        height: ev.height,
        scale_factor: ev.scale_factor,
    });

    attach(&veldsdk::proto::app::SetSurface {
        plugin_id: ev.plugin_id.clone(),
        surface: Some(handle),
    });

    if let Some(old) = old_texture {
        abi::arena_free(old);
    }
    Some(texture_id)
}
