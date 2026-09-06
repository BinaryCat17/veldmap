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
use veldsdk::proto::core::ResourceOpened;

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

/// Последний сегмент ключа хранилища — имя файла, каким его показывают в строке
/// слоя. Ключ папки с косой чертой на конце называется папкой.
fn file_name(identifier: &str) -> &str {
    identifier.trim_end_matches('/').rsplit('/').next().unwrap_or_default()
}

/// «На глобус» из строки списка — и обратно: тот же значок снимает то, что
/// положил.
///
/// Переключателем, а не односторонним показом, и ровно по той же механике, что
/// у значка контура: снимок либо на шаре, либо нет — вопрос один, и отвечать на
/// него нажатием туда и кнопкой в другом списке обратно значило бы завести
/// одному состоянию два рычага в разных местах.
///
/// Камеру это не двигает. Наводка — отдельное намерение и отдельная кнопка
/// (`Msg::OutlineFocus`): «покажи здесь» и «отвези меня туда» — разные просьбы,
/// и сведённые в одну они отбирают друг у друга ответ — положить второй снимок
/// рядом с первым становится нельзя, не улетев к нему.
///
/// Ключ приходит уже без завершающего слэша листинга: приводит его к виду
/// продукта сама строка (см. `Row::snapshot_key`), и второе такое правило
/// разошлось бы с первым.
///
/// Показ выбора коробочкой не ставит: это разные намерения. Выбирают, чтобы
/// что-то с набранным сделать — очертить, удалить, скачать, — а показывают,
/// чтобы посмотреть, и живёт показанное своим списком («На просмотре»). Ставя
/// заодно коробочку, показ клал бы снимок в состав следующего пакетного
/// действия, которого никто не просил.
pub fn on_toggle_pressed(state: &mut State, view: ViewId, identifier: String) {
    // Меню строки закрываем сами: нажатое в нём сделано, а открытым оно
    // осталось бы висеть над списком.
    state.close_menus();
    // Прежний отказ снимается новой попыткой: держать его дальше значило бы
    // объяснять причиной то, чего больше не происходит, — а сорваться показ
    // мог и по сети, и второй заход тогда удаётся.
    state.unshowable.remove(&identifier);
    // Уже на шаре или туда едет — значит просят снять. Скрытый в этот счёт
    // идёт наравне с видимым: он остаётся слоем, и значок в строке горит у
    // него так же.
    if state.overlays.iter().any(|overlay| overlay.identifier == identifier) {
        remove(state, &identifier);
        return;
    }
    // Ход к каталогу за продуктом убить нечем — он ответит всё равно, — но
    // расхотеть показ можно: ответ спросит, всё ли ещё его ждут (см.
    // [`on_located`]).
    if state.showing.remove(&identifier) {
        veldsdk::log::info!(target: "handlers", "показ '{}' отменён до ответа каталога", identifier);
        send_set(state);
        return;
    }
    // Сперва выдача: там у продукта есть всё, что нужно, и ходить за ним в
    // каталог второй раз незачем.
    if super::search::show(state, view, &identifier) {
        super::nav::on_new_globe(state);
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
    // Просьба уже видна — значком в строке и полосой под ней (см.
    // `rows::onto_globe`), — а место снимка на шаре покажет штриховка, если
    // геометрия под рукой.
    send_set(state);
    super::nav::on_new_globe(state);
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
    // приложение через несколько секунд после отмены само кладёт на шар
    // снимок, которого не просили.
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
        // Просьбы больше нет — значит нечего и штриховать (см. [`send_set`]).
        send_set(state);
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
}

/// Положить продукт на шар. Уже лежащий не кладётся заново: пересборка стоила
/// бы переоткрытия растров, а на шаре он и так есть.
///
/// Новый слой встаёт поверх прежних — концом набора, потому что его и просили
/// показать.
///
/// `source` — вид поиска, из выдачи которого продукт взят; для продукта,
/// восстановленного по ключу, его нет (см. `OverlayState::source`).
pub fn show(state: &mut State, product: &DataProduct, source: Option<ViewId>) {
    // Прежний отказ снимается всякой новой попыткой показать, каким бы путём
    // она ни пришла — значком, контуром или выдачей поиска. Сниматься он обязан
    // здесь, в общей воронке: у входов в показ три места, и причина, оставшаяся
    // после удавшегося показа, пережила бы его и всплыла бы охрой на снятом
    // слое.
    state.unshowable.remove(&product.identifier);
    if state.overlays.iter().any(|overlay| overlay.identifier == product.identifier) {
        // Класть нечего, а сказать есть о чём: сюда приходят и с ответом
        // каталога, снявшим просьбу (см. [`on_located`]), — а просьба помечена
        // на шаре штриховкой, и снять её больше некому.
        send_set(state);
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
        // Скачан ли снимок, знаем только мы: у провайдера одно хранилище. От
        // этого зависит состав растров — подробный, читаемый целым проходом,
        // предлагается лишь тогда, когда файл под рукой.
        downloaded: state.library.whole(&product.identifier),
        identifier: product.identifier.clone(),
    }, &correlation);
    // Набор наложений сам по себе не изменился — собирающийся в него не
    // попадает, — но изменилось то, что на шаре видно: место снимка помечается
    // штриховкой, пока картинки нет (см. [`send_set`]).
    send_set(state);
}

