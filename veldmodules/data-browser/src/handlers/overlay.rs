//! Наложения на шар: свести «какой продукт → какие файлы → показать».
//!
//! Ровно та же граница, что у контуров: провайдер знает раскладку продукта и
//! отдаёт роли растров с рамкой, глобус умеет натянуть открытые ресурсы на
//! привязку — а свести одно с другим может только тот, у кого и список, и
//! шар. Ресурсы открывает провайдер (подпись только у него), владение уходит
//! глобусу вместе с набором наложений.
//!
//! Наложений много, и глобусу они уезжают набором целиком — не потому, что так
//! дешевле, а потому, что таков его контракт: чего не прислали, того больше
//! нет. Отсюда единственное правило этого модуля: **всякое изменение набора
//! кончается отправкой набора** ([`send_set`]). Дописать одно наложение или
//! погасить одно нечем, и заводить для этого второй способ значило бы держать
//! набор в двух местах сразу.
//!
//! Уже отправленные растры пересылаются теми же дескрипторами: владения за
//! ними нет, это имена, по которым глобус узнаёт прежнее наложение и не
//! переоткрывает его.

use crate::module::footprint;
use crate::module::state::{overlay::Assembly, overlay::OverlayState, Shift, State, ViewId};
use crate::proto::data_provider::{
    DataProduct, ImageryRequest, ImageryResponse, LocateRequest, LocateResponse,
};
use crate::proto::globe::{
    camera_command::Command, CameraCommand, Focus, GeoPoint, Overlay, OverlayRaster, OverlayRole,
    Overlays, UtmFrame,
};
use veldsdk::proto::core::ResourceOpened;

/// Роль растра у провайдера и роль растра у глобуса — два разных перечисления
/// в двух разных контрактах, и связаны они здесь, а не совпадением чисел.
///
/// Совпадение — не связь: оно держится ровно до того дня, когда одному из
/// перечислений добавят вариант в середину, и тогда подробный растр поедет
/// превью, а компилятор об этом не скажет. Match'ем скажет.
fn role_for_globe(role: crate::proto::data_provider::ImageryRole) -> OverlayRole {
    use crate::proto::data_provider::ImageryRole;
    match role {
        ImageryRole::ImageryPreview => OverlayRole::OverlayPreview,
        ImageryRole::ImageryDetailed => OverlayRole::OverlayDetailed,
    }
}

/// «На глобус» из строки списка — любой: у найденного продукт с контуром уже
/// под рукой, у строки каталога или загрузок есть только ключ, и продукт по
/// нему восстанавливает провайдер (см. его on_locate).
pub fn on_show_pressed(state: &mut State, view: ViewId, identifier: String) {
    // Меню строки закрываем сами: показ уводит с этого экрана, а открытым оно
    // осталось бы до возвращения.
    state.close_menus();
    // Ключ папки приходит со слэшем листинга, продукт каталога — без; продукт
    // один и тот же.
    let identifier = identifier.trim_end_matches('/').to_string();
    // Сперва выдача: там у продукта есть контур, и щелчок по строке обязан его
    // выделить — даже если снимок на шаре уже лежит.
    if super::search::show(state, view, &identifier) {
        return;
    }
    // Уже на шаре, но не из выдачи — значит просят посмотреть на него, а не
    // положить туда ещё раз. Спрашивать ради этого каталог нечего: куда
    // смотреть, посчитано в момент показа.
    if focus(state, &identifier) {
        return;
    }
    let correlation = state.locates.begin();
    crate::calls::data_provider::on_locate(&LocateRequest { identifier }, &correlation);
}

