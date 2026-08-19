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
//! Спрашивать перед отправкой «а изменился ли набор» при этом не нужно: топик
//! объявлен снимком, и совпавший с прошлым до шины не доходит. Правило поэтому
//! и звучит без оговорок — отправляют всегда.
//!
//! Уже отправленные растры пересылаются теми же дескрипторами: владения за
//! ними нет, это имена, по которым глобус узнаёт прежнее наложение и не
//! переоткрывает его.

use crate::module::footprint;
use crate::module::state::{
    overlay::Assembly, overlay::OverlayState, overlay::Part, overlay::Progress, Locate, Located,
    Open, Shift,
    State, ViewId,
};
use crate::proto::data_provider::{
    DataProduct, ImageryRequest, ImageryResponse, LocateRequest, LocateResponse,
};
use crate::proto::globe::{
    GeoPoint, Overlay, OverlayRaster, OverlayRole, Overlays, UtmFrame,
};
use veldsdk::proto::core::{ResourceHandle, ResourceOpened};

/// Начиная с какого размаха долгот контур перестаёт быть четырёхугольником и
/// становится поясом вокруг Земли. Полоса съёмки такой ширины не бывает: самая
/// широкая гранула — четыреста километров, а виток пересекает меридианы,
/// оставаясь полосой, и в габарит по долготе укладывается с большим запасом.
const WHOLE_EARTH_DEG: f64 = 350.0;

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
///
/// Ключ приходит уже без завершающего слэша листинга: приводит его к виду
/// продукта сама строка (см. `Row::snapshot_key`), и второе такое правило
/// разошлось бы с первым.
///
/// Показ выбора не ставит: это разные намерения. Выбирают, чтобы что-то с
/// набранным сделать — очертить, удалить, скачать, — а показывают, чтобы
/// посмотреть, и живёт показанное своим списком («На просмотре»). Ставя заодно
/// коробочку, показ клал бы снимок в состав следующего пакетного действия,
/// которого никто не просил.
pub fn on_show_pressed(state: &mut State, view: ViewId, identifier: String) {
    // Меню строки закрываем сами: показ уводит с этого экрана, а открытым оно
    // осталось бы до возвращения.
    state.close_menus();
    // Сперва выдача: там у продукта есть всё, что нужно, и ходить за ним в
    // каталог второй раз незачем.
    if super::search::show(state, view, &identifier) {
        super::outline::focus(state, &identifier);
        return;
    }
    // Уже на шаре, но не из выдачи — значит просят посмотреть на него, а не
    // положить туда ещё раз. Спрашивать ради этого каталог нечего: куда
    // смотреть, посчитано в момент показа.
    if focus(state, &identifier) {
        return;
    }
    // Продукт придётся восстанавливать у провайдера — и один ход отвечает
    // обоим: и наложению, и контуру. Ответ помечается ожидаемым здесь же, иначе
    // сборка контуров послала бы за тем же продуктом второй запрос. Известное
    // при этом не затирается: у очерченного геометрия уже лежит, и стёртая
    // означала бы пропавший на кадр контур.
    if !matches!(state.located.get(&identifier), Some(Located::Found(_))) {
        state.located.insert(identifier.clone(), Located::Asking);
    }
    state.showing.insert(identifier.clone());
    let correlation = state.locates.begin(Locate::Overlay(identifier.clone()));
    crate::calls::data_provider::on_locate(&LocateRequest { identifier }, &correlation);
}

/// Вернуть слой на шар, навести на него камеру и выделить. `false` — такого
/// слоя нет.
///
/// Скрытый при этом возвращается на шар: значок глобуса в строке списка просят
/// у того, чего не видно, и молча навести камеру на пустое место — это ответить
/// не на тот вопрос. В списке слоёв кнопка другая (`Msg::OutlineFocus`): там
/// показом заведует соседний глаз, и трогать его за спиной нечему.
pub fn focus(state: &mut State, key: &str) -> bool {
    if !state.overlays.iter().any(|overlay| overlay.identifier == key) {
        return false;
    }
    set_hidden(state, key, false);
    super::outline::focus(state, key);
    true
}