/// Убрать одно наложение: ресурсы отпустить, набор переслать.
pub fn remove(state: &mut State, key: &str) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let overlay = state.overlays.remove(index);
    abandon(state, overlay);
    // Снятый слой мог быть тем, что обведён лентой: без него ленте не на чем
    // держаться (см. `outline::forget_gone`).
    super::outline::forget_gone(state);
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
    super::outline::forget_gone(state);
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
        if let Some((_, role, ordinal, part)) = state.opens.take(correlation) {
            state.opens.insert(correlation.clone(), (String::new(), role, ordinal, part));
        }
    }
    for (.., handle) in assembly.collected {
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
    super::outline::forget_gone(state);
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
    // Сказать надо в двух местах, и это не дублирование: вопросы разные.
    // Строка состояния отвечает «что случилось сейчас» — нажавший значок в
    // этот миг смотрит на шар, а не в список, и другого места сказать ему нет.
    // Память у ключа отвечает «что это за снимок»: причина переживает слово в
    // строке состояния и встречает вернувшегося в список — иначе он видит тот
    // же зовущий значок и ждёт отказа снова.
    state.notice =
        Some(format!("«{}»: {}", crate::module::components::format::ellipsize(label, 40), why));
    state.unshowable.insert(key.to_string(), why);
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
    // Имена файлов по номеру в ответе — ими потом называется лежащий подробным.
    let files: Vec<String> = response.rasters.iter().map(|raster| raster.identifier.clone()).collect();
    // Порядковый номер растра в ответе — им координаты находят свой растр:
    // подробных бывает несколько (выбранный и запасные), и роли на это не
    // хватает.
    for (ordinal, raster) in (0u32..).zip(response.rasters) {
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
            let correlation = state.opens.begin((key.clone(), role, ordinal, part));
            opens.push(correlation.clone());
            super::open_resource(identifier, local, &correlation);
        }
    }

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    // Запрос растров кончился — его корреляция больше не наша.
    overlay.imagery = None;
    overlay.files = files;
    overlay.assembly = Some(Assembly { utm, opens, collected: Vec::new() });
}

/// Открытие растра наложения. `false` — ответ не наш: чужая корреляция либо
/// сборка, которую уже оборвали (её открытия сняты с учёта в `abandon`), и
/// приехавший ресурс добьёт общий discard (см. module::on_open_result).
pub fn on_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation = veldsdk::correlation();
    let Some((key, role, ordinal, part)) = state.opens.take(&correlation) else { return false };

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
        Ok(handle) => assembly.collected.push((role, ordinal, part, handle)),
        // Не открылось — наложение живёт тем, что открылось: без растра нет
        // роли, без координат нет привязки, и оба исхода объясняются ниже.
        Err(error) => veldsdk::log::warn!(target: "handlers", "файл наложения: {}", error),
    }
    if assembly.opens.is_empty() {
        finish(state, &key);
    }
    true
}

