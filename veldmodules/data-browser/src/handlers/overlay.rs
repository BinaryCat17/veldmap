//! Наложение снимка на шар: свести «какой продукт → какие файлы → показать».
//!
//! Ровно та же граница, что у контуров: провайдер знает раскладку продукта и
//! отдаёт роли растров с рамкой, глобус умеет натянуть открытые ресурсы на
//! привязку — а свести одно с другим может только тот, у кого и список, и
//! шар. Ресурсы открывает провайдер (подпись только у него), владение уходит
//! глобусу вместе с набором наложений.
//!
//! Наложение одно — выбранного продукта; новый показ заменяет прежний, и
//! глобус сам освобождает снятое (набор приходит целиком, см. его Overlays).

use crate::module::footprint;
use crate::module::state::{overlay::Assembly, overlay::OverlayState, State, ViewId};
use crate::proto::data_provider::{
    DataProduct, ImageryRequest, ImageryResponse, LocateRequest, LocateResponse,
};
use crate::proto::globe::{
    camera_command::Command, CameraCommand, Focus, GeoPoint, Overlay, OverlayRaster, Overlays,
    UtmFrame,
};
use veldsdk::proto::core::ResourceOpened;

/// «Показать на шаре» из меню строки — любой: у найденного продукт с контуром
/// уже под рукой, у строки каталога или загрузок есть только ключ, и продукт
/// по нему восстанавливает провайдер (см. его on_locate).
pub fn on_show_pressed(state: &mut State, identifier: String) {
    // Меню строки закрываем сами: показ уводит с этого экрана, а открытым оно
    // осталось бы до возвращения.
    state.close_menus();
    // Ключ папки приходит со слэшем листинга, продукт каталога — без; продукт
    // один и тот же.
    let identifier = identifier.trim_end_matches('/').to_string();
    if super::search::show(state, &identifier) {
        return;
    }
    let correlation = state.locates.begin();
    crate::calls::data_provider::on_locate(&LocateRequest { identifier }, &correlation);
}

/// Продукт восстановлен по ключу — показываем, как показывали бы из поиска,
/// только без выделения контура: выдачи, в которой его выделять, нет.
pub fn on_locate_result(state: &mut State, response: LocateResponse) {
    if state.locates.settle(&veldsdk::correlation()) != veldsdk::Reply::Current {
        return;
    }
    let Some(product) = response.product else {
        veldsdk::log::warn!(target: "handlers", "показать на шаре не вышло: {}", response.error);
        return;
    };

    if let Some(frame) = footprint::frame(&product.footprint) {
        crate::calls::globe::on_camera(&CameraCommand {
            command: Some(Command::Focus(Focus {
                at: Some(GeoPoint { lat: frame.lat, lon: frame.lon }),
                radius_deg: frame.radius_deg,
            })),
        });
    }
    // Подсветка прежнего выбранного контура гаснет: на шар ложится другой
    // снимок, и полоса глобуса должна назвать его, а не прошлый выбор.
    super::search::deselect(state);
    show(state, &product, None);
    super::nav::on_new_globe(state);
}

/// Показать продукт на шаре. Прежнее наложение снимается сразу: пользователь
/// уже смотрит на другой продукт, и старый снимок под новым контуром — ложь.
///
/// `source` — вид поиска, из выдачи которого продукт взят; для продукта,
/// восстановленного по ключу, его нет (см. `OverlayState::source`).
pub fn show(state: &mut State, product: &DataProduct, source: Option<ViewId>) {
    if state.overlay.as_ref().is_some_and(|overlay| overlay.identifier == product.identifier) {
        // Тот же продукт: наложение уже показано или в сборке, наводка камеры
        // своё дело сделала.
        return;
    }
    clear(state);

    let quad = quad_of(product);
    let mut overlay =
        OverlayState::new(product.identifier.clone(), product.name.clone(), source, quad);
    let correlation = overlay.imagery.begin();
    state.overlay = Some(overlay);

    crate::calls::data_provider::on_imagery(&ImageryRequest {
        identifier: product.identifier.clone(),
    }, &correlation);
}

/// Снять наложение с шара. Пустой набор — глобус освободит ресурсы.
pub fn clear(state: &mut State) {
    if state.overlay.take().is_some_and(|overlay| overlay.sent) {
        crate::calls::globe::on_overlay(&Overlays { overlays: Vec::new() });
    }
}

/// Наложение пережило новый поиск в своём виде, только если его продукт
/// остался в выдаче, — как выделение (см. search::show_on_globe). Чужая
/// выдача наложение не трогает: продукт из каталога или загрузок в ней и не
/// бывал.
pub fn keep_only(state: &mut State, view: ViewId, alive: impl Fn(&str) -> bool) {
    let ours = state.overlay.as_ref().is_some_and(|overlay| {
        overlay.source == Some(view) && !alive(&overlay.identifier)
    });
    if ours {
        clear(state);
    }
}

