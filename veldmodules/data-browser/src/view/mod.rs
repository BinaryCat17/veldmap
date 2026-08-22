//! View рендеринг для data-browser.
//!
//! Здесь каркас окна: дерево панелей, у каждой своя полоса вкладок и тело
//! активной в ней вкладки, а под ними — общая строка состояния. Содержимое
//! вкладки собирают модули рядом.
//!
//! Строка состояния одна на окно, а не на панель: она говорит о том, что
//! делает приложение целиком — идут ли закачки и сколько занято на диске, — и
//! к тому, на что смотрят в этой панели, отношения не имеет.

pub mod browse;
pub mod downloaded;
pub mod globe;
pub mod preview;
pub mod search;
pub mod shown;

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, popover, progress_bar, space, text, Alignment, Color, DividerAxis, Element,
    FontWeight, Length, Padding, ScrollDirection, scrollable,
};
use crate::module::components::{format, menu};
use crate::module::state::listing::Choice;
use crate::module::state::{Axis, Node, Open, PaneId, Side, State, ViewId, ViewKind};
use crate::module::{theme, Msg, NewTab, ViewMsg};


/// Высота полосы вкладок. Фиксирована, а не выведена из содержимого: это хром
/// постоянного размера, и ни одна вкладка не вправе растянуть его собой.
/// Строка состояния — обычная полоса хрома (theme::BAR_HEIGHT).
const TAB_STRIP_HEIGHT: f32 = 38.0;

/// Зазор между тем, что стоит в полосе хрома. Один на обе полосы: они читаются
/// как один язык, и разный зазор в них видно.
pub const BAR_SPACING: f32 = 10.0;