/// Собранное — в пары «растр и его координаты», превью вперёд. Вторым ответом
/// — координаты, которым растра не нашлось: наложению они не файл, а свойство
/// растра, и отпускает их вызывающий, пока они ещё его.
///
/// Порядок здесь не украшение, а цена показа: глобус рассылает описания
/// подряд по набору (`globe::module::adopt_overlay`), а тайлер описывает по
/// одному, так что первый в списке держит остальных. Превью — квиклук в сотни
/// килобайт, подробный — гранула в мегабайты, и по сети между ними десятки
/// секунд. Провайдер кладёт превью первым, а теряется порядок посередине:
/// открытия уходят разом, ответы сети приходят вразнобой, и собранное лежит в
/// порядке их прихода.
///
/// Первую картинку это приближает не всякому слою: у квадовой привязки
/// глобус не рисует ничего, пока не описались оба растра
/// (`overlay::binding_pending`), — там порядок решает только то, что раньше
/// освободит очередь тайлера.
///
/// Подробных растров бывает несколько — выбранный и запасные за ним, в
/// порядке провайдера (см. `imagery::spares`), — и координаты находят свой
/// растр по порядковому номеру в ответе, а не по роли: роль у запасных та же.
/// Порядок номеров внутри роли сохраняется — он и есть порядок предпочтения.
fn paired<H>(
    collected: Vec<(OverlayRole, u32, Part, H)>,
) -> (Vec<(OverlayRole, u32, H, Option<H>)>, Vec<H>) {
    let mut pairs: Vec<(OverlayRole, u32, H, Option<H>)> = Vec::new();
    let mut coordinates = Vec::new();
    for (role, ordinal, part, handle) in collected {
        match part {
            Part::Raster => pairs.push((role, ordinal, handle, None)),
            Part::Geolocation => coordinates.push((ordinal, handle)),
        }
    }
    pairs.sort_by_key(|(role, ordinal, ..)| (*role as i32, *ordinal));

    let mut orphans = Vec::new();
    for (ordinal, handle) in coordinates {
        match pairs.iter_mut().find(|(_, other, ..)| *other == ordinal) {
            Some(pair) => pair.3 = Some(handle),
            None => orphans.push(handle),
        }
    }
    (pairs, orphans)
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
    let (pairs, orphans) = paired(assembly.collected);
    // Координаты сами по себе наложению не файл, а свойство растра: растр не
    // открылся — показывать по ним нечего.
    for handle in orphans {
        veldsdk::log::warn!(target: "handlers", "'{}': координаты без растра — отпускаем", label);
        veldsdk::resource::release(handle);
    }

    // Передача владения — до сообщения: получив набор, глобус вправе сразу
    // считать ресурсы своими. При отказе хелпер освободил ресурс сам.
    let mut rasters = Vec::new();
    for (role, ordinal, raster, coordinates) in pairs {
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
            ordinal,
            variable: String::new(),
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
    // Работа не кончилась, а началась: глобус получит растры и примется их
    // описывать — по сети это десятки секунд. Своего слова о ходе у нас нет и
    // не будет, его скажет он же (`on_overlay_progress`), но до первого его
    // слова проходит кадр, и оставленный на этот кадр покой гасил бы и полосу
    // под строкой, и штрих на шаре — на глазах и без причины.
    overlay.progress = Progress { working: true, blank: true, ..Progress::default() };
    send_set(state);
}

/// Отправить глобусу весь набор. Единственный способ что-либо ему сказать про
/// наложения — отсюда и одна точка вызова на каждое изменение.
///
/// Собирающиеся в набор не попадают: наложение без растров глобус принять не
/// может, и слать его значило бы просить снять то, чего он ещё не видел. Зато
/// именно они и есть та половина набора, которая на шаре видна контуром: место
/// едущего снимка помечено штрихом, пока картинки нет, — поэтому отсюда же
/// уезжает и набор контуров (см. `outline::send`). Два набора, но одно
/// изменение: собирающееся наложение меняет ровно то, что видно на шаре.
fn send_set(state: &State) {
    let overlays = state
        .overlays
        .iter()
        .filter(|overlay| overlay.on_globe())
        .map(|overlay| {
            // Квад — запасная привязка; при рамке он глобусу не нужен. Признак
            // его габаритности решается тем же ходом и не иначе: сказанный при
            // пустом кваде, он говорил бы о четырёхугольнике, которого в
            // сообщении нет.
            let (quad, rough) = match (overlay.utm.is_some(), overlay.quad) {
                (false, Some(points)) => (
                    points.map(|(lat, lon)| GeoPoint { lat, lon }).to_vec(),
                    overlay.rough,
                ),
                _ => (Vec::new(), false),
            };
            Overlay {
                key: overlay.identifier.clone(),
                label: overlay.label.clone(),
                utm: overlay.utm.clone(),
                quad,
                rough,
                rasters: with_variable(&overlay.rasters, overlay.variable_wanted.as_ref()),
                opacity: Some(overlay.opacity),
                hidden: overlay.hidden,
            }
        })
        .collect();

    crate::calls::globe::on_overlay(&Overlays { overlays });
    super::outline::send(state);
}

/// Растры набора с названной величиной у того, о ком она сказана, — по
/// номеру файла; у остальных пусто (выбор тайлера).
fn with_variable(rasters: &[OverlayRaster], wanted: Option<&(u32, String)>) -> Vec<OverlayRaster> {
    rasters
        .iter()
        .map(|raster| OverlayRaster {
            variable: match wanted {
                Some((ordinal, path)) if *ordinal == raster.ordinal => path.clone(),
                _ => String::new(),
            },
            ..raster.clone()
        })
        .collect()
}

/// Список величин в строке слоя: раскрыть, закрыть щелчком мимо или
/// повторным нажатием. Раскрытое одно на экран (см. `state::Open`).
pub fn variables_menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::LayerVariables(key)),
        None => state.close_menus(),
    }
}

