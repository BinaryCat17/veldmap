//! View рендеринг для data-browser
//!
//! Здесь каркас окна: панель открытия видов, полоса вкладок и тело активной.

pub mod browse;
pub mod search;
pub mod downloaded;
pub mod preview;

use crate::module::state::{State, ViewKind};
use crate::module::{styles, Msg};
use veld_ui_service_wrap::{column, row, container};
use crate::proto::ui_service::{Element, text, button, icon, scrollable, Length, Padding, Color, Alignment, ScrollDirection};

pub fn build_root(state: &State) -> Element<Msg> {
    let body: Element<Msg> = match state.active() {
        Some(ViewKind::Browse(browse)) => browse::view(state, browse),
        Some(ViewKind::Search(search)) => search::view(state, search),
        Some(ViewKind::Downloaded) => downloaded::view(state),
        Some(ViewKind::Preview(preview)) => preview::view(preview),
        // Закрыть последнюю вкладку — законное состояние, а не ошибка.
        None => container(text("No open tabs").size(16.0).color(styles::COLOR_TEXT_DIM))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .into(),
    };

    // Панель открытия: показывает уже открытый вид такого рода или заводит
    // его (см. handlers::nav). Превью здесь нет — оно открывается на файл.
    let toolbar = row![
        styles::apply_nav(button(text("Browse"))).on_press(Msg::NavBrowse),
        styles::apply_nav(button(text("Search"))).on_press(Msg::NavSearch),
        styles::apply_nav(button(text("Downloaded"))).on_press(Msg::NavDownloaded)
    ].spacing(10.0).padding(Padding::new(10.0));

    // Дети корневой колонки не именованы намеренно: они разного рода, и
    // подмену тела при смене вкладки ловит обычная колонка — по несовпадению
    // типа виджета (см. Widget.key в types.proto).
    let mut rows: Vec<Element<Msg>> = vec![toolbar.into(), tab_strip(state), body];
    // Строка состояния появляется, только когда есть что сказать: пустая
    // занимала бы место под сообщение, которого нет.
    if let Some(error) = &state.error {
        rows.push(
            container(text(error).size(14.0).color(styles::COLOR_DANGER))
                .padding(Padding::new(5.0))
                .into(),
        );
    }

    // Тёмный фон окна: корневой Container (#1e1e24)
    container(
        column(rows)
            .width(Length::Fill)
            .height(Length::Fill)
    )
    .background(Color::from_rgb(0.118, 0.118, 0.141))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Высота полосы вкладок. Фиксирована, а не выведена из содержимого: полоса —
/// хром постоянного размера, и ни одна вкладка не вправе растянуть её собой.
const TAB_STRIP_HEIGHT: f32 = 44.0;

/// Полоса вкладок. Вкладка адресуется своим `ViewId`, а не позицией в полосе:
/// позиция меняется, когда закрывают соседа.
///
/// Вкладки не сжимаются, а уезжают под горизонтальную прокрутку: сжатие
/// доводит подпись до нулевой ширины, а такой текст занимает высоту, а не
/// ширину (см. `Wrapping` в types.proto).
fn tab_strip(state: &State) -> Element<Msg> {
    if state.views().is_empty() {
        return row![].into();
    }

    let tabs = state.views().iter().map(|view| {
        let active = Some(view.id) == state.active_id();
        let label = row![
            icon(glyph(&view.kind)),
            text(view.kind.title()).single_line(),
        ].spacing(6.0).align_items(Alignment::Center);

        row![
            styles::apply_tab(button(label), active).on_press(Msg::TabSelect(view.id)),
            styles::apply_tab(button(icon("\u{f00d}")), active).on_press(Msg::TabClose(view.id)),
        ].spacing(1.0).align_items(Alignment::Center).into()
    });

    let strip = row(tabs)
        .spacing(4.0)
        .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
        .align_items(Alignment::Center);

    container(
        scrollable(strip)
            .direction(ScrollDirection::ScrollHorizontal)
            .width(Length::Fill)
            .height(Length::Fill)
    )
    .width(Length::Fill)
    .height(Length::Fixed(TAB_STRIP_HEIGHT))
    .into()
}

/// Глиф вида — тот же, которым он подписан в списках: папка у Browse, лупа у
/// поиска, картинка у превью.
fn glyph(kind: &ViewKind) -> &'static str {
    match kind {
        ViewKind::Search(_) => "\u{f002}",
        ViewKind::Browse(_) => "\u{f07b}",
        ViewKind::Downloaded => "\u{f019}",
        ViewKind::Preview(_) => "\u{f03e}",
    }
}
