//! View рендеринг для data-browser.
//!
//! Здесь каркас окна: половина (или две) со своей полосой вкладок и телом
//! активной в ней вкладки, а под ними — общая строка состояния. Содержимое
//! вкладки собирают модули рядом.
//!
//! Строка состояния одна на окно, а не на половину: она говорит о том, что
//! делает приложение целиком — идут ли закачки и сколько занято на диске, — и
//! к тому, на что смотрят в этой половине, отношения не имеет.

pub mod browse;
pub mod downloaded;
pub mod globe;
pub mod preview;
pub mod search;
pub mod shown;

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, popover, progress_bar, space, text, Alignment, Color, Element,
    FontWeight, Length, Padding, ScrollDirection, scrollable,
};
use crate::module::components::{format, menu};
use crate::module::state::{Half, Placement, State, ViewId, ViewKind};
use crate::module::{theme, Msg, NewTab};


/// Высота полосы вкладок. Фиксирована, а не выведена из содержимого: это хром
/// постоянного размера, и ни одна вкладка не вправе растянуть его собой.
/// Строка состояния — обычная полоса хрома (theme::BAR_HEIGHT).
const TAB_STRIP_HEIGHT: f32 = 38.0;

/// Зазор между тем, что стоит в полосе хрома. Один на обе полосы: они читаются
/// как один язык, и разный зазор в них видно.
pub const BAR_SPACING: f32 = 10.0;