/// Другая величина лежащего подробным файла слоя — глобусу тем же набором:
/// растр с названной величиной он опишет заново, ресурсы прежние. Показанная
/// уже она — нечего делать, кроме как закрыть список.
pub fn variable(state: &mut State, key: &str, variable: String) {
    state.close_menus();
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    if overlay.variable.as_ref().is_some_and(|shown| shown.path == variable) {
        return;
    }
    let Some(ordinal) = overlay.detailed_ordinal else { return };
    if overlay.variable_wanted.as_ref() == Some(&(ordinal, variable.clone())) {
        return;
    }
    overlay.variable_wanted = Some((ordinal, variable));
    send_set(state);
}

/// Ход добычи тайлов от глобуса: набор целиком, тем же правилом, что и сам
/// набор наложений, — о чьём ключе не сказано, у того ничего и не добывают.
///
/// Числа приезжают готовыми, и показывает их список слоёв и строка снимка в
/// каждом списке, где он есть (`rows::onto_globe`). Но одно из них меняет и
/// сам шар: пока по слою
/// нечего нарисовать, его место заштриховано, и снять штриховку может только
/// пришедшее отсюда «уже есть что» (см. `Progress::blank`).
/// Поэтому набор контуров пересылается здесь же.
pub fn on_overlay_progress(state: &mut State, msg: crate::proto::globe::OverlaysProgress) {
    for overlay in &mut state.overlays {
        // О чём не сказано, того не трогаем. Молчание здесь значит не «работа
        // кончилась», а «глобус про этот слой ещё не знает»: он отчитывается
        // обо всех, что у него есть, — и слой, только что отданный ему
        // (`finish`), первые кадры в его наборе не значится. Стёртое молчанием
        // гасило бы и полосу, и штрих ровно на эти кадры.
        let Some(said) = msg.overlays.iter().find(|progress| progress.key == overlay.identifier)
        else {
            continue;
        };
        overlay.trouble = said.trouble.clone();
        overlay.detailed_trouble = said.detailed_trouble.clone();
        overlay.variable = said.detailed_variable.clone();
        overlay.detailed_ordinal = said.detailed;
        // Список — от последнего отчёта с ним: отказавшее названной величине
        // описание списка не несёт, а выбрать другую нужно как раз тогда.
        if !said.detailed_variables.is_empty() {
            overlay.variables = said.detailed_variables.clone();
        }
        // Подробным лёг другой файл (запасной) — названное про прежний к нему
        // не относится.
        if overlay.variable_wanted.as_ref().is_some_and(|(ordinal, _)| said.detailed != Some(*ordinal)) {
            overlay.variable_wanted = None;
        }
        // Номер от глобуса — в имя файла: глобус имён не знает, а строке слоя
        // нужно имя, и стоять оно обязано рядом с приговором о том же файле.
        overlay.detailed = said
            .detailed
            .and_then(|at| overlay.files.get(at as usize))
            .map(|identifier| file_name(identifier).to_string());
        overlay.progress = Progress {
            ready: said.ready,
            total: said.total,
            share: said.share,
            working: said.working,
            step: said.step,
            steps: said.steps,
            blank: said.blank,
            pass: (said.pass_read, said.pass_total),
        };
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

    // Набор наложений от хода не меняется — меняется то, что на шаре видно
    // помимо них. Неизменившийся набор до шины не доходит, поэтому платы за
    // «на всякий случай» здесь нет.
    super::outline::send(state);
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

    /// Убранное наложение не оставляет за собой ничего: ресурсы оборванной
    /// сборки отпущены, а открытия в полёте сняты с ключа — их ответ пройдёт
    /// веткой «наложение убрали», а не ляжет в чужой набор.
    #[test]
    fn an_abandoned_overlay_leaks_nothing() {
        veldsdk::fake::install();
        let mut state = State::new(crate::module::handlers::Config { initial_view: None }).unwrap();
        let (preview, detailed) = (OverlayRole::OverlayPreview, OverlayRole::OverlayDetailed);
        let quicklook = veldsdk::fake::mount(vec![1, 2, 3]);
        let raster = veldsdk::fake::mount(vec![4, 5, 6]);
        state.opens.insert("open-1".to_string(), ("scene".to_string(), detailed, 1, Part::Raster));

        let mut overlay = OverlayState::new("scene".to_string(), "снимок".to_string(), false, None, None, None);
        overlay.assembly = Some(Assembly {
            utm: None,
            opens: vec!["open-1".to_string()],
            collected: vec![(preview, 0, Part::Raster, quicklook), (detailed, 1, Part::Raster, raster)],
        });

        abandon(&mut state, overlay);

        assert!(veldsdk::fake::leaked().is_empty(), "собранные растры не отпущены");
        let (key, ..) = state.opens.take("open-1").expect("открытие в полёте остаётся на учёте");
        assert!(key.is_empty(), "открытие снято с ключа наложения");
    }

    /// Набор уезжает глобусу превью вперёд, каким бы ни был порядок ответов
    /// сети. Тайлер описывает набор по одному, поэтому вставший первым держит
    /// остальных: у гранулы, где подробный растр вернулся раньше квиклука,
    /// очередь занимали мегабайты вместо сотен килобайт.
    #[test]
    fn превью_уезжает_впереди_подробного() {
        let (preview, detailed) = (OverlayRole::OverlayPreview, OverlayRole::OverlayDetailed);
        let collected = vec![(detailed, 1, Part::Raster, "подробный"), (preview, 0, Part::Raster, "квиклук")];

        let (pairs, orphans) = paired(collected);

        let order: Vec<_> = pairs.iter().map(|(_, _, raster, ..)| *raster).collect();
        assert_eq!(order, ["квиклук", "подробный"]);
        assert!(orphans.is_empty());
    }

    /// Координаты достаются растру своей роли, а не соседней и не первому
    /// попавшемуся: у Sentinel-3 подробный растр везёт свои широты отдельным
    /// файлом, и перепутать их с превью значит уложить снимок мимо себя.
    #[test]
    fn координаты_достаются_растру_своей_роли() {
        let (preview, detailed) = (OverlayRole::OverlayPreview, OverlayRole::OverlayDetailed);
        let collected = vec![
            (detailed, 1, Part::Geolocation, "широты"),
            (preview, 0, Part::Raster, "квиклук"),
            (detailed, 1, Part::Raster, "подробный"),
        ];

        let (pairs, orphans) = paired(collected);

        assert_eq!(pairs, vec![(preview, 0, "квиклук", None), (detailed, 1, "подробный", Some("широты"))]);
        assert!(orphans.is_empty());
    }

    /// Запасные подробные растры едут за выбранным своим порядком, и каждый
    /// со своими координатами: у гранулы SLSTR полукилометровый канал и
    /// километровый лежат на разных сетках, и файлы координат у них разные.
    #[test]
    fn запасные_идут_за_выбранным_каждый_со_своими_координатами() {
        let (preview, detailed) = (OverlayRole::OverlayPreview, OverlayRole::OverlayDetailed);
        let collected = vec![
            (detailed, 2, Part::Raster, "тепловой"),
            (detailed, 1, Part::Geolocation, "широты an"),
            (detailed, 2, Part::Geolocation, "широты tx"),
            (preview, 0, Part::Raster, "квиклук"),
            (detailed, 1, Part::Raster, "видимый"),
        ];

        let (pairs, orphans) = paired(collected);

        assert_eq!(
            pairs,
            vec![
                (preview, 0, "квиклук", None),
                (detailed, 1, "видимый", Some("широты an")),
                (detailed, 2, "тепловой", Some("широты tx")),
            ]
        );
        assert!(orphans.is_empty());
    }

    /// Номер от глобуса становится именем файла: лежащий подробным называется
    /// последним сегментом ключа из ответа провайдера, а пустой номер — нет
    /// подробного — оставляет строку без имени.
    #[test]
    fn лежащий_подробным_называется_именем_файла() {
        let mut state = State::new(crate::module::handlers::Config { initial_view: None }).unwrap();
        let mut overlay = OverlayState::new("scene".to_string(), "снимок".to_string(), false, None, None, None);
        overlay.files = vec![
            "eodata/S3A.SEN3/quicklook.jpg".to_string(),
            "eodata/S3A.SEN3/S1_radiance_an.nc".to_string(),
            "eodata/S3A.SEN3/F1_BT_fn.nc".to_string(),
        ];
        state.overlays.push(overlay);
        let progress = |detailed: Option<u32>| crate::proto::globe::OverlaysProgress {
            overlays: vec![crate::proto::globe::OverlayProgress {
                key: "scene".to_string(),
                detailed,
                ..Default::default()
            }],
        };

        on_overlay_progress(&mut state, progress(Some(2)));
        assert_eq!(state.overlays[0].detailed.as_deref(), Some("F1_BT_fn.nc"), "запасной назван своим именем");
        on_overlay_progress(&mut state, progress(None));
        assert_eq!(state.overlays[0].detailed, None);
        on_overlay_progress(&mut state, progress(Some(9)));
        assert_eq!(state.overlays[0].detailed, None, "номер вне ответа — имени нет");

        // Величина едет с тем же отчётом и хранится как приехала: словами её
        // называет строка слоя.
        let variable = crate::proto::globe::Variable {
            path: "/F1_BT_fn".to_string(),
            said: "brightness temperature".to_string(),
            units: "K".to_string(),
        };
        on_overlay_progress(&mut state, crate::proto::globe::OverlaysProgress {
            overlays: vec![crate::proto::globe::OverlayProgress {
                key: "scene".to_string(),
                detailed: Some(2),
                detailed_variable: Some(variable.clone()),
                ..Default::default()
            }],
        });
        assert_eq!(state.overlays[0].variable, Some(variable));
        on_overlay_progress(&mut state, progress(Some(2)));
        assert_eq!(state.overlays[0].variable, None, "отчёт без величины снимает прежнюю");
    }

    /// Названная величина уезжает глобусу у растра с тем номером, о котором
    /// сказана, и снимается, когда подробным лёг другой файл; список величин
    /// переживает отчёт без него.
    #[test]
    fn названная_величина_едет_к_своему_растру_и_снимается_с_запасным() {
        let raster = |ordinal: u32| crate::proto::globe::OverlayRaster { ordinal, ..Default::default() };
        let sent = with_variable(&[raster(0), raster(1), raster(2)], Some(&(1, "/PRODUCT/qa_value".to_string())));
        assert_eq!(sent.iter().map(|r| r.variable.as_str()).collect::<Vec<_>>(), ["", "/PRODUCT/qa_value", ""]);
        assert!(with_variable(&[raster(0)], None).iter().all(|r| r.variable.is_empty()));

        let mut state = State::new(crate::module::handlers::Config { initial_view: None }).unwrap();
        let mut overlay = OverlayState::new("scene".to_string(), "снимок".to_string(), false, None, None, None);
        overlay.rasters = vec![raster(0), raster(1)];
        state.overlays.push(overlay);
        let listed = |detailed: Option<u32>, paths: &[&str]| crate::proto::globe::OverlaysProgress {
            overlays: vec![crate::proto::globe::OverlayProgress {
                key: "scene".to_string(),
                detailed,
                detailed_variables: paths
                    .iter()
                    .map(|path| crate::proto::globe::Variable { path: path.to_string(), ..Default::default() })
                    .collect(),
                ..Default::default()
            }],
        };
        on_overlay_progress(&mut state, listed(Some(1), &["/a", "/b"]));
        // Показанную шаром называть заново нечего: набор с названным выбором
        // тайлера — второй отпечаток тех же тайлов.
        state.overlays[0].variable = Some(crate::proto::globe::Variable { path: "/a".to_string(), ..Default::default() });
        variable(&mut state, "scene", "/a".to_string());
        assert_eq!(state.overlays[0].variable_wanted, None, "показанная названа заново");
        variable(&mut state, "scene", "/b".to_string());
        assert_eq!(state.overlays[0].variable_wanted, Some((1, "/b".to_string())));

        // Отчёт без списка прежнего не стирает; тот же файл — названное живёт.
        on_overlay_progress(&mut state, listed(Some(1), &[]));
        assert_eq!(state.overlays[0].variables.len(), 2, "отказ стёр список");
        assert_eq!(state.overlays[0].variable_wanted, Some((1, "/b".to_string())));
        // Подробным лёг запасной — названное снято.
        on_overlay_progress(&mut state, listed(Some(2), &["/c"]));
        assert_eq!(state.overlays[0].variable_wanted, None);
    }

    /// Имя файла — последний сегмент ключа; ключ папки называется папкой, а
    /// ключ без косых — сам собой.
    #[test]
    fn имя_файла_это_последний_сегмент_ключа() {
        assert_eq!(file_name("eodata/S3A.SEN3/F1_BT_fn.nc"), "F1_BT_fn.nc");
        assert_eq!(file_name("eodata/S3A.SEN3/"), "S3A.SEN3");
        assert_eq!(file_name("quicklook.jpg"), "quicklook.jpg");
        assert_eq!(file_name(""), "");
    }

    /// Координаты, чей растр не открылся, наложению не файл: их возвращают
    /// вызывающему, пока они ещё его и есть чем их отпустить.
    #[test]
    fn координаты_без_своего_растра_возвращаются_отпустить() {
        let (preview, detailed) = (OverlayRole::OverlayPreview, OverlayRole::OverlayDetailed);
        let collected = vec![(detailed, 1, Part::Geolocation, "широты"), (preview, 0, Part::Raster, "квиклук")];

        let (pairs, orphans) = paired(collected);

        assert_eq!(pairs, vec![(preview, 0, "квиклук", None)]);
        assert_eq!(orphans, vec!["широты"]);
    }

    /// Значок кладёт снимок на шар и им же снимает — в обоих положениях, в
    /// каких показ бывает: пока продукт восстанавливают по ключу и когда слой
    /// уже заведён. Без второго нажатия снять слой можно было бы только из
    /// другого списка, то есть одному состоянию досталось бы два рычага в
    /// разных местах.
    #[test]
    fn показ_снимается_тем_же_значком() {
        let key = "eodata/store/A.SAFE";
        let mut state =
            State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние");
        let pane = state.focused();
        let view = state.open_in(
            pane,
            crate::module::state::ViewKind::Browse(crate::module::state::BrowseState::default()),
        );

        // Продукта под рукой нет — ушёл вопрос к каталогу, и просьба записана.
        on_toggle_pressed(&mut state, view, key.to_string());
        assert!(state.showing.contains(key), "просьба не записана");

        // Второе нажатие снимает её, не дожидаясь ответа.
        on_toggle_pressed(&mut state, view, key.to_string());
        assert!(!state.showing.contains(key), "просьбу нечем отменить");

        // Слой уже заведён — второе нажатие убирает его.
        let product = DataProduct {
            identifier: key.to_string(),
            footprint: vec![ring(&[(10.0, 10.0), (10.0, 20.0), (20.0, 20.0), (20.0, 10.0)])],
            ..Default::default()
        };
        show(&mut state, &product, None);
        assert_eq!(state.overlays.len(), 1);
        on_toggle_pressed(&mut state, view, key.to_string());
        assert!(state.overlays.is_empty(), "лежащий слой нечем снять");
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

    /// Отказ показа помнится ключом снимка и переживает слово в строке
    /// состояния: вернувшись в список через минуту, человек обязан увидеть, что
    /// этот снимок уже пробовали и почему не вышло, — иначе он жмёт тот же
    /// значок и ждёт тех же полминуты.
    ///
    /// А новая попытка причину снимает: сорваться показ мог и по сети, и
    /// объяснять отказом то, чего больше не происходит, — врать.
    #[test]
    fn отказ_показа_помнится_снимком_и_снимается_новой_попыткой() {
        let key = "eodata/store/A.SEN3";
        let mut state =
            State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние");
        let pane = state.focused();
        let view = state.open_in(
            pane,
            crate::module::state::ViewKind::Browse(crate::module::state::BrowseState::default()),
        );

        give_up(&mut state, key, "A.SEN3", "это измерения, а не изображение".to_string());
        assert_eq!(
            crate::module::components::rows::unshowable(&state, key),
            "это измерения, а не изображение",
            "причина не досталась строке"
        );
        // Сказано и там, где смотрит нажавший: он в этот миг на шаре, а не в
        // списке.
        assert!(state.notice.is_some(), "о случившемся не сказано вовсе");

        on_toggle_pressed(&mut state, view, key.to_string());
        assert!(
            crate::module::components::rows::unshowable(&state, key).is_empty(),
            "прежний отказ пережил новую попытку"
        );

        // Чужой строки это не касается: причина принадлежит снимку.
        give_up(&mut state, key, "A.SEN3", "не влезает".to_string());
        assert!(crate::module::components::rows::unshowable(&state, "eodata/store/B.SEN3").is_empty());

        // И снимается она всяким путём в показ, а не одним лишь значком: путей
        // три (значок, контур, выдача поиска), и причина, пережившая удавшийся
        // показ, всплыла бы охрой на снятом слое.
        let product = DataProduct { identifier: key.to_string(), ..Default::default() };
        show(&mut state, &product, None);
        assert!(
            crate::module::components::rows::unshowable(&state, key).is_empty(),
            "показ другим путём оставил прежнюю причину"
        );
    }
}