/// Навести шар на слой и показать вкладку с ним. `false` — такого слоя нет.
///
/// Скрытый при этом возвращается на шар: «на глобус» просят у того, чего не
/// видно, и молча навести камеру на пустое место — это ответить не на тот
/// вопрос.
pub fn focus(state: &mut State, key: &str) -> bool {
    if !state.overlays.iter().any(|overlay| overlay.identifier == key) {
        return false;
    }
    set_hidden(state, key, false);
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else {
        return false;
    };
    if let Some(frame) = &overlay.focus {
        crate::calls::globe::on_camera(&CameraCommand {
            command: Some(Command::Focus(Focus {
                at: Some(GeoPoint { lat: frame.lat, lon: frame.lon }),
                radius_deg: frame.radius_deg,
            })),
        });
    }
    // Подсветка прежнего выбора гаснет — по той же причине, что и при показе
    // по ключу: полоса глобуса называет то, на что смотрят, а смотрят теперь
    // на этот слой.
    super::search::deselect(state);
    super::nav::on_new_globe(state);
    true
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
    // Подсветка прежнего выбранного контура гаснет: на шар лёг другой снимок,
    // и полоса глобуса должна назвать его, а не прошлый выбор.
    super::search::deselect(state);
    show(state, &product, None);
    super::nav::on_new_globe(state);
}

/// Положить продукт на шар. Уже лежащий не кладётся заново: наводка камеры
/// своё дело сделала, а пересборка стоила бы переоткрытия растров.
///
/// Новый слой встаёт поверх прежних — концом набора, потому что его и просили
/// показать.
///
/// `source` — вид поиска, из выдачи которого продукт взят; для продукта,
/// восстановленного по ключу, его нет (см. `OverlayState::source`).
pub fn show(state: &mut State, product: &DataProduct, source: Option<ViewId>) {
    if state.overlays.iter().any(|overlay| overlay.identifier == product.identifier) {
        return;
    }

    state.overlays.push(OverlayState::new(
        product.identifier.clone(),
        product.name.clone(),
        source,
        quad_of(product),
        footprint::frame(&product.footprint),
    ));

    let correlation = state.imageries.begin(product.identifier.clone());
    if let Some(overlay) = state.overlays.last_mut() {
        overlay.imagery = Some(correlation.clone());
    }
    crate::calls::data_provider::on_imagery(&ImageryRequest {
        identifier: product.identifier.clone(),
    }, &correlation);
}

/// Убрать одно наложение: ресурсы отпустить, набор переслать.
pub fn remove(state: &mut State, key: &str) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let overlay = state.overlays.remove(index);
    abandon(state, overlay);
    send_set(state);
}

/// Подвинуть слой в наборе на одно место. Порядок набора — он же порядок слоёв
/// на шаре снизу вверх, поэтому вся перестановка здесь и кончается: глобус
/// получит набор целиком и разложит его в присланном порядке.
///
/// Направление здесь — про набор, а не про экран: список «На просмотре»
/// перевёрнут (сверху новые), и переворот живёт у него (см. `view::shown`).
/// Крайний слой не двигается и молча — упереться в край не ошибка.
pub fn shift(state: &mut State, key: &str, shift: Shift) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let target = match shift {
        Shift::Up if index + 1 < state.overlays.len() => index + 1,
        Shift::Down if index > 0 => index - 1,
        _ => return,
    };
    state.overlays.swap(index, target);
    send_set(state);
}

/// Снять с шара всё.
pub fn clear_all(state: &mut State) {
    for overlay in std::mem::take(&mut state.overlays) {
        abandon(state, overlay);
    }
    send_set(state);
}

/// Конец одного наложения. Отправленные растры освобождает глобус — он их
/// владелец, и следующий набор без этого ключа их и освободит; наши остаются
/// только у сборки, оборванной на середине.
fn abandon(state: &mut State, overlay: OverlayState) {
    // Запрос растров снимается с учёта здесь же, что и открытия: без этого его
    // ответ опознался бы слоем, положенным под тем же ключом заново, и завёл бы
    // ему вторую сборку поверх первой.
    if let Some(correlation) = &overlay.imagery {
        state.imageries.take(correlation);
    }
    let Some(assembly) = overlay.assembly else { return };
    // Открытия в полёте снимаются с общего учёта здесь: иначе их ответ
    // опознался бы следующей сборкой того же ключа, а его ресурс лёг бы в
    // чужой набор. Снятый доедет до общего discard в module::on_open_result.
    for correlation in &assembly.opens {
        state.opens.take(correlation);
    }
    for (_, handle) in assembly.collected {
        veldsdk::resource::release(handle);
    }
}