/// Продукт восстановлен по ключу — показываем, как показывали бы из поиска.
///
/// Ключ продукта может оказаться не тем, которым его позвали: спросили файл
/// внутри снимка, а каталог отвечает корнем продукта (см. `on_locate` у
/// провайдера). Наложение ложится под ключом продукта — иначе глобус получил
/// бы два ключа на один снимок.
pub fn on_located(state: &mut State, key: &str, response: LocateResponse) {
    // Пока ход к каталогу шёл, показа могли и расхотеть — «снять с шара» за
    // эти секунды нажать успевают. Убить запрос в полёте нечем, он ответит
    // всё равно, поэтому спрашиваем здесь, всё ли ещё его ждут. Иначе
    // приложение через несколько секунд после отмены само прыгает на вкладку
    // глобуса, уводит камеру и кладёт снимок, которого не просили.
    //
    // Закрытая вкладка показа не отменяет: слой лежит в состоянии модуля и
    // живёт своим списком, а не тем, из которого его позвали.
    if !state.showing.remove(key) {
        veldsdk::log::info!(target: "handlers", "показ '{}' расхотели, пока шёл ответ", key);
        return;
    }
    let Some(product) = response.product else {
        veldsdk::log::warn!(target: "handlers", "показать на шаре не вышло: {}", response.error);
        state.notice = Some(format!("Показать на шаре не вышло: {}", response.error));
        return;
    };

    // Ответ пришёл под другим ключом, и спрошенное под этим больше ничего не
    // значит: следующий, кому понадобится геометрия файла, спросит заново.
    // Кроме случая, когда этот ключ очерчен: там ответ ждут, и забытый он
    // уехал бы в каталог второй раз за тем же самым.
    if key != product.identifier && !state.outlines.contains(key) {
        state.located.remove(key);
    }
    // Продукт кладётся в кэш под своим ключом — тем, которым он теперь и
    // адресуется: без этого сборка контуров сочла бы его неспрошенным и
    // послала бы в каталог второй запрос за тем же самым.
    state.located.insert(product.identifier.clone(), Located::Found(product.clone()));
    show(state, &product, None);
    super::outline::focus(state, &product.identifier);
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
        product.folder,
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
    // И то, что ещё едет: ответ каталога придёт всё равно, а положить снимок
    // после «снять с шара» значило бы вернуть на шар то, что с него убрали.
    state.showing.clear();
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
        // Корреляция остаётся на учёте, но с пустым ключом: наложения с таким
        // ключом нет, и приехавший позже ответ пройдёт по ветке «наложение
        // убрали» — ресурс в нём наш, там его примут и отпустят. Снять с учёта
        // совсем нельзя: неопознанный ответ уходит в `discard`, а тот ресурса
        // не трогает — файл остался бы открытым до конца сеанса.
        if let Some((_, role, part)) = state.opens.take(correlation) {
            state.opens.insert(correlation.clone(), (String::new(), role, part));
        }
    }
    for (_, _, handle) in assembly.collected {
        veldsdk::resource::release(handle);
    }
}

/// Наложение переживает новый поиск в своём виде, только если его продукт
/// остался в выдаче, — тем же правилом, что и отметка (см.
/// `search::survivors`). Чужая выдача наложений не трогает: продукт из
/// каталога или загрузок в ней и не бывал.
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

/// Оставить те, что прошли условие, и переслать набор.
///
/// О снятом говорится вслух: убирает его не человек, а правило («наложение
/// живёт жизнью своей выдачи»), и снаружи молчаливое исчезновение снимка с
/// шара выглядит как то, что он выгрузился сам собой.
fn retain(state: &mut State, keep: impl Fn(&OverlayState) -> bool) {
    let (kept, gone): (Vec<_>, Vec<_>) =
        std::mem::take(&mut state.overlays).into_iter().partition(&keep);
    state.overlays = kept;
    let said = match gone.as_slice() {
        [] => None,
        [one] => Some(format!(
            "«{}» снят с шара: его больше нет в выдаче",
            crate::module::components::format::ellipsize(&one.label, 40)
        )),
        many => Some(format!(
            "{} {} снято с шара: их больше нет в выдаче",
            many.len(),
            crate::module::components::format::plural(
                many.len(),
                ["снимок", "снимка", "снимков"]
            )
        )),
    };
    for overlay in gone {
        abandon(state, overlay);
    }
    if let Some(said) = said {
        state.notice = Some(said);
    }
    send_set(state);
}

// ── Показ ──────────────────────────────────────────────────────

/// Прозрачность слоя. Набор пересылается целиком, но растры в нём — прежние
/// дескрипторы, поэтому движение ползунка ничего не переоткрывает.
pub fn set_opacity(state: &mut State, key: &str, opacity: f32) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.opacity = opacity.clamp(0.0, 1.0);
    send_set(state);
}

/// Скрыть или показать слой.
pub fn set_hidden(state: &mut State, key: &str, hidden: bool) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.hidden = hidden;
    send_set(state);
}