pub fn build_root(state: &State) -> Element<Msg> {
    container(
        column![
            container(node(state, state.layout().root()))
                .width(Length::Fill)
                .height(Length::Fill),
            theme::hairline(theme::LINE),
            status_bar(state),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .background(theme::PAGE)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Узел дерева: панель либо деление надвое (см. state::layout).
///
/// Состояние виджетов — прокрутку списка, каретку поля — рендерер сопоставляет
/// по месту в дереве разметки, а деление это место меняет: панель, бывшая
/// корнем, становится ребёнком строки. Прокрутка её списка поэтому переезда не
/// переживает, и это цена произвольных делений: форму дерева разметки задаёт
/// форма раскладки, и совпадать они обязаны.
fn node(state: &State, node: &Node) -> Element<Msg> {
    let split = match node {
        Node::Leaf(leaf) => return pane(state, leaf.id),
        Node::Split(split) => split,
    };

    let (head, tail) = portions(split.fraction);
    let first = container(self::node(state, &split.first));
    let second = container(self::node(state, &split.second));
    // Граница — не линия между детьми, а виджет: её тянут мышью, и ею дети
    // меряют своё место (см. `Msg::Divide`).
    let id = split.id;
    let border = |axis| theme::divider(axis, move |delta| Msg::Divide(id, delta), Msg::Divided);
    match split.axis {
        Axis::Row => row![
            first.width(Length::FillPortion(head)).height(Length::Fill),
            border(DividerAxis::DividerVertical),
            second.width(Length::FillPortion(tail)).height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
        Axis::Column => column![
            first.width(Length::Fill).height(Length::FillPortion(head)),
            border(DividerAxis::DividerHorizontal),
            second.width(Length::Fill).height(Length::FillPortion(tail)),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    }
}

/// Доля первого ребёнка — долями места: другого способа сказать «эта часть
/// вдвое шире» у разметки нет. Тысячные, потому что доли целые, а тысячная
/// экрана мельче, чем различает глаз.
///
/// Ни одна часть не доходит до нуля: панель нулевого размера остаётся в дереве
/// со всеми своими вкладками, но становится недоступной — вернуть ей место
/// было бы уже нечем.
fn portions(fraction: f32) -> (u16, u16) {
    let head = ((fraction * 1000.0).round() as i32).clamp(1, 999) as u16;
    (head, 1000 - head)
}

/// Одна панель: своя полоса вкладок и тело её активной вкладки.
fn pane(state: &State, pane: PaneId) -> Element<Msg> {
    let shown = state.active_in(pane).and_then(|id| state.get(id).map(|kind| (id, kind)));
    let body: Element<Msg> = match shown {
        Some((id, ViewKind::Empty)) => empty_tab(id),
        Some((id, ViewKind::Browse(view))) => browse::view(state, id, view),
        Some((id, ViewKind::Search(view))) => search::view(state, id, view),
        Some((id, ViewKind::Downloaded(listing))) => downloaded::view(state, id, listing),
        Some((id, ViewKind::Preview(view))) => preview::view(state, id, view),
        Some((id, ViewKind::Globe(view))) => globe::view(state, id, view),
        Some((id, ViewKind::Shown)) => shown::view(state, id),
        None => nothing_open(pane),
    };

    // Тело панели — место приёма: в середину вкладку кладут, в край — делят им
    // место (см. `Msg::TabDrop`). Полоса вкладок принимает тоже, но краёв у неё
    // нет: делить полосу не по чему.
    column![
        tab_strip(state, pane),
        theme::hairline(theme::LINE),
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .on_drop(move |drop| Msg::TabDrop(pane, drop), true, theme::DROP_HINT),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Пустая вкладка: место, где ещё не решили, что смотреть. Спрашивает об этом
/// прямо — списком того, что бывает, а не пустым местом с крестиком в углу.
fn empty_tab(id: ViewId) -> Element<Msg> {
    chooser("Пустая вкладка", "Выберите, что здесь показать.", |kind| {
        Msg::In(id, ViewMsg::Fill(kind))
    })
}

/// Панель, в которой не осталось ни одной вкладки. Так бывает у последней —
/// прочие опустевшие уходят из дерева (см. `State::close`).
fn nothing_open(pane: PaneId) -> Element<Msg> {
    chooser("Все вкладки закрыты", "Выберите, с чего начать.", move |kind| {
        Msg::NewTab(pane, kind)
    })
}

/// Выбор рода вкладки. Что с ним делать — наполнить пустую вкладку или завести
/// новую в опустевшей панели, — решает вызывающий: спрашивают они об одном и
/// том же, а отвечают разным.
fn chooser(title: &str, hint: &str, message: impl Fn(NewTab) -> Msg) -> Element<Msg> {
    let choices = NewTab::KINDS.iter().map(|kind| {
        theme::surface_button(
            row![
                icon::<Msg>(tab_glyph(*kind)).size(12.0).color(theme::ACCENT),
                text::<Msg>(kind.title().to_string()).size(theme::TEXT_BODY).single_line(),
            ]
            .spacing(10.0)
            .align_items(Alignment::Center),
            false,
        )
        .width(Length::Fill)
        .padding(Padding { top: 9.0, bottom: 9.0, left: 12.0, right: 12.0 })
        .align_x(Alignment::Start)
        .on_press(message(*kind))
        .into()
    });

    container(
        column![
            text::<Msg>(title.to_string())
                .size(theme::TEXT_TITLE)
                .color(theme::INK)
                .weight(FontWeight::WeightBold)
                .single_line(),
            text::<Msg>(hint.to_string()).size(theme::TEXT_BODY).color(theme::INK_DIM),
            container(space::<Msg>(Length::Fixed(0.0), Length::Fixed(6.0))),
            column(choices).spacing(6.0).width(Length::Fill),
        ]
        .spacing(6.0)
        .width(Length::Fixed(340.0)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x()
    .center_y()
    .into()
}

/// Полоса вкладок одной панели. Вкладка адресуется своим `ViewId`, а не
/// позицией: позиция меняется, когда закрывают соседа.
///
/// Вкладки не сжимаются, а уезжают под горизонтальную прокрутку: сжатие
/// доводит подпись до нулевой ширины, а такой текст занимает высоту, а не
/// ширину (см. `Wrapping` в types.proto).
fn tab_strip(state: &State, pane: PaneId) -> Element<Msg> {
    let active = state.active_in(pane);
    let tabs = state.views_in(pane).map(|view| {
        let current = Some(view.id) == active;
        let label = row![
            icon::<Msg>(glyph(&view.kind))
                .size(10.0)
                .color(if current { theme::ACCENT } else { theme::INK_FAINT }),
            text::<Msg>(view.kind.title()).size(theme::TEXT_BODY).single_line(),
        ]
        .spacing(7.0)
        .align_items(Alignment::Center);

        // Крестик и «ещё» — отдельные кнопки внутри вкладки: нажатие на них до
        // самой вкладки не доходит, поэтому не выбирает её заодно.
        //
        // Кнопки меню нет, пока предлагать нечего: единственной вкладке на весь
        // экран переезжать некуда, а «закрыть» у неё и так под рукой крестиком —
        // меню из одного этого пункта обещало бы выбор, которого нет.
        let options = state.open == Open::TabOptions(view.id);
        let arrangements = arrangements(state, view.id);
        let more: Element<Msg> = match arrangements.is_empty() {
            true => theme::nothing(),
            false => popover(
                theme::chrome_icon(
                    icon::<Msg>(theme::glyph::SPLIT).size(9.0).color(theme::INK_FAINT),
                )
                .on_press(Msg::TabOptions(if options { None } else { Some(view.id) })),
                options,
                || menu::panel(tab_options(arrangements, view.id)),
            )
            .gap(4.0)
            .on_dismiss(Msg::TabOptions(None))
            .into(),
        };

        let tab = theme::tab(
            row![
                label,
                more,
                theme::chrome_icon(icon::<Msg>(theme::glyph::CLOSE).size(9.0).color(theme::INK_FAINT))
                    .on_press(Msg::TabClose(view.id)),
            ]
            .spacing(4.0)
            .align_items(Alignment::Center),
            current,
        )
        .height(Length::Fill)
        .on_press(Msg::TabSelect(view.id))
        // Вкладку берут мышью и несут: чем она себя называет, тем и вернётся
        // в событии броска.
        .draggable(view.id.to_string());

        // Подчёркивание активной: рамка в этом протоколе одна на все стороны,
        // а нужна только нижняя.
        column![
            container(tab).height(Length::Fill).width(Length::Shrink),
            theme::hairline(if current { theme::ACCENT } else { Color::TRANSPARENT }),
        ]
        .width(Length::Shrink)
        .key(view.id.to_string())
        .into()
    });

    let strip = row(tabs)
        .spacing(2.0)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding { top: 5.0, right: 7.0, bottom: 0.0, left: 7.0 })
        .align_items(Alignment::End);

    let open = state.open == Open::TabMenu(pane);
    let opener = popover(
        theme::chrome_icon(icon::<Msg>(theme::glyph::PLUS).size(11.0).color(theme::INK_DIM))
            .width(Length::Fixed(TAB_STRIP_HEIGHT))
            .height(Length::Fill)
            .on_press(Msg::TabMenu(if open { None } else { Some(pane) })),
        open,
        || {
            menu::panel(
                NewTab::ALL
                    .iter()
                    .map(|kind| {
                        menu::Item::new(kind.title(), Msg::NewTab(pane, *kind))
                            .glyph(tab_glyph(*kind))
                    })
                    .collect(),
            )
        },
    )
    .gap(2.0)
    .on_dismiss(Msg::TabMenu(None));

    container(
        row![
            opener,
            theme::vline(theme::LINE_SOFT),
            scrollable(strip)
                .direction(ScrollDirection::ScrollHorizontal)
                .scrollbar(theme::scrollbar())
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_drop(move |drop| Msg::TabDrop(pane, drop), false, theme::DROP_HINT)
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(TAB_STRIP_HEIGHT))
    .into()
}

/// Что вкладка может сделать с раскладкой: куда переехать и можно ли свести
/// панели обратно.
///
/// Направления предлагаются те, что изменят экран: перенести туда, где сосед
/// уже есть, — значит положить вкладку к нему; туда, где соседа нет, — значит
/// поделить экран этим переносом. Пункт, который ничего не сделает, обещает
/// выбор и молчит в ответ, поэтому его здесь нет вовсе (см. `State::can_move`).
fn arrangements(state: &State, id: ViewId) -> Vec<menu::Item> {
    let mut items = Vec::new();
    for &side in Side::ALL {
        if state.can_move(id, side) {
            items.push(
                menu::Item::new(side.title(), Msg::TabMove(id, side)).glyph(side_glyph(side)),
            );
        }
    }
    if state.layout().count() > 1 {
        items.push(menu::Item::new("Свести панели", Msg::TabCollapse).glyph(theme::glyph::SPLIT));
    }
    items
}

/// Меню самой вкладки: куда её деть. Закрытие последним пунктом — оно про
/// вкладку целиком, а не про её место.
fn tab_options(mut items: Vec<menu::Item>, id: ViewId) -> Vec<menu::Item> {
    items.push(menu::Item::new("Закрыть вкладку", Msg::TabClose(id)).glyph(theme::glyph::CLOSE));
    items
}

/// Знак направления переноса — та же стрелка, которой направление показано
/// везде ещё.
fn side_glyph(side: Side) -> &'static str {
    match side {
        Side::Left => theme::glyph::LEFT,
        Side::Right => theme::glyph::RIGHT,
        Side::Above => theme::glyph::UP,
        Side::Below => theme::glyph::DOWN,
    }
}

/// Глиф вкладки, которую предлагают открыть, — тот же, которым она подписана
/// потом (см. [`glyph`]).
fn tab_glyph(kind: NewTab) -> &'static str {
    match kind {
        NewTab::Browse => theme::glyph::FOLDER,
        NewTab::Search => theme::glyph::SEARCH,
        NewTab::Downloaded => theme::glyph::DOWNLOAD,
        NewTab::Globe => theme::glyph::GLOBE,
        NewTab::Shown => theme::glyph::LAYERS,
        NewTab::Empty => theme::glyph::PLUS,
    }
}

/// Строка состояния: что происходит прямо сейчас и сколько занято на диске.
/// Пустых мест в ней нет — то, о чём сказать нечего, не показывается вовсе.
fn status_bar(state: &State) -> Element<Msg> {
    let mut parts: Vec<Element<Msg>> = Vec::new();
    let label = |content: String, color: Color| {
        text::<Msg>(content).size(theme::TEXT_LABEL).color(color).single_line()
    };

    let (count, done, total) = state.library.downloading();
    if count > 0 {
        parts.push(
            label(
                format!("Скачивается {} {}", count, format::plural(count, ["файл", "файла", "файлов"])),
                theme::ACCENT_TEXT,
            )
                .weight(FontWeight::WeightMedium)
                .into(),
        );
        if total > 0 {
            parts.push(
                container(
                    progress_bar::<Msg>(0.0..=1.0, done as f32 / total as f32)
                        .style(theme::progress(theme::ACCENT))
                        .width(Length::Fixed(88.0))
                        .height(Length::Fixed(5.0)),
                )
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .into(),
            );
        }
        if state.speed > 0.0 {
            parts.push(label(format!("{}/с", format::bytes(state.speed as u64)), theme::INK_DIM).into());
        }
        parts.push(label("|".to_string(), theme::LINE).into());
    }

    parts.push(label(format!("на диске {}", format::bytes(state.library.stored())), theme::INK_DIM).into());
    parts.push(theme::spacer().into());
    // Извещение о только что не вышедшем старше отказа библиотеки: оно про то,
    // что человек сделал секунду назад, а места под обоих в полосе нет.
    if let Some(said) = state.notice.as_ref().or(state.error.as_ref()) {
        // Ужимаем: в обоих строках сидит сказанное отказавшим — путь папки,
        // причина от провайдера, — и длину его ничто не ограничивает. Полоса
        // идёт одной строкой, поэтому нетронутая причина не переносится, а
        // уезжает за край окна. Целиком она всегда в логе.
        parts.push(label(format::ellipsize(said, 72), theme::DANGER).into());
    }

    theme::chrome_bar(
        row(parts)
            .spacing(BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}

/// Глиф вида — тот же, которым подписан его пункт в меню открытия: вкладка и
/// пункт обозначают одно и то же (см. `ViewKind::opened_as`). Своего знака нет
/// только у просмотра снимка — его из меню и не открывают.
fn glyph(kind: &ViewKind) -> &'static str {
    kind.opened_as().map_or(theme::glyph::IMAGE, tab_glyph)
}