/// Наложение переживает новый поиск в своём виде, только если его продукт
/// остался в выдаче, — как выделение (см. search::show_on_globe). Чужая
/// выдача наложений не трогает: продукт из каталога или загрузок в ней и не
/// бывал.
pub fn keep_only(state: &mut State, view: ViewId, alive: impl Fn(&str) -> bool) {
    retain(state, |overlay| {
        overlay.source != Some(view) || alive(&overlay.identifier)
    });
}

/// Вкладка поиска закрылась — наложения из её выдачи уходят вместе с ней.
/// Восстановленные по ключу живут дальше: их источник не вкладка.
pub fn source_closed(state: &mut State, view: ViewId) {
    retain(state, |overlay| overlay.source != Some(view));
}

/// Оставить те, что прошли условие, и переслать набор — если что-то ушло.
/// Пересылка при пустом отсеве была бы лишним сообщением на каждый поиск.
fn retain(state: &mut State, keep: impl Fn(&OverlayState) -> bool) {
    let (kept, gone): (Vec<_>, Vec<_>) =
        std::mem::take(&mut state.overlays).into_iter().partition(&keep);
    state.overlays = kept;
    if gone.is_empty() {
        return;
    }
    for overlay in gone {
        abandon(state, overlay);
    }
    send_set(state);
}

// ── Показ ──────────────────────────────────────────────────────

/// Прозрачность слоя. Набор пересылается целиком, но растры в нём — прежние
/// дескрипторы, поэтому движение ползунка ничего не переоткрывает.
pub fn set_opacity(state: &mut State, key: &str, opacity: f32) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    let opacity = opacity.clamp(0.0, 1.0);
    if overlay.opacity == opacity {
        return;
    }
    overlay.opacity = opacity;
    send_set(state);
}

/// Скрыть или показать слой.
pub fn set_hidden(state: &mut State, key: &str, hidden: bool) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    if overlay.hidden == hidden {
        return;
    }
    overlay.hidden = hidden;
    send_set(state);
}

/// Скрыть или показать все сразу — кнопка «Скрыть все» в списке.
pub fn hide_all(state: &mut State, hidden: bool) {
    let mut changed = false;
    for overlay in &mut state.overlays {
        changed |= overlay.hidden != hidden;
        overlay.hidden = hidden;
    }
    if changed {
        send_set(state);
    }
}

// ── Сборка ─────────────────────────────────────────────────────

/// Ответ провайдера: роли растров и рамка. Открываем каждый растр ресурсом.
pub fn on_imagery_result(state: &mut State, response: ImageryResponse) {
    let Some(key) = state.imageries.take(&veldsdk::correlation()) else { return };
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { return };
    let label = overlay.label.clone();
    // Сборка уже идёт — значит это ответ на запрос, который сняли с учёта не
    // до конца. Заводить вторую поверх первой нельзя: её открытия остались бы
    // без хозяина, а их ресурсы легли бы в чужой набор.
    if overlay.assembly.is_some() {
        veldsdk::log::warn!(target: "handlers", "'{}': растры уже собираются", label);
        return;
    }

    // Отказаться от наложения — значит убрать его, тем же путём, что и по
    // кнопке: пустая строка, которая никогда ничего не покажет, хуже её
    // отсутствия. Именно `remove`, а не выкидывание из списка: слой мог уже
    // лежать на шаре, и тогда его нужно ещё и снять оттуда.
    let give_up = |state: &mut State, why: String| {
        veldsdk::log::warn!(target: "handlers", "'{}': {}", label, why);
        remove(state, &key);
    };

    if !response.error.is_empty() {
        return give_up(state, format!("растры не спросились: {}", response.error));
    }
    if response.rasters.is_empty() {
        return give_up(state, "нет растров для наложения".to_string());
    }
    // Привязки нет ни рамкой, ни квадом — снимку негде лежать; честнее не
    // открывать ресурсы, чем дать глобусу отказаться от готового.
    if response.utm.is_none() && overlay.quad.is_none() {
        return give_up(state, "без привязки: контур не четырёхугольник и рамки нет".to_string());
    }

    let utm = response.utm.map(|utm| UtmFrame {
        zone: utm.zone,
        south: utm.south,
        x0: utm.x0,
        y0: utm.y0,
        x1: utm.x1,
        y1: utm.y1,
    });
    let mut opens = Vec::new();
    for raster in response.rasters {
        // Роль переводится сразу, на границе: дальше по нашему коду ездит уже
        // та, которую поймёт глобус.
        let correlation = state.opens.begin((key.clone(), role_for_globe(raster.role())));
        opens.push(correlation.clone());
        crate::calls::data_provider::on_open(&crate::proto::data_provider::OpenRequest {
            identifier: raster.identifier,
        }, &correlation);
    }

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    // Запрос растров кончился — его корреляция больше не наша.
    overlay.imagery = None;
    overlay.assembly = Some(Assembly { utm, opens, collected: Vec::new() });
}

