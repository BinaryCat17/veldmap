//! view/shown.rs — «На просмотре»: чем сейчас накрыт шар.
//!
//! Общий экран списка сюда не годится, и это не лень: у таблицы строка — это
//! запись каталога, у которой спрашивают размер, дату и состояние загрузки, а
//! здесь строка — слой, у которого спрашивают прозрачность и видимость. Общими
//! у них остаются только имя и значок, и сводить ради этого две сетки в одну
//! значило бы завести колонки, пустующие у каждой второй строки.
//!
//! Порядок на экране обратен порядку набора: сверху новые, а на шаре последний
//! пришедший лежит поверх остальных (см. state::overlay). Переворот живёт
//! здесь, потому что «сверху новые» — свойство экрана, а не набора.
//!
//! Показано здесь всё, что на шаре, а не одни растры: снимок бывает на нём и
//! одним контуром, и тогда убрать его больше неоткуда — значок стои́т в списке,
//! до которого ещё надо дойти. Слои и контуры идут двумя группами: это два
//! независимых состояния снимка (см. handlers::outline), и спрашивают у них
//! разное — у слоя прозрачность и порядок, у контура ничего, — так что одна
//! сетка на двоих пустовала бы у каждой второй строки.

use veld_ui_service_wrap::{column, popover, row, slider, Keyed};
use crate::proto::ui_service::{
    container, mono, scrollable, text, tooltip, Alignment, Color, Element, Length, Padding,
    ScrollDirection, TooltipPosition,
};
use crate::module::components::{format, list_screen, menu, preview_of, table};
use crate::module::state::listing::Choice;
use crate::module::state::{globe::Outlined, overlay::OverlayState, Shift, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// Кнопок в строке слоя: скрыть, навести, убрать и меню. Порядок слоёв и
/// переходы к снимку живут в меню (см. [`options`]). Ряд здесь свой, а не
/// табличный: у слоя есть порядок, а у записи каталога его нет.
const BUTTONS: f32 = 4.0;

/// Что занимает в строке место помимо имени и подписи состояния: ряд кнопок со
/// своими зазорами, отступы экрана, зазор до кнопок, значок слева и два зазора
/// вокруг него. Считается по числу кнопок, а не подбирается: приписанная кнопка
/// иначе молча уедет под обрезку.
///
/// Подписи состояния здесь нет: её ширина своя у каждой строки, и постоянным
/// числом её не выразить — «скрыт» это шесть знаков, а «ступень 1 из 9 · тайлы
/// 3/12» двадцать семь. Названная одним числом, она обрезалась ровно там, где
/// говорила больше всего (см. [`row_name_chars`]).
const NAME_OVERHEAD: f32 = theme::ROW_BUTTON * BUTTONS
    + table::BUTTON_GAP * (BUTTONS - 1.0)
    + table::BUTTON_GAP * 2.0
    + theme::GUTTER * 2.0
    + NAME_GLYPH
    + NAME_GAPS;

/// Значок слева от имени и два зазора вокруг него (`row![…].spacing(8.0)`).
const NAME_GLYPH: f32 = 13.0;
const NAME_GAPS: f32 = 8.0 * 2.0;

/// Сколько знаков имени влезает в строку слоя рядом с его подписью состояния.
///
/// Подпись меряется по себе самой: она стои́т справа от имени, растёт вместе с
/// ходом добычи и в тесной панели отнимает у имени вдвое больше, чем отнимало
/// постоянное число.
fn row_name_chars(width: f32, state: &str) -> usize {
    let room = width - NAME_OVERHEAD - format::text_width(state, theme::TEXT_SMALL);
    format::mono_fit(room.max(60.0), theme::TEXT_MONO)
}

/// Высота строки слоя: две строчки — имя и ползунок под ним.
const ROW_HEIGHT: f32 = 54.0;

pub fn view(state: &State, view: ViewId) -> Element<Msg> {
    // Ширина под имя — от панели, в которой список стоит: та же арифметика,
    // что у таблицы, и по той же причине.
    let width = state.pane_width(view);
    let shown = state.overlays.iter().filter(|overlay| !overlay.hidden).count();
    let ready = state.overlays.iter().filter(|overlay| overlay.on_globe()).count();
    // Снимок, лежащий растром, очерчен и сам: показывать его дважды значило бы
    // предложить убрать одно и то же двумя разными кнопками.
    let contours: Vec<&Outlined> = state
        .outlined
        .iter()
        .filter(|outlined| !state.overlays.iter().any(|layer| layer.identifier == outlined.key))
        .collect();

    let body: Element<Msg> = if state.overlays.is_empty() && contours.is_empty() {
        theme::empty("Пусто. Коробочка в строке снимка очерчивает его здесь, значок глобуса кладёт растром.").into()
    } else {
        let mut lines: Vec<Element<Msg>> = Vec::new();
        // Снизу вверх у набора — сверху вниз на экране.
        lines.extend(
            state.overlays.iter().rev().map(|overlay| layer(state, view, overlay, width)),
        );
        if !contours.is_empty() {
            if !state.overlays.is_empty() {
                lines.push(divider("Только контур"));
            }
            lines.extend(
                contours.iter().map(|outlined| contour(state, view, outlined, width)),
            );
        }
        scrollable(column(lines).width(Length::Fill))
            .direction(ScrollDirection::ScrollVertical)
            .scrollbar(theme::scrollbar())
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    column![
        header(state.overlays.len(), contours.len(), shown, ready),
        theme::hairline(theme::LINE_SOFT),
        container(body).width(Length::Fill).height(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Подпись над второй группой. Не заголовок списка: он один и стоит выше, а
/// это черта внутри — сказать, что дальше строки другого рода.
fn divider(label: &str) -> Element<Msg> {
    container(
        text::<Msg>(label.to_string())
            .size(theme::TEXT_SMALL)
            .color(theme::INK_DIM)
            .single_line(),
    )
    .background(theme::SHELF)
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// Заголовок: сколько слоёв и что с ними сделать разом. Собирается общей ролью
/// (см. `list_screen::heading`) — заголовок здесь такой же, как над всяким
/// списком, и своя его копия разошлась бы кеглем и высотой.
fn header(total: usize, contours: usize, shown: usize, ready: usize) -> Element<Msg> {
    let mut counts = format::counted(&[
        (total, ["слой", "слоя", "слоёв"]),
        (contours, ["контур", "контура", "контуров"]),
    ]);
    // О том, что не доехало, говорится только пока оно не доехало: строка
    // «0 собирается» на готовом наборе — шум.
    let assembling = total - ready;
    if assembling > 0 {
        counts.push_str(&format!(", {} собирается", assembling));
    } else if shown < total {
        counts.push_str(&format!(", {} на шаре", shown));
    }

    let mut trailing = Vec::new();
    if total > 0 {
        // Одна кнопка на оба действия: она называет то, что случится, а не то,
        // что есть сейчас. Всё скрыто — предлагает показать.
        let (label, hidden) = match shown > 0 {
            true => ("Скрыть все", true),
            false => ("Показать все", false),
        };
        trailing.push(theme::bar_button(label).on_press(Msg::OverlayHideAll(hidden)).into());
    }
    list_screen::heading("На просмотре", counts, trailing)
}

/// Одна строка: значок, имя, состояние и ползунок прозрачности.
fn layer(state: &State, view: ViewId, overlay: &OverlayState, width: f32) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let dim = overlay.hidden || !overlay.on_globe();
    // Скрытый слой гасится фоном, а не только чернилами: приглушённые чернила
    // здесь уже заняты — ими сказано «на шаре его ещё нет», и скрытый от
    // собирающегося отличался бы одним словом справа. Подсветка старше:
    // выделенный слой назван полосой под шаром и обведён на нём лентой, и
    // строка обязана сказать то же самое.
    let tint = if state.picked_key() == key {
        theme::RowTint::Picked
    } else if overlay.hidden {
        theme::RowTint::Dim
    } else {
        theme::RowTint::Plain
    };

    let said = state_said(overlay).map(|said| said.shown).unwrap_or_default();
    let name_chars = row_name_chars(width, &said);
    let name = row![
        theme::row_glyph::<Msg>(
            theme::glyph::SATELLITE,
            if dim { theme::INK_FAINT } else { theme::ACCENT },
        ),
        mono::<Msg>(format::ellipsize(&overlay.label, name_chars))
            .size(theme::TEXT_MONO)
            .color(if dim { theme::INK_FAINT } else { theme::INK_SOFT }),
        theme::spacer(),
        state_label(overlay),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);
    let name = container(name).width(Length::Fill);

    // Ползунок доступен и у скрытого: прозрачность — то, с чем возвращают на
    // шар уже настроенным, и гасить его значило бы заставлять сперва показать.
    let opacity = row![
        container(
            slider(0.0..=1.0, overlay.opacity, {
                let key = key.clone();
                move |value| Msg::OverlayOpacity(key, value)
            })
            // Шаг — не про точность, а про цену: прозрачность запечена в
            // вершинах наложения, и всякое новое значение пересобирает их все.
            // iced шлёт событие только на смене значения, поэтому шаг режет
            // сотни сообщений за протаскивание до двух десятков, а пяти
            // процентов глазу довольно.
            .step(0.05)
            .height(14.0)
            .style(theme::slider())
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center),
        container(
            mono::<Msg>(format!("{:.0}%", overlay.opacity * 100.0))
                .size(theme::TEXT_SMALL)
                .color(theme::INK_MUTED),
        )
        // Ширина под «100%» фиксирована: иначе ползунок дёргается на каждый
        // разряд, пока его тянут.
        .width(Length::Fixed(38.0))
        .align_x(Alignment::End),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);

    let buttons = vec![
        table::icon_button(
            if overlay.hidden { theme::glyph::EYE_OFF } else { theme::glyph::EYE },
            // Скрытие — включённый режим строки, и кнопка, которая его держит,
            // стои́т нажатой: иначе о нём говорит один перечёркнутый глаз в
            // одиннадцать точек.
            match overlay.hidden {
                true => theme::IconTone::Raised,
                false => theme::IconTone::Rest,
            },
            if overlay.hidden { "Показать на шаре" } else { "Скрыть" },
            Msg::OverlayHidden(key.clone(), !overlay.hidden),
        ),
        table::icon_button(
            theme::glyph::FOCUS,
            theme::IconTone::Rest,
            "Навести камеру и выделить",
            Msg::OutlineFocus(key.clone()),
        ),
        table::icon_button(
            theme::glyph::TRASH,
            theme::IconTone::Rest,
            "Убрать",
            Msg::OverlayRemove(key.clone()),
        ),
        options(state, view, overlay),
    ];

    line(
        column![name, opacity].spacing(6.0).width(Length::Fill).into(),
        buttons,
        key,
        tint,
        ROW_HEIGHT,
    )
}

/// Одна строка контура: снимок, который на шаре только очерчен.
///
/// Строчка одна, а не две: у контура нет ни прозрачности, ни порядка — он
/// либо есть, либо нет. Оттого и кнопок меньше: положить растром, убрать
/// контур и меню с переходами к самому снимку.
fn contour(state: &State, view: ViewId, outlined: &Outlined, width: f32) -> Element<Msg> {
    let key = outlined.key.clone();

    let name = row![
        theme::row_glyph::<Msg>(theme::glyph::SATELLITE, theme::INK_FAINT),
        mono::<Msg>(format::ellipsize(&outlined.label, row_name_chars(width, "контур")))
            .size(theme::TEXT_MONO)
            .color(theme::INK_SOFT),
        theme::spacer(),
        text::<Msg>("контур".to_string())
            .size(theme::TEXT_SMALL)
            .color(theme::INK_FAINT)
            .single_line(),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);

    // Значки те же и в том же порядке, что у строки слоя, и повторяющиеся —
    // те же самые: слои и контуры лежат в одном списке друг под другом, и
    // одинаково подписанное действие обязано быть одним и тем же. Разводит их
    // первый значок — то, чего у соседней строки нет: контур кладут растром, а
    // слою прятать. Знак «положить растром» тот же, каким это действие названо
    // в строке списка (см. `table::quick`), — одно действие, один знак.
    let buttons = vec![
        table::icon_button(
            theme::glyph::GLOBE,
            theme::IconTone::Rest,
            "Положить растром",
            Msg::In(view, ViewMsg::GlobeToggle(key.clone())),
        ),
        table::icon_button(
            theme::glyph::FOCUS,
            theme::IconTone::Rest,
            "Навести камеру и выделить",
            Msg::OutlineFocus(key.clone()),
        ),
        table::icon_button(
            theme::glyph::TRASH,
            theme::IconTone::Rest,
            "Убрать контур",
            Msg::OutlineRemove(key.clone()),
        ),
        menu_button(state, key.clone(), jumps(state, view, &key, outlined.folder)),
    ];

    let tint = match state.picked_key() == key {
        true => theme::RowTint::Picked,
        false => theme::RowTint::Plain,
    };
    line(
        container(name).width(Length::Fill).into(),
        buttons,
        key,
        tint,
        theme::ROW_HEIGHT,
    )
}

/// Хвост, общий у обеих строк списка: тело слева, ряд кнопок справа, подсветка
/// под ними и черта под строкой. Один на слой и на контур — стоят они в одном
/// списке друг под другом, и, разъехавшись отступом, зазором кнопок или высотой
/// черты, читались бы как два разных списка.
///
/// Высота объявляется и здесь, а не только у обёртки: `Length::Fill` у кнопок
/// внутри строки высотой `Shrink` схлопывается в ноль, и кнопки пропадают, не
/// оставив следа в раскладке (то же правило, что у переноса текста, — Fill
/// нужен на обоих уровнях).
fn line(
    body: Element<Msg>,
    buttons: Vec<Element<Msg>>,
    key: String,
    tint: theme::RowTint,
    height: f32,
) -> Element<Msg> {
    let content = row![
        body,
        container(row(buttons).spacing(table::BUTTON_GAP).align_items(Alignment::Center))
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ]
    .spacing(12.0)
    .align_items(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: theme::GUTTER,
        right: theme::GUTTER,
    });

    column![
        theme::layer_row(content, tint).width(Length::Fill).height(Length::Fixed(height)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(key)
    .into()
}

/// Меню строки: значок «ещё», раскрытая под ним панель и её закрытие. Одно на
/// слой и на контур — раскрытое адресуется ключом снимка (см.
/// `State::layer_menu`), и вторая запись этого правила однажды оставила бы
/// панель висеть над закрывшейся строкой.
fn menu_button(state: &State, key: String, items: Vec<menu::Item>) -> Element<Msg> {
    let open = state.layer_menu(&key);
    let anchor = theme::row_button_icon(
        theme::glyph::MORE,
        match open {
            true => theme::IconTone::Raised,
            false => theme::IconTone::Rest,
        },
    )
    .on_press(Msg::OverlayMenu(if open { None } else { Some(key) }));
    popover(anchor, open, || menu::panel(items))
        .align_x(Alignment::End)
        .gap(4.0)
        .on_dismiss(Msg::OverlayMenu(None))
        .into()
}

/// Пункты, которыми со строки уходят к самому снимку: посмотреть его и найти в
/// каталоге. Одни и те же у слоя и у контура — снимок за ними один, и
/// одинаково подписанное действие обязано быть одним и тем же.
///
/// `folder` — лежит ли снимок в хранилище каталогом: этим и различается, чем
/// его открывать (см. [`preview_of`]).
fn jumps(state: &State, view: ViewId, key: &str, folder: bool) -> Vec<menu::Item> {
    vec![
        menu::Item::new("Смотреть снимок", Msg::In(view, preview_of(&state.library, key, folder)))
            .glyph(theme::glyph::EYE),
        menu::Item::new("Показать в каталоге", Msg::In(view, ViewMsg::InCatalog(key.to_string())))
            .glyph(theme::glyph::FOLDER),
    ]
}

/// Меню слоя: то, что делают редко.
///
/// Порядок слоёв живёт здесь же. Двигают его нечасто, а каждый значок на строке
/// отнимает место у имени, которое и без того обрезается по ширине половины;
/// ряд, доросший до семи знаков, перестаёт читаться раньше, чем кончается
/// место. Переходы к снимку — по той же причине: со слоя уходят к тому, из чего
/// он сделан, и делают это тоже не каждую минуту.
///
/// Порядок в наборе — снизу вверх, на экране — сверху вниз, поэтому «выше» на
/// экране это `Shift::Up` в наборе: переворот один и живёт он здесь.
fn options(state: &State, view: ViewId, overlay: &OverlayState) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let mut items = vec![
        menu::Item::new(Shift::Up.title(), Msg::OverlayShift(key.clone(), Shift::Up))
            .glyph(theme::glyph::UP),
        menu::Item::new(Shift::Down.title(), Msg::OverlayShift(key.clone(), Shift::Down))
            .glyph(theme::glyph::DOWN),
    ];
    items.extend(jumps(state, view, &key, overlay.folder));
    menu_button(state, key, items)
}

/// Что со слоем прямо сейчас. Пусто у доехавшего видимого слоя: сказать о нём
/// нечего, кроме того, что и так видно на шаре.
///
/// Три состояния и три источника: собирается — наше (растры ещё открываются),
/// скрыт — тоже наше, а сколько тайлов осталось, знает только глобус и
/// рассказывает сам (см. `state::overlay::Progress`). Ждать снимок приходится
/// десятками секунд, и без этой подписи он выглядит зависшим.
/// Сколько знаков подписи о беде помещается в строку.
///
/// Ограничение не украшение: ширину под имя считает [`row_name_chars`] по тому,
/// что осталось от подписи, а причину пишет не разметка, а тот, кто отказал, —
/// длины у неё нет вовсе. Неограниченная, она оставляет имени снимка его нижний
/// предел в шестьдесят точек, то есть огрызок, по которому строку не узнать.
const TROUBLE_CHARS: usize = 44;

/// Подпись состояния слоя: что стои́т в строке, каким цветом и что сказать
/// целиком.
///
/// Целиком — врозь с показанным, потому что показанное ужато: срезанная на
/// полуслове причина не объясняет ничего, а место, где она помещается вся, в
/// строке одно — подсказка.
struct Said {
    shown: String,
    color: Color,
    whole: Option<String>,
}

/// Сколько знаков имени файла помещается перед словами о нём. Имя режется
/// отдельно от слов: срезанные вместе, они теряли бы то, ради чего стоят —
/// «не подробнее превью» за именем в тридцать знаков не доехало бы до строки.
const FILE_CHARS: usize = 16;

/// Сколько знаков в подсказке: одна строка, шире окна ей не быть — тот же
/// предел, что у списка загрузок. Части подписи делят её между собой
/// ([`format::share`]), каждая режется своим способом.
const TOOLTIP_CHARS: usize = 90;

fn state_said(overlay: &OverlayState) -> Option<Said> {
    let plain = |shown: &str, color| Said { shown: shown.to_string(), color, whole: None };
    let said = match (overlay.on_globe(), overlay.hidden) {
        (false, _) => plain("готовится…", theme::INK_FAINT),
        (true, true) => plain("скрыт", theme::INK_FAINT),
        // Отказ растра говорится последним и остаётся насовсем: пока слой
        // чего-то ждёт, важнее ожидание, а когда дождался — только он и
        // объясняет, почему приближение больше ничего не даст.
        (true, false) => match overlay.progress.said() {
            Some(said) => plain(&said, theme::ACCENT_TEXT),
            None => match settled_said(overlay) {
                Some((shown, whole)) => Said { shown, color: theme::INK_FAINT, whole: Some(whole) },
                None => return None,
            },
        },
    };
    Some(said)
}

/// Подпись улёгшегося слоя: имя лежащего подробным файла, его величина
/// (`variable`) и слова о нём — только о нём (`detailed_trouble`), — затем
/// остальное (`trouble`). Предел превью или отказ привязки под именем
/// подробного файла читались бы как сказанное о нём, поэтому граница между
/// ними идёт с глобуса, а не проводится здесь. Имя без слов — слой улёгся, и
/// сказать о нём больше нечего, кроме того, какой файл лёг и какая его
/// величина; остальное идёт за ним через точку с запятой, своим утверждением.
/// Пусто — нет ни имени, ни слов.
///
/// Показанное собирается по частям, каждая своим пределом: имя — [`FILE_CHARS`],
/// слова о подробном — тем, что осталось от строки, остальное — только если
/// место ещё есть. Величина в строке стои́т, пока о файле нечего сказать
/// хуже: слова о беде старше имени того, что не показалось. Подсказка — та же
/// подпись целиком, насколько её строка ([`TOOLTIP_CHARS`]) вмещает: части
/// делят строку поровну, короткие берут своё ([`format::share`]), так что имя
/// с единицами величины и суть беды доезжают при любом имени файла. Имя
/// режется посередине (края узнаваемы), слова — с хвоста (суть впереди).
fn settled_said(overlay: &OverlayState) -> Option<(String, String)> {
    /// Как режется часть: имя — серединой, слова — хвостом.
    enum Cut {
        Middle,
        Tail,
    }
    let mut shown: Vec<String> = Vec::new();
    // Части подсказки целиком, с тем, как каждую резать; остальное — последней.
    let mut parts: Vec<(String, Cut)> = Vec::new();
    let room = |shown: &[String]| {
        let taken: usize = shown.iter().map(|part| part.chars().count()).sum();
        match shown.is_empty() {
            true => TROUBLE_CHARS,
            false => TROUBLE_CHARS.saturating_sub(taken + 3).max(TROUBLE_CHARS / 4),
        }
    };
    if let Some(file) = &overlay.detailed {
        shown.push(format::ellipsize(file, FILE_CHARS));
        parts.push((file.clone(), Cut::Middle));
    }
    if let Some(variable) = &overlay.variable {
        if overlay.detailed_trouble.is_empty() {
            shown.push(format::head(format::variable_name(&variable.path), room(&shown)));
        }
        parts.push((format::variable(&variable.path, &variable.said, &variable.units), Cut::Tail));
    }
    if !overlay.detailed_trouble.is_empty() {
        let said = &overlay.detailed_trouble;
        shown.push(format::head(said, room(&shown)));
        parts.push((said.clone(), Cut::Tail));
    }
    let mut shown = shown.join(" · ");
    let about_detailed = parts.len();
    if !overlay.trouble.is_empty() {
        let rest = &overlay.trouble;
        if about_detailed == 0 {
            shown = format::head(rest, TROUBLE_CHARS);
        } else {
            let room = TROUBLE_CHARS.saturating_sub(shown.chars().count() + 2);
            if room >= TROUBLE_CHARS / 4 {
                shown = format!("{}; {}", shown, format::head(rest, room));
            }
        }
        parts.push((rest.clone(), Cut::Tail));
    }
    if parts.is_empty() {
        return None;
    }
    // Разделители — по три знака на стык (« · » длиннее «; » на один, и это
    // запас, а не недостача).
    let lengths: Vec<usize> = parts.iter().map(|(text, _)| text.chars().count()).collect();
    let shares = format::share(&lengths, TOOLTIP_CHARS.saturating_sub(3 * (parts.len() - 1)));
    let mut whole = String::new();
    for (at, ((text, cut), share)) in parts.into_iter().zip(shares).enumerate() {
        if at > 0 {
            whole.push_str(if at < about_detailed { " · " } else { "; " });
        }
        whole.push_str(&match cut {
            Cut::Middle => format::ellipsize(&text, share),
            Cut::Tail => format::head(&text, share),
        });
    }
    Some((shown, whole))
}

/// Та же подпись элементом. Врозь с [`state_said`] затем, что её ширину надо
/// знать до сборки строки: место под имя считается по ней.
fn state_label(overlay: &OverlayState) -> Element<Msg> {
    let Some(said) = state_said(overlay) else { return theme::nothing() };
    let label = text::<Msg>(said.shown).size(theme::TEXT_SMALL).color(said.color).single_line();
    match said.whole {
        // Подсказка собрана в свою строку ([`TOOLTIP_CHARS`]); срез здесь —
        // страховка, а не правило.
        Some(whole) => tooltip(label, format::ellipsize(&whole, TOOLTIP_CHARS), TooltipPosition::TooltipTop)
            .style(theme::panel())
            .text_size(theme::TEXT_SMALL)
            .into(),
        None => label.into(),
    }
}


#[cfg(test)]
mod tests {
    use super::{settled_said, FILE_CHARS, TOOLTIP_CHARS, TROUBLE_CHARS};
    use crate::module::state::overlay::OverlayState;

    fn overlay(detailed: Option<&str>, about_detailed: &str, rest: &str) -> OverlayState {
        let mut overlay = OverlayState::new("scene".to_string(), "снимок".to_string(), false, None, None, None);
        overlay.detailed = detailed.map(str::to_string);
        overlay.detailed_trouble = about_detailed.to_string();
        overlay.trouble = rest.to_string();
        overlay
    }

    fn with_variable(mut overlay: OverlayState) -> OverlayState {
        overlay.variable = Some(crate::proto::globe::Variable {
            path: "/PRODUCT/carbonmonoxide_total_column".to_string(),
            said: "Vertically integrated CO column".to_string(),
            units: "mol m-2".to_string(),
        });
        overlay
    }

    /// Величина стои́т за именем файла, пока о нём нечего сказать хуже; при
    /// словах о беде она остаётся в подсказке — перед словами, словами файла.
    #[test]
    fn the_variable_follows_the_name_until_there_is_trouble_to_tell() {
        let (shown, whole) = settled_said(&with_variable(overlay(Some("S5P_CO.nc"), "", ""))).unwrap();
        assert_eq!(whole, "S5P_CO.nc · carbonmonoxide_total_column, mol m-2 — Vertically integrated CO column");
        assert_eq!(shown, "S5P_CO.nc · carbonmonoxide_total_column");

        let (shown, whole) = settled_said(&with_variable(overlay(Some("S5P_CO.nc"), REFUSAL, ""))).unwrap();
        assert!(whole.starts_with("S5P_CO.nc · carbonmonoxide_total_column, mol m-2") && whole.contains(" · не подробнее превью: подробный растр"), "{whole}");
        assert!(whole.chars().count() <= TOOLTIP_CHARS, "{whole}");
        assert!(shown.starts_with("S5P_CO.nc · не подробнее") && !shown.contains("carbonmonoxide"), "{shown}");
    }

    /// Подсказка — одна строка на всех: имя файла в восемьдесят знаков не
    /// выталкивает из неё ни имя величины с единицами, ни суть беды. Части
    /// делят строку поровну, короткие берут своё.
    #[test]
    fn the_tooltip_shares_its_line_so_every_part_is_recognisable() {
        let long = "S5P_NRTI_L2__CO_____20260827T161616_20260827T162116_45971_03_020901_20260827T164732.nc";
        let (_, whole) = settled_said(&with_variable(overlay(Some(long), "", ""))).unwrap();
        assert!(whole.chars().count() <= TOOLTIP_CHARS, "{whole}");
        assert!(whole.starts_with("S5P_NRTI_L2__CO_____") && whole.contains(".nc · carbonmonoxide_total_column, mol m-2"), "{whole}");

        let (_, whole) = settled_said(&with_variable(overlay(Some(long), REFUSAL, CAP))).unwrap();
        assert!(whole.chars().count() <= TOOLTIP_CHARS, "{whole}");
        for part in ["S5P_NRTI_L", ".nc · carbonmonoxide_tota", " · не подробнее превью", "; подробнее 2001"] {
            assert!(whole.contains(part), "в подсказке нет «{part}»: {whole}");
        }
    }

    const REFUSAL: &str = "не подробнее превью: подробный растр на шар не кладётся";
    const CAP: &str = "подробнее 2001×1501 из 4001×3001 не будет";

    /// Имя файла стоит перед словами о нём и только перед ними: предел превью
    /// идёт после, за точкой с запятой, без имени. Целиком — в подсказке, а в
    /// строке имя и слова режутся порознь, так что суть слов доезжает.
    #[test]
    fn the_name_goes_before_the_words_about_its_file_only() {
        let (shown, whole) = settled_said(&overlay(Some("F1_BT_fn.nc"), REFUSAL, CAP)).unwrap();
        // Подсказка — одна строка на троих: суть отказа и начало предела
        // доезжают оба, срезанные с хвоста.
        assert!(whole.starts_with("F1_BT_fn.nc · не подробнее превью: подробный растр") && whole.contains("; подробнее 2001"), "{whole}");
        assert!(whole.chars().count() <= TOOLTIP_CHARS, "{whole}");
        assert!(shown.starts_with("F1_BT_fn.nc · не подробнее"), "{shown}");
        assert!(shown.chars().count() <= TROUBLE_CHARS, "{shown}");
        assert!(!shown.contains("4001×3001"), "остальному в строке места нет, оно в подсказке: {shown}");

        // Короткие слова оставляют остальному место — оно дописывается.
        let (shown, _) = settled_said(&overlay(Some("F1_BT_fn.nc"), "отвергнут", "мал")).unwrap();
        assert_eq!(shown, "F1_BT_fn.nc · отвергнут; мал");

        // Без слов о подробном имя — своё утверждение, за точкой с запятой;
        // под точкой посередине предел превью читался бы как сказанное о нём.
        let (shown, whole) = settled_said(&overlay(Some("F1_BT_fn.nc"), "", CAP)).unwrap();
        assert_eq!(whole, format!("F1_BT_fn.nc; {}", CAP));
        assert!(shown.starts_with("F1_BT_fn.nc; подробнее") && !shown.contains(" · "), "{shown}");

        let long = "S3A_SL_1_RBT____20260824T174507_F1_BT_fn.nc";
        let (shown, whole) = settled_said(&overlay(Some(long), REFUSAL, "")).unwrap();
        assert!(whole.starts_with(long) && whole.contains(" · не подробнее превью: подробный растр"), "{whole}");
        assert!(whole.chars().count() <= TOOLTIP_CHARS, "{whole}");
        assert!(shown.contains("не подробнее превью"), "суть слов срезана вместе с именем: {shown}");
        assert!(shown.find(" · ").is_some_and(|at| shown[..at].chars().count() <= FILE_CHARS), "{shown}");
    }

    /// Улёгшийся без слов слой называет файл, и только его; без имени и слов
    /// подписи нет.
    #[test]
    fn a_settled_layer_with_nothing_to_say_names_its_file() {
        assert_eq!(
            settled_said(&overlay(Some("F1_BT_fn.nc"), "", "")),
            Some(("F1_BT_fn.nc".to_string(), "F1_BT_fn.nc".to_string()))
        );
        assert_eq!(settled_said(&overlay(None, "", "")), None);
        let (shown, whole) = settled_said(&overlay(None, REFUSAL, "")).unwrap();
        assert_eq!(whole, REFUSAL, "слова о подробном без имени — как есть");
        assert!(shown.chars().count() <= TROUBLE_CHARS && shown.starts_with("не подробнее"), "{shown}");
    }
}