/// Вкладка поиска закрылась — наложение из её выдачи уходит вместе с ней.
/// Восстановленное по ключу живёт дальше: его источник не вкладка.
pub fn source_closed(state: &mut State, view: ViewId) {
    if state.overlay.as_ref().is_some_and(|overlay| overlay.source == Some(view)) {
        clear(state);
    }
}

/// Ответ провайдера: роли растров и рамка. Открываем каждый растр ресурсом.
pub fn on_imagery_result(state: &mut State, response: ImageryResponse) {
    let correlation = veldsdk::correlation();
    let Some(overlay) = &mut state.overlay else { return };
    if overlay.imagery.settle(&correlation) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "растры '{}': {}", overlay.label, response.error);
        state.overlay = None;
        return;
    }
    if response.rasters.is_empty() {
        veldsdk::log::info!(target: "handlers", "у '{}' нет растров для наложения", overlay.label);
        state.overlay = None;
        return;
    }
    // Привязки нет ни рамкой, ни квадом — снимку негде лежать; честнее не
    // открывать ресурсы, чем дать глобусу отказаться от готового.
    if response.utm.is_none() && overlay.quad.is_none() {
        veldsdk::log::warn!(target: "handlers", "'{}' без привязки: контур не четырёхугольник и рамки нет", overlay.label);
        state.overlay = None;
        return;
    }

    overlay.assembly = Some(Assembly {
        utm: response.utm,
        pending: response.rasters.len(),
        collected: Vec::new(),
    });
    for raster in response.rasters {
        let correlation = state.overlay_opens.begin(raster.role);
        crate::calls::data_provider::on_open(&crate::proto::data_provider::OpenRequest {
            identifier: raster.identifier,
        }, &correlation);
    }
}

/// Открытие растра наложения. `false` — ответ не наш.
pub fn on_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation = veldsdk::correlation();
    let Some(role) = state.overlay_opens.take(&correlation) else { return false };

    // Наложение успели сменить или снять: ресурс наш, но класть его некуда.
    let Some(assembly) = state.overlay.as_mut().and_then(|o| o.assembly.as_mut()) else {
        if let Some(handle) = &opened.handle {
            veldsdk::resource::release(handle.clone());
        }
        return true;
    };

    assembly.pending -= 1;
    match veldsdk::resource::accept(opened) {
        Ok(handle) => assembly.collected.push((role, handle)),
        // Роль пропускается: наложение живёт тем, что открылось.
        Err(error) => veldsdk::log::warn!(target: "handlers", "растр наложения: {}", error),
    }
    if assembly.pending == 0 {
        send(state);
    }
    true
}

/// Все открытия кончились — передать владение глобусу и отправить набор.
fn send(state: &mut State) {
    let Some(overlay) = &mut state.overlay else { return };
    let Some(assembly) = overlay.assembly.take() else { return };

    if assembly.collected.is_empty() {
        veldsdk::log::warn!(target: "handlers", "'{}': ни один растр не открылся", overlay.label);
        state.overlay = None;
        return;
    }

    let mut rasters = Vec::new();
    for (role, handle) in assembly.collected {
        // Передача владения — до сообщения: получив набор, глобус вправе
        // сразу считать ресурсы своими. При отказе хелпер освободил его сам.
        match veldsdk::resource::hand_off(handle, "globe") {
            Ok(handle) => rasters.push(OverlayRaster { resource: Some(handle), role }),
            Err(error) => {
                veldsdk::log::warn!(target: "handlers", "растр не передался глобусу: {}", error)
            }
        }
    }
    if rasters.is_empty() {
        state.overlay = None;
        return;
    }

    let utm = assembly.utm.map(|utm| UtmFrame {
        zone: utm.zone,
        south: utm.south,
        x0: utm.x0,
        y0: utm.y0,
        x1: utm.x1,
        y1: utm.y1,
    });
    let quad = match utm.is_none() {
        // Квад — запасная привязка; при рамке он глобусу не нужен.
        true => overlay
            .quad
            .map(|points| points.map(|(lat, lon)| GeoPoint { lat, lon }).to_vec())
            .unwrap_or_default(),
        false => Vec::new(),
    };

    crate::calls::globe::on_overlay(&Overlays {
        overlays: vec![Overlay {
            key: overlay.identifier.clone(),
            label: overlay.label.clone(),
            utm,
            quad,
            rasters,
        }],
    });
    overlay.sent = true;
}

/// Четырёхугольник футпринта: первое кольцо ровно из четырёх вершин (замыкающий
/// дубль не в счёт). Сложный контур квадом не является — рамка тогда
/// обязательна.
fn quad_of(product: &DataProduct) -> Option<[(f64, f64); 4]> {
    let ring = product.footprint.first()?;
    let points = match ring.points.as_slice() {
        [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
            &ring.points[..ring.points.len() - 1]
        }
        all => all,
    };
    match points {
        [a, b, c, d] => Some([(a.lat, a.lon), (b.lat, b.lon), (c.lat, c.lon), (d.lat, d.lon)]),
        _ => None,
    }
}
