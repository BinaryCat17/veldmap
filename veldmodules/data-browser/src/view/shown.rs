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
//! одним контуром, и тогда убрать его больше неоткуда — отметка стои́т в
//! списке, до которого ещё надо дойти. Слои и контуры идут двумя группами:
//! спрашивают у них разное (у слоя — прозрачность и порядок, у контура —
//! ничего), и одна сетка на двоих пустовала бы у каждой второй строки.

use veld_ui_service_wrap::{column, popover, row, slider, Keyed};
use crate::proto::ui_service::{
    container, mono, scrollable, text, Alignment, Element, Length, Padding, ScrollDirection,
};
use crate::module::components::{format, list_screen, menu, preview_of, table};
use crate::module::state::{globe::Outlined, overlay::OverlayState, Shift, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// Кнопок в строке слоя: скрыть, навести, убрать и меню. Порядок слоёв и
/// переходы к снимку живут в меню (см. [`options`]). Ряд здесь свой, а не
/// табличный: у слоя есть порядок, а у записи каталога его нет.
const BUTTONS: f32 = 4.0;

/// Что занимает в строке место помимо имени: ряд кнопок со своими зазорами,
/// отступы экрана, зазор до кнопок и подпись состояния. Считается по числу
/// кнопок, а не подбирается: приписанная кнопка иначе молча уедет под обрезку.
const NAME_OVERHEAD: f32 = theme::ROW_BUTTON * BUTTONS
    + table::BUTTON_GAP * (BUTTONS - 1.0)
    + table::BUTTON_GAP * 2.0
    + theme::GUTTER * 2.0
    + STATE_WIDTH;

/// Место под подпись состояния справа от имени («готовится…», «скрыт»).
const STATE_WIDTH: f32 = 90.0;

/// Высота строки слоя: две строчки — имя и ползунок под ним.
const ROW_HEIGHT: f32 = 54.0;

pub fn view(state: &State, view: ViewId) -> Element<Msg> {
    // Ширина под имя — от панели, в которой список стоит: та же арифметика,
    // что у таблицы, и по той же причине.
    let name_chars = format::mono_fit(
        (state.pane_width(view) - NAME_OVERHEAD).max(120.0),
        theme::TEXT_MONO,
    );
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
            state.overlays.iter().rev().map(|overlay| layer(state, view, overlay, name_chars)),
        );
        if !contours.is_empty() {
            if !state.overlays.is_empty() {
                lines.push(divider("Только контур"));
            }
            lines.extend(
                contours.iter().map(|outlined| contour(state, view, outlined, name_chars)),
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
    // Пустых слов в подписи нет: чего нет, о том и не сказано.
    let mut said: Vec<String> = Vec::new();
    if total > 0 {
        said.push(format!("{} {}", total, format::plural(total, ["слой", "слоя", "слоёв"])));
    }
    if contours > 0 {
        said.push(format!(
            "{} {}",
            contours,
            format::plural(contours, ["контур", "контура", "контуров"])
        ));
    }
    let mut counts = said.join(", ");
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
fn layer(state: &State, view: ViewId, overlay: &OverlayState, name_chars: usize) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let dim = overlay.hidden || !overlay.on_globe();

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

    let buttons = row![
        table::hinted(
            theme::row_button_icon(
                if overlay.hidden { theme::glyph::EYE_OFF } else { theme::glyph::EYE },
                false,
            )
            .on_press(Msg::OverlayHidden(key.clone(), !overlay.hidden)),
            if overlay.hidden { "Показать на шаре" } else { "Скрыть" },
        ),
        table::hinted(
            theme::row_button_icon(theme::glyph::GLOBE, false)
                .on_press(Msg::In(view, ViewMsg::GlobeShow(key.clone()))),
            "Навести шар",
        ),
        table::hinted(
            theme::row_button_icon(theme::glyph::TRASH, false)
                .on_press(Msg::OverlayRemove(key.clone())),
            "Убрать",
        ),
        options(state, view, overlay),
    ]
    .spacing(6.0)
    .align_items(Alignment::Center);

    // Высота строки объявляется здесь, а не только у обёртки: `Length::Fill`
    // у кнопок внутри строки высотой `Shrink` схлопывается в ноль, и кнопки
    // пропадают, не оставив следа в раскладке (то же правило, что у переноса
    // текста, — Fill нужен на обоих уровнях).
    let line = row![
        column![name, opacity].spacing(6.0).width(Length::Fill),
        container(buttons).height(Length::Fill).align_y(Alignment::Center),
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
        container(line).width(Length::Fill).height(Length::Fixed(ROW_HEIGHT)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(key)
    .into()
}

/// Одна строка контура: снимок, который на шаре только очерчен.
///
/// Строчка одна, а не две: у контура нет ни прозрачности, ни порядка — он
/// либо есть, либо нет. Оттого и кнопок меньше: положить растром, убрать
/// контур и меню с переходами к самому снимку.
fn contour(state: &State, view: ViewId, outlined: &Outlined, name_chars: usize) -> Element<Msg> {
    let key = outlined.key.clone();
    let open = state.layer_menu.as_deref() == Some(key.as_str());

    let name = row![
        theme::row_glyph::<Msg>(theme::glyph::SATELLITE, theme::INK_FAINT),
        mono::<Msg>(format::ellipsize(&outlined.label, name_chars))
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

    let items = vec![
        menu::Item::new("Навести шар", Msg::OutlineFocus(key.clone()))
            .glyph(theme::glyph::GLOBE),
        menu::Item::new(
            "Смотреть снимок",
            Msg::In(view, preview_of(&state.library, &key, outlined.folder)),
        )
        .glyph(theme::glyph::EYE),
        menu::Item::new("Показать в каталоге", Msg::In(view, ViewMsg::InCatalog(key.clone())))
            .glyph(theme::glyph::FOLDER),
    ];
    let anchor = theme::row_button_icon(theme::glyph::MORE, open)
        .on_press(Msg::OverlayMenu(if open { None } else { Some(key.clone()) }));

    let buttons = row![
        table::hinted(
            theme::row_button_icon(theme::glyph::LAYERS, false)
                .on_press(Msg::In(view, ViewMsg::GlobeShow(key.clone()))),
            "Положить растром",
        ),
        table::hinted(
            theme::row_button_icon(theme::glyph::TRASH, false)
                .on_press(Msg::OutlineRemove(key.clone())),
            "Убрать контур",
        ),
        popover(anchor, menu::panel(items))
            .open(open)
            .align_x(Alignment::End)
            .gap(4.0)
            .on_dismiss(Msg::OverlayMenu(None)),
    ]
    .spacing(6.0)
    .align_items(Alignment::Center);

    let line = row![
        container(name).width(Length::Fill),
        container(buttons).height(Length::Fill).align_y(Alignment::Center),
    ]
    .spacing(12.0)
    .align_items(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER });

    column![
        container(line).width(Length::Fill).height(Length::Fixed(theme::ROW_HEIGHT)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(key)
    .into()
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
    let open = state.layer_menu.as_deref() == Some(key.as_str());

    let items = vec![
        menu::Item::new("Выше", Msg::OverlayShift(key.clone(), Shift::Up))
            .glyph(theme::glyph::UP),
        menu::Item::new("Ниже", Msg::OverlayShift(key.clone(), Shift::Down))
            .glyph(theme::glyph::DOWN),
        menu::Item::new(
            "Смотреть снимок",
            Msg::In(view, preview_of(&state.library, &key, overlay.folder)),
        )
        .glyph(theme::glyph::EYE),
        menu::Item::new("Показать в каталоге", Msg::In(view, ViewMsg::InCatalog(key.clone())))
            .glyph(theme::glyph::FOLDER),
    ];

    let anchor = theme::row_button_icon(theme::glyph::MORE, open)
        .on_press(Msg::OverlayMenu(if open { None } else { Some(key) }));
    popover(anchor, menu::panel(items))
        .open(open)
        .align_x(Alignment::End)
        .gap(4.0)
        .on_dismiss(Msg::OverlayMenu(None))
        .into()
}

/// Что со слоем прямо сейчас. Пусто у обычного видимого слоя: сказать о нём
/// нечего, кроме того, что и так видно на шаре.
fn state_label(overlay: &OverlayState) -> Element<Msg> {
    let (label, color) = match (overlay.on_globe(), overlay.hidden) {
        (false, _) => ("готовится…", theme::INK_FAINT),
        (true, true) => ("скрыт", theme::INK_FAINT),
        (true, false) => return theme::nothing(),
    };
    text::<Msg>(label.to_string()).size(theme::TEXT_SMALL).color(color).single_line().into()
}

