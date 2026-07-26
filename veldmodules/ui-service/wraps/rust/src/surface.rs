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
/// `attach` — стаб топика app/on_set_surface из кодогена вызывающего модуля:
/// `surface::delegate(&ev, old, crate::calls::app::on_set_surface)`. Wrap-крейт
/// не публикует туда сам, потому что аттачит окно не он, а его потребитель —
/// владелец окна, и объявить `app: calls: [on_set_surface]` должна схема
/// именно этого модуля.
pub fn delegate(
    ev: &veldsdk::proto::app::WindowResized,
    old_texture: Option<u64>,
    attach: impl FnOnce(&veldsdk::proto::app::SetSurface),
) -> Option<u64> {
    let texture_id = abi::arena_alloc_texture(ev.width, ev.height, ev.format, RENDER_TARGET_USAGE)?;

    if !abi::arena_grant_write(texture_id, "ui-service") {
        veldsdk::verror!(veldsdk::FLAG_SDK, "[SURFACE] grant_write to ui-service failed for texture {}", texture_id);
        abi::arena_free(texture_id);
        return None;
    }

    let handle = veldsdk::proto::core::ResourceHandle { id: texture_id, size: 0, content_hash: Vec::new() };

    crate::inputs::on_set_surface(&crate::proto::SetSurfaceRequest {
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