/// Меню слоя: раскрыть или закрыть раскрытое.
pub fn menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::Layer(key)),
        None => state.close_menus(),
    }
}

/// Скрыть или показать все сразу — кнопка «Скрыть все» в списке.
pub fn hide_all(state: &mut State, hidden: bool) {
    for overlay in &mut state.overlays {
        overlay.hidden = hidden;
    }
    send_set(state);
}

// ── Сборка ─────────────────────────────────────────────────────

/// Отказаться от наложения — значит убрать его, тем же путём, что и по кнопке:
/// пустая строка, которая никогда ничего не покажет, хуже её отсутствия. Именно
/// `remove`, а не выкидывание из списка: слой мог уже лежать на шаре, и тогда
/// его нужно ещё и снять оттуда.
///
/// Причина уходит и на экран: нажали «на глобус», а на шаре не появилось
/// ничего — и без извещения это выглядит как несработавшая кнопка. Одной
/// функцией на все отказы сборки, потому что снаружи они неразличимы: слой
/// пропал, и человек вправе узнать почему.
fn give_up(state: &mut State, key: &str, label: &str, why: String) {
    veldsdk::log::warn!(target: "handlers", "'{}': {}", label, why);
    state.notice =
        Some(format!("«{}»: {}", crate::module::components::format::ellipsize(label, 40), why));
    remove(state, key);
}

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

    if !response.error.is_empty() {
        return give_up(state, &key, &label, format!("растры не спросились: {}", response.error));
    }
    if response.rasters.is_empty() {
        // Причину называет провайдер — он один видел, что в продукте лежит.
        // Своя фраза остаётся на случай, когда он промолчал: пустая строка на
        // экране хуже общей.
        let why = match response.reason.is_empty() {
            true => "нет растров для наложения".to_string(),
            false => response.reason,
        };
        return give_up(state, &key, &label, why);
    }
    // Геометрии нет вовсе — ни рамки, ни контура: снимку негде лежать даже
    // приблизительно, и открывать растры незачем. Сложный контур сюда не
    // относится: привязку принесёт сам растр, а до него о ней не узнать
    // (см. [`quad_of`]).
    if response.utm.is_none() && overlay.quad.is_none() {
        return give_up(state, &key, &label, "без привязки: у продукта нет геометрии".to_string());
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
        let role = role_for_globe(raster.role());
        // Координаты открываются тем же порядком и наравне с растром: у
        // Sentinel-3 они лежат соседним файлом, и без них полосе съёмки негде
        // лечь. Пусто — координаты в самом растре либо их нет вовсе.
        let files = [(Part::Raster, raster.identifier)]
            .into_iter()
            .chain((!raster.geolocation.is_empty()).then_some((
                Part::Geolocation,
                raster.geolocation,
            )));
        for (part, identifier) in files {
            // Чем открывать, решается по файлу, а не по снимку: у продукта
            // скачана бывает часть, и лежащий рядом квиклук не делает локальным
            // измерительный растр. Правило общее с просмотром строки — оно в
            // `LibraryState::local_name`.
            let local = state.library.local_name(&identifier).map(str::to_string);
            // Роль переводится сразу, на границе: дальше по нашему коду ездит
            // уже та, которую поймёт глобус.
            let correlation = state.opens.begin((key.clone(), role, part));
            opens.push(correlation.clone());
            match local {
                Some(name) => crate::calls::data_library::on_open(
                    &crate::proto::data_library::OpenRequest { name },
                    &correlation,
                ),
                None => crate::calls::data_provider::on_open(
                    &crate::proto::data_provider::OpenRequest { identifier },
                    &correlation,
                ),
            }
        }
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
    let Some((key, role, part)) = state.opens.take(&correlation) else { return false };

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
        Ok(handle) => assembly.collected.push((role, part, handle)),
        // Не открылось — наложение живёт тем, что открылось: без растра нет
        // роли, без координат нет привязки, и оба исхода объясняются ниже.
        Err(error) => veldsdk::log::warn!(target: "handlers", "файл наложения: {}", error),
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

    // Пары «растр и его координаты» складываются ДО передачи владения.
    // Порядок здесь не вкусовой: отпустить лишнее можно, только пока оно наше,
    // а после `hand_off` ресурс принадлежит глобусу — освобождение отклонит
    // хост, и о брошенном файле вспомнить будет некому.
    let mut pairs: Vec<(OverlayRole, ResourceHandle, Option<ResourceHandle>)> = Vec::new();
    let mut coordinates: Vec<(OverlayRole, ResourceHandle)> = Vec::new();
    for (role, part, handle) in assembly.collected {
        match part {
            Part::Raster => pairs.push((role, handle, None)),
            Part::Geolocation => coordinates.push((role, handle)),
        }
    }
    for (role, handle) in coordinates {
        match pairs.iter_mut().find(|(other, ..)| *other == role) {
            Some(pair) => pair.2 = Some(handle),
            // Координаты сами по себе наложению не файл, а свойство растра:
            // растр не открылся — показывать по ним нечего.
            None => {
                veldsdk::log::warn!(target: "handlers",
                    "'{}': координаты без растра — отпускаем", label);
                veldsdk::resource::release(handle);
            }
        }
    }

    // Передача владения — до сообщения: получив набор, глобус вправе сразу
    // считать ресурсы своими. При отказе хелпер освободил ресурс сам.
    let mut rasters = Vec::new();
    for (role, raster, coordinates) in pairs {
        let raster = match veldsdk::resource::hand_off(raster, "globe") {
            Ok(handle) => handle,
            Err(error) => {
                veldsdk::log::warn!(target: "handlers", "растр не передался глобусу: {}", error);
                // Координаты этого растра больше никому не нужны, и они пока
                // наши — отпускаем, пока есть чем.
                if let Some(handle) = coordinates {
                    veldsdk::resource::release(handle);
                }
                continue;
            }
        };
        let coordinates = coordinates.and_then(|handle| {
            match veldsdk::resource::hand_off(handle, "globe") {
                Ok(handle) => Some(handle),
                // Растр уже у глобуса и ляжет по контуру каталога: привязки
                // из файла у него не будет, но слой состоится.
                Err(error) => {
                    veldsdk::log::warn!(target: "handlers",
                        "координаты не передались глобусу: {}", error);
                    None
                }
            }
        });
        rasters.push(OverlayRaster {
            resource: Some(raster),
            role: role as i32,
            geolocation: coordinates,
        });
    }
    if rasters.is_empty() {
        // Тем же путём и с тем же словом, что и прочие отказы сборки (см.
        // `give_up`): пустая строка, которая никогда ничего не покажет, хуже
        // её отсутствия, а нажали-то на значок — и на шаре не появилось ничего.
        return give_up(state, &key, &label, "ни один растр не открылся".to_string());
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
            rough: overlay.rough,
            rasters: overlay.rasters.clone(),
            opacity: Some(overlay.opacity),
            hidden: overlay.hidden,
        })
        .collect();

    crate::calls::globe::on_overlay(&Overlays { overlays });
}