pub fn build_root(state: &State) -> Element<Msg> {
    // Половины равны: делить экран пополам и есть то, о чём просили, а тянуть
    // границу мышью нечем — своего виджета под неё в разметке нет.
    //
    // Неразделённый экран собирается той же парой, что и разделённый вправо,
    // только вторая половина нулевой ширины. Форма дерева от этого не зависит
    // от разделения, а состояние виджетов (прокрутка, каретка, наведение)
    // рендерер сопоставляет по месту в дереве: собери первую половину то
    // корнем, то ребёнком строки — и список, который читали, прыгнет в начало
    // ровно в тот момент, когда экран делят, чтобы не терять его из виду.
    let screen: Element<Msg> = match state.split() {
        None => beside(pane(state, Half::First), None),
        Some(Placement::Right) => beside(pane(state, Half::First), Some(pane(state, Half::Second))),
        Some(Placement::Left) => beside(pane(state, Half::Second), Some(pane(state, Half::First))),
        Some(Placement::Below) => column![
            container(pane(state, Half::First)).width(Length::Fill).height(Length::FillPortion(1)),
            theme::hairline(theme::LINE),
            container(pane(state, Half::Second)).width(Length::Fill).height(Length::FillPortion(1)),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    };

    container(
        column![
            container(screen).width(Length::Fill).height(Length::Fill),
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

/// Две половины бок о бок. `None` справа — неразделённый экран: место второй
/// половины остаётся в дереве, но нулевой ширины, чтобы форма не зависела от
/// разделения (см. `build_root`). Разделитель едет вместе с правой половиной,
/// а не отдельным ребёнком, — тогда он исчезает с ней заодно.
fn beside(left: Element<Msg>, right: Option<Element<Msg>>) -> Element<Msg> {
    // Ширину строке-обёртке задаём явно: `Shrink` по умолчанию свёл бы
    // `Fill`-половину внутри себя в ноль.
    let (width, right) = match right {
        Some(right) => (
            Length::FillPortion(1),
            row![theme::vline(theme::LINE), container(right).width(Length::Fill).height(Length::Fill)]
                .width(Length::Fill)
                .height(Length::Fill),
        ),
        None => (Length::Fixed(0.0), row![].width(Length::Fill).height(Length::Fill)),
    };
    row![
        container(left).width(Length::FillPortion(1)).height(Length::Fill),
        container(right).width(width).height(Length::Fill).clip(),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Одна половина: своя полоса вкладок и тело её активной вкладки.
fn pane(state: &State, half: Half) -> Element<Msg> {
    let body: Element<Msg> = match state.active_in(half).and_then(|id| state.get(id).map(|kind| (id, kind))) {
        Some((id, ViewKind::Browse(view))) => browse::view(state, id, view),
        Some((id, ViewKind::Search(view))) => search::view(state, id, view),
        Some((id, ViewKind::Downloaded(listing))) => downloaded::view(state, id, listing),
        Some((id, ViewKind::Preview(view))) => preview::view(state, id, view),
        Some((id, ViewKind::Globe(view))) => globe::view(state, id, view),
        Some((id, ViewKind::Shown)) => shown::view(state, id),
        None => empty_pane(state, half),
    };

    column![
        tab_strip(state, half),
        theme::hairline(theme::LINE),
        container(body).width(Length::Fill).height(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Пустая половина. Не ошибка и не «всё закрыто»: половина ждёт, что в неё
/// положат, и спрашивает об этом прямо — списком того, что бывает, а не пустым
/// местом с крестиком где-то в углу.
fn empty_pane(state: &State, half: Half) -> Element<Msg> {
    let split = state.split().is_some();
    let title = match split {
        true => "Что показать в этой половине",
        false => "Все вкладки закрыты",
    };
    let hint = match split {
        true => "Половина пустая. Выберите вкладку — она встанет рядом.",
        false => "Выберите, с чего начать.",
    };

    let choices = NewTab::ALL.iter().map(|kind| {
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
        .on_press(Msg::NewTab(half, *kind))
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

/// Полоса вкладок одной половины. Вкладка адресуется своим `ViewId`, а не
/// позицией: позиция меняется, когда закрывают соседа.
///
/// Вкладки не сжимаются, а уезжают под горизонтальную прокрутку: сжатие
/// доводит подпись до нулевой ширины, а такой текст занимает высоту, а не
/// ширину (см. `Wrapping` в types.proto).
fn tab_strip(state: &State, half: Half) -> Element<Msg> {
    let active = state.active_in(half);
    let tabs = state.views_in(half).map(|view| {
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
        let options = state.tab_options == Some(view.id);
        let tab = theme::tab(
            row![
                label,
                popover(
                    theme::chrome_icon(
                        icon::<Msg>(theme::glyph::SPLIT).size(9.0).color(theme::INK_FAINT),
                    )
                    .on_press(Msg::TabOptions(if options { None } else { Some(view.id) })),
                    menu::panel(tab_options(state, view.id)),
                )
                .open(options)
                .gap(4.0)
                .on_dismiss(Msg::TabOptions(None)),
                theme::chrome_icon(icon::<Msg>(theme::glyph::CLOSE).size(9.0).color(theme::INK_FAINT))
                    .on_press(Msg::TabClose(view.id)),
            ]
            .spacing(4.0)
            .align_items(Alignment::Center),
            current,
        )
        .height(Length::Fill)
        .on_press(Msg::TabSelect(view.id));

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

    let open = state.tab_menu == Some(half);
    let opener = popover(
        theme::chrome_icon(icon::<Msg>(theme::glyph::PLUS).size(11.0).color(theme::INK_DIM))
            .width(Length::Fixed(TAB_STRIP_HEIGHT))
            .height(Length::Fill)
            .on_press(Msg::TabMenu(if open { None } else { Some(half) })),
        menu::panel(
            NewTab::ALL
                .iter()
                .map(|kind| {
                    menu::Item::new(kind.title(), Msg::NewTab(half, *kind)).glyph(tab_glyph(*kind))
                })
                .collect(),
        ),
    )
    .open(open)
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
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(TAB_STRIP_HEIGHT))
    .into()
}

/// Меню самой вкладки: куда её деть. Разделение предлагается, пока экран не
/// разделён; разделённому предлагается перенос и обратное сведение — второго
/// разделения не бывает, а пункт, который ничего не сделает, обещает выбор и
/// молчит в ответ.
fn tab_options(state: &State, id: ViewId) -> Vec<menu::Item> {
    let mut items = Vec::new();
    match state.split() {
        None => {
            for placement in Placement::ALL {
                items.push(
                    menu::Item::new(placement.title(), Msg::TabSplit(id, placement))
                        .glyph(theme::glyph::SPLIT),
                );
            }
        }
        Some(_) => {
            items.push(
                menu::Item::new("Перенести в другую половину", Msg::TabMove(id))
                    .glyph(theme::glyph::ENTER),
            );
            items.push(
                menu::Item::new("Свести половины", Msg::TabUnsplit).glyph(theme::glyph::CLOSE),
            );
        }
    }
    items.push(menu::Item::new("Закрыть вкладку", Msg::TabClose(id)).glyph(theme::glyph::CLOSE));
    items
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
    if let Some(error) = &state.error {
        parts.push(label(error.clone(), theme::DANGER).into());
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