/// Открытие растра наложения. `false` — ответ не наш: чужая корреляция либо
/// сборка, которую уже оборвали (её открытия сняты с учёта в `abandon`), и
/// приехавший ресурс добьёт общий discard (см. module::on_open_result).
pub fn on_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation = veldsdk::correlation();
    let Some((key, role)) = state.opens.take(&correlation) else { return false };

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else {
        // Наложение убрали между запросом и ответом: ресурс наш, и отпустить
        // его больше некому.
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };
    // Сборки нет — она кончилась (или её оборвали) раньше, чем доехал этот
    // ответ. Ресурс всё равно наш: принять и отпустить его больше некому.
    let Some(assembly) = overlay.assembly.as_mut() else {
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };

    assembly.opens.retain(|waiting| waiting != &correlation);
    match veldsdk::resource::accept(opened) {
        Ok(handle) => assembly.collected.push((role, handle)),
        // Роль пропускается: наложение живёт тем, что открылось.
        Err(error) => veldsdk::log::warn!(target: "handlers", "растр наложения: {}", error),
    }
    if assembly.opens.is_empty() {
        finish(state, &key);
    }
    true
}

/// Все открытия этого наложения кончились — передать владение глобусу и
/// переслать набор.
fn finish(state: &mut State, key: &str) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    let Some(assembly) = overlay.assembly.take() else { return };
    let label = overlay.label.clone();

    let mut rasters = Vec::new();
    for (role, handle) in assembly.collected {
        // Передача владения — до сообщения: получив набор, глобус вправе сразу
        // считать ресурсы своими. При отказе хелпер освободил его сам.
        match veldsdk::resource::hand_off(handle, "globe") {
            Ok(handle) => {
                rasters.push(OverlayRaster { resource: Some(handle), role: role as i32 })
            }
            Err(error) => {
                veldsdk::log::warn!(target: "handlers", "растр не передался глобусу: {}", error)
            }
        }
    }
    if rasters.is_empty() {
        veldsdk::log::warn!(target: "handlers", "'{}': ни один растр не открылся", label);
        state.overlays.retain(|overlay| overlay.identifier != key);
        return;
    }

    overlay.utm = assembly.utm;
    overlay.rasters = rasters;
    send_set(state);
}

/// Отправить глобусу весь набор. Единственный способ что-либо ему сказать про
/// наложения — отсюда и одна точка вызова на каждое изменение.
///
/// Собирающиеся в набор не попадают: наложение без растров глобус принять не
/// может, и слать его значило бы просить снять то, чего он ещё не видел.
fn send_set(state: &State) {
    let overlays = state
        .overlays
        .iter()
        .filter(|overlay| overlay.on_globe())
        .map(|overlay| Overlay {
            key: overlay.identifier.clone(),
            label: overlay.label.clone(),
            utm: overlay.utm.clone(),
            // Квад — запасная привязка; при рамке он глобусу не нужен.
            quad: match overlay.utm.is_none() {
                true => overlay
                    .quad
                    .map(|points| points.map(|(lat, lon)| GeoPoint { lat, lon }).to_vec())
                    .unwrap_or_default(),
                false => Vec::new(),
            },
            rasters: overlay.rasters.clone(),
            opacity: Some(overlay.opacity),
            hidden: overlay.hidden,
        })
        .collect();

    crate::calls::globe::on_overlay(&Overlays { overlays });
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