/// Ход добычи тайлов от глобуса: набор целиком, тем же правилом, что и сам
/// набор наложений, — о чьём ключе не сказано, у того ничего и не добывают.
///
/// Ничего, кроме записи в состояние: числа приезжают готовыми, а показывает их
/// список слоёв и строка того списка, из которого снимок родом.
pub fn on_overlay_progress(state: &mut State, msg: crate::proto::globe::OverlaysProgress) {
    for overlay in &mut state.overlays {
        let said = msg.overlays.iter().find(|progress| progress.key == overlay.identifier);
        overlay.trouble = said.map_or(String::new(), |progress| progress.trouble.clone());
        overlay.progress = said.map_or(Progress::default(), |progress| Progress {
            ready: progress.ready,
            total: progress.total,
            share: progress.share,
            working: progress.working,
            step: progress.step,
            steps: progress.steps,
        });
    }

    // Слой, которому нечем лечь, убираем тем же путём и с тем же словом, что и
    // отказы сборки: набор наложений наш, и снять с него неложащийся слой может
    // только тот, кто его туда положил. Заметить это может только глобус — у
    // него растры и привязка, — а «пустая строка, которая никогда ничего не
    // покажет, хуже её отсутствия» одинаково верно с обеих сторон.
    let doomed: Vec<(String, String)> = msg
        .overlays
        .iter()
        .filter(|progress| !progress.error.is_empty())
        .map(|progress| (progress.key.clone(), progress.error.clone()))
        .collect();
    for (key, why) in doomed {
        let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { continue };
        let label = overlay.label.clone();
        give_up(state, &key, &label, why);
    }
}

