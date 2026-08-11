//! Делегирование render-таргета чужому рендереру.
//!
//! Обряд «выделить текстуру → выдать право записи → сказать рендереру» один и
//! тот же, кому бы место ни отдавали: владелец окна отдаёт поверхность окна
//! ui-service, автор разметки отдаёт трёхмерному виду место под него. Написанный
//! по разу на каждом call-сайте, он заводит по одному месту, где можно забыть
//! про грант или про освобождение прежней текстуры.
//!
//! Публикует при этом не SDK, а вызывающий: топик обязан быть объявлен в его
//! `schema.yaml` (`<сервис>: calls: [on_set_surface]`), иначе граф связей, по
//! которому идут валидация и порядок сборки, будет неполным.

use crate::abi;
use crate::graphics::texture_usage;
use crate::proto::core::{ResourceHandle, SurfaceDelegated};

/// Права текстуры, отдаваемой рендереру: он в неё рисует
/// (`RENDER_ATTACHMENT`), а показывает её потом кто-то ещё — либо композитор
/// окна, либо разметка (`TEXTURE_BINDING`).
const RENDER_TARGET_USAGE: u32 = texture_usage::COPY_DST
    | texture_usage::TEXTURE_BINDING
    | texture_usage::RENDER_ATTACHMENT;

/// Текстура, отданная рендереру.
///
/// Владеющая: место под рендер живёт ровно столько, сколько его держит хозяин,
/// и закрытая вкладка обязана освободить своё, ничего для этого не делая.
/// Голый id потребовал бы помнить об освобождении и здесь, при перевыделении,
/// и там, где хозяин заканчивается, — двух мест на один факт достаточно, чтобы
/// однажды осталось одно.
pub struct Delegated {
    texture: crate::OwnedResource,
    pub width: u32,
    pub height: u32,
}

impl Delegated {
    pub fn id(&self) -> u64 {
        self.texture.id()
    }

    /// Ручка текстуры — для тех сообщений, где место надо назвать. Размера у
    /// неё нет: объём текстуры задают ширина, высота и формат, а не длина в
    /// байтах (см. `ResourceHandle` в core.proto).
    pub fn handle(&self) -> ResourceHandle {
        self.texture.handle()
    }

    /// Тот же ли это размер. Вопрос задаётся до выделения: место в разметке
    /// пересчитывается чаще, чем меняется, а перевыделение меняет id текстуры —
    /// по нему рендерер и решает, что таргет сменился.
    pub fn covers(current: Option<&Self>, width: u32, height: u32) -> bool {
        matches!(current, Some(d) if (d.width, d.height) == (width, height))
    }
}

/// Выделяет render-таргет, раздаёт права и отдаёт его названному рендереру.
///
/// `renderer` получает право записи — он в текстуру рисует. `readers` — право
/// чтения: тем, кто её потом показывает. У поверхности окна читателей нет
/// (её забирает композитор хоста, а он не модуль), у места в разметке читатель
/// один — ui-service, который её сэмплит.
///
/// `previous` — то, что отдавали в прошлый раз; освобождается после того, как
/// новая текстура ушла рендереру. При отказе (не выделилась текстура, не вышел
/// грант) возвращается прежнее: старая поверхность продолжает действовать,
/// рендерер про новую не узнал, а потеряв её id, освободить её было бы уже
/// некому — и утекала бы текстура на каждый такой отказ. Хелперы грантов при
/// отказе освобождают текстуру сами, поэтому здесь остаётся только вернуться.
///
/// ```ignore
/// state.surface = veldsdk::surface::delegate(
///     state.surface, width, height, scale, format, "globe", &["ui-service"],
///     crate::calls::globe::on_set_surface,
/// );
/// ```
pub fn delegate(
    previous: Option<Delegated>,
    width: u32,
    height: u32,
    scale_factor: f32,
    format: i32,
    renderer: &str,
    readers: &[&str],
    publish: impl FnOnce(&SurfaceDelegated),
) -> Option<Delegated> {
    let Some(texture_id) = abi::resource_alloc_texture(width, height, format, RENDER_TARGET_USAGE) else {
        crate::log::error!(target: "sdk", "[SURFACE] не выделилась текстура {}x{} для '{}'", width, height, renderer);
        return previous;
    };

    if let Err(error) = crate::resource::grant_write_or_free(texture_id, renderer) {
        crate::log::error!(target: "sdk", "[SURFACE] {}", error);
        return previous;
    }

    // Права раздаются до публикации: рендерер, узнав о текстуре, вправе сразу
    // в неё рисовать, а показывающий — сразу её показать.
    for reader in readers {
        if let Err(error) = crate::resource::grant_read_or_free(texture_id, reader) {
            crate::log::error!(target: "sdk", "[SURFACE] {}", error);
            return previous;
        }
    }

    // Во владение — только после всех грантов: до этого текстуру освобождает
    // сам отказавший хелпер, и владелец освободил бы её вторым.
    let delegated = Delegated {
        texture: crate::OwnedResource::new(ResourceHandle { id: texture_id, ..Default::default() }),
        width,
        height,
    };

    publish(&SurfaceDelegated {
        surface: Some(delegated.handle()),
        width,
        height,
        scale_factor,
        format,
    });

    // Прежняя доживает до этой строки и освобождается своим Drop: рендерер уже
    // знает о новой, и держать обе одновременно приходится ровно этот миг.
    drop(previous);
    Some(delegated)
}

/// Конец делегирования: место отзывается, текстура освобождается.
///
/// Освободить её молча — недостаточно. View, сделанный рендерером по этой
/// текстуре, держит её живой в обход реестра, так что рисовать он будет
/// исправно и дальше — в место, которого больше никто не видит, каждый кадр до
/// конца процесса. Отсюда порядок: сначала сказать, потом освободить.
///
/// Зовётся там, где место кончается вместе со своим хозяином: закрытая вкладка,
/// закрытое окно. Перевыделение под новый размер — не отзыв, там `delegate`
/// сам рассказывает про новую текстуру и освобождает прежнюю.
pub fn revoke(previous: Option<Delegated>, publish: impl FnOnce(&SurfaceDelegated)) {
    if previous.is_none() {
        return;
    }
    publish(&SurfaceDelegated { surface: None, ..Default::default() });
    drop(previous);
}