/// Запасная привязка по контуру каталога и то, насколько ей можно верить.
///
/// Ровно четыре вершины — это углы снятого куска, и растр по ним натягивают
/// (`rough: false`). Всё сложнее — полоса съёмки, очерченная десятками вершин:
/// углов у неё нет, и вместо них берётся габарит вписанного круга
/// (`rough: true`). Натягивать по нему нельзя, и глобус этого не делает — он
/// держит место, пока привязку не принесёт сам растр (см. `Frame::Rough`).
///
/// Отказывать сложному контуру здесь было бы рано: привязка живёт в растре
/// чаще, чем в каталоге, — гранула Sentinel-5P несёт широту и долготу
/// поотсчётно, — а спросить растр можно только открыв его.
///
/// Четырёхугольник во всю Землю углами тоже не описывается, и это не редкость:
/// глобальная сетка климатики очерчена прямоугольником от −180 до 180, а это на
/// шаре одна и та же долгота. Растягивать по нему нечего — все четыре угла
/// сходятся на одном меридиане, — поэтому такой контур идёт тем же путём, что и
/// полоса съёмки: местом, а не привязкой.
fn quad_of(product: &DataProduct) -> Option<([(f64, f64); 4], bool)> {
    // Колец несколько — снимок разрезан швом: через антимеридиан каталог
    // отдаёт его двумя полигонами, по половине с каждой стороны. Взять первое
    // и растянуть по нему весь растр значит показать снимок вдвое у́же снятого
    // и на своей же половине; сшить их обратно по вершинам, лежащим ровно на
    // ±180, нечем — порядок обхода у половин свой. Значит, это не
    // четырёхугольник, и путь у него тот же, что у полосы съёмки: место, а не
    // привязка.
    if product.footprint.len() == 1
        && let Some(ring) = product.footprint.first()
    {
        let points = match ring.points.as_slice() {
            [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
                &ring.points[..ring.points.len() - 1]
            }
            all => all,
        };
        if let [a, b, c, d] = points {
            let lons = [a.lon, b.lon, c.lon, d.lon];
            let span = lons.iter().fold(f64::MIN, |wide, lon| wide.max(*lon))
                - lons.iter().fold(f64::MAX, |narrow, lon| narrow.min(*lon));
            if span < WHOLE_EARTH_DEG {
                return Some((
                    [(a.lat, a.lon), (b.lat, b.lon), (c.lat, c.lon), (d.lat, d.lon)],
                    false,
                ));
            }
        }
    }
    // Круг, в который вписан контур, — единственная мера сложного контура,
    // которая не врёт ни у полюса, ни на шве (см. `footprint::frame`).
    let circle = footprint::frame(&product.footprint)?;
    let (north, south) = (
        (circle.lat + circle.radius_deg).min(90.0),
        (circle.lat - circle.radius_deg).max(-90.0),
    );
    let (west, east) = (circle.lon - circle.radius_deg, circle.lon + circle.radius_deg);
    Some(([(north, west), (north, east), (south, east), (south, west)], true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::data_provider::{GeoPoint, Ring};

    fn ring(points: &[(f64, f64)]) -> Ring {
        Ring { points: points.iter().map(|&(lat, lon)| GeoPoint { lat, lon }).collect() }
    }

    fn shot(footprint: Vec<Ring>) -> DataProduct {
        DataProduct { footprint, ..Default::default() }
    }

    /// Обычный четырёхугольник остаётся привязкой-догадкой: по нему растр и
    /// кладут, пока из самого растра не приехала решётка.
    #[test]
    fn один_четырёхугольник_остаётся_догадкой() {
        let (corners, rough) = quad_of(&shot(vec![ring(&[
            (60.0, 10.0),
            (60.0, 20.0),
            (50.0, 20.0),
            (50.0, 10.0),
        ])]))
        .expect("контур есть");
        assert!(!rough, "четырёхугольник назван местом");
        assert_eq!(corners[0], (60.0, 10.0));
    }

    /// Снимок через антимеридиан каталог отдаёт двумя полигонами. Первый из
    /// них — половина снимка, и растянутый по ней растр лёг бы вдвое у́же
    /// снятого; такой контур идёт местом, а не привязкой.
    #[test]
    fn разрезанный_швом_контур_привязкой_не_становится() {
        // Живой футпринт S1C_EW_OCN…: две половины по обе стороны от ±180.
        let halves = vec![
            ring(&[(83.13, 180.0), (86.61, 180.0), (87.43, 136.38), (83.76, 147.32)]),
            ring(&[(86.61, -180.0), (83.13, -180.0), (83.09, -178.30), (86.10, -153.28)]),
        ];
        let (_, rough) = quad_of(&shot(halves)).expect("контур есть");
        assert!(rough, "половина снимка выдана за его привязку");
    }

    /// Контура нет вовсе — наводить и класть не по чему.
    #[test]
    fn без_контура_нет_и_места() {
        assert!(quad_of(&shot(Vec::new())).is_none());
    }
}
