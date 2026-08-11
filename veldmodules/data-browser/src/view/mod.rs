//! View рендеринг для data-browser.
//!
//! Здесь каркас окна: полоса вкладок сверху, тело активной вкладки и строка
//! состояния снизу. Содержимое вкладки собирают модули рядом.

pub mod browse;
pub mod downloaded;
pub mod globe;
pub mod preview;
pub mod search;

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, popover, progress_bar, space, text, Alignment, Color, Element,
    FontWeight, Length, Padding, ScrollDirection, scrollable,
};
use crate::module::components::{format, menu};
use crate::module::state::{State, ViewKind};
use crate::module::{theme, Msg};

const GLYPH_PLUS: &str = "\u{f067}";
const GLYPH_CLOSE: &str = "\u{f00d}";
const GLYPH_FOLDER: &str = "\u{f07b}";
const GLYPH_SEARCH: &str = "\u{f002}";
const GLYPH_DOWNLOAD: &str = "\u{f019}";
const GLYPH_IMAGE: &str = "\u{f1c5}";
const GLYPH_GLOBE: &str = "\u{f0ac}";

/// Высота полосы вкладок и строки состояния. Фиксированы, а не выведены из
/// содержимого: это хром постоянного размера, и ни одна вкладка не вправе
/// растянуть его собой.
const TAB_STRIP_HEIGHT: f32 = 38.0;
const STATUS_HEIGHT: f32 = 30.0;

pub fn build_root(state: &State) -> Element<Msg> {
    let body: Element<Msg> = match state.active() {
        Some(ViewKind::Browse(view)) => browse::view(state, view),
        Some(ViewKind::Search(view)) => search::view(state, view),
        Some(ViewKind::Downloaded(listing)) => downloaded::view(state, listing),
        Some(ViewKind::Preview(view)) => preview::view(state, view),
        Some(ViewKind::Globe(view)) => globe::view(state, view),
        // Закрыть последнюю вкладку — законное состояние, а не ошибка.
        None => container(text::<Msg>("Все вкладки закрыты".to_string()).size(theme::TEXT_BODY).color(theme::INK_DIM))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .into(),
    };

    container(
        column![
            tab_strip(state),
            theme::hairline(theme::LINE),
            container(body).width(Length::Fill).height(Length::Fill),
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

/// Полоса вкладок. Вкладка адресуется своим `ViewId`, а не позицией: позиция
/// меняется, когда закрывают соседа.
///
/// Вкладки не сжимаются, а уезжают под горизонтальную прокрутку: сжатие
/// доводит подпись до нулевой ширины, а такой текст занимает высоту, а не
/// ширину (см. `Wrapping` в types.proto).
fn tab_strip(state: &State) -> Element<Msg> {
    let tabs = state.views().iter().map(|view| {
        let active = Some(view.id) == state.active_id();
        let label = row![
            icon::<Msg>(glyph(&view.kind))
                .size(10.0)
                .color(if active { theme::ACCENT } else { theme::INK_FAINT }),
            text::<Msg>(view.kind.title()).size(theme::TEXT_BODY).single_line(),
        ]
        .spacing(7.0)
        .align_items(Alignment::Center);

        // Крестик — отдельная кнопка внутри вкладки: нажатие на неё до самой
        // вкладки не доходит, поэтому «закрыть» не выбирает заодно и вкладку.
        let tab = theme::tab(
            row![
                label,
                theme::chrome_icon(icon::<Msg>(GLYPH_CLOSE).size(9.0).color(theme::INK_FAINT))
                    .on_press(Msg::TabClose(view.id)),
            ]
            .spacing(4.0)
            .align_items(Alignment::Center),
            active,
        )
        .height(Length::Fill)
        .on_press(Msg::TabSelect(view.id));

        // Подчёркивание активной: рамка в этом протоколе одна на все стороны,
        // а нужна только нижняя.
        column![
            container(tab).height(Length::Fill).width(Length::Shrink),
            theme::hairline(if active { theme::ACCENT } else { Color::TRANSPARENT }),
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

    let opener = popover(
        theme::chrome_icon(icon::<Msg>(GLYPH_PLUS).size(11.0).color(theme::INK_DIM))
            .width(Length::Fixed(TAB_STRIP_HEIGHT))
            .height(Length::Fill)
            .on_press(Msg::TabMenu(!state.tab_menu)),
        menu::panel(vec![
            menu::Item::new("Сетевой каталог", Msg::NewBrowse).glyph(GLYPH_FOLDER),
            menu::Item::new("Поиск снимков", Msg::NewSearch).glyph(GLYPH_SEARCH),
            menu::Item::new("Скачанное", Msg::NewDownloaded).glyph(GLYPH_DOWNLOAD),
            menu::Item::new("Глобус", Msg::NewGlobe).glyph(GLYPH_GLOBE),
        ]),
    )
    .open(state.tab_menu)
    .gap(2.0)
    .on_dismiss(Msg::TabMenu(false));

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
    parts.push(container(space::<Msg>(Length::Fill, Length::Fixed(0.0))).width(Length::Fill).into());
    if let Some(error) = &state.error {
        parts.push(label(error.clone(), theme::DANGER).into());
    }

    container(
        row(parts)
            .spacing(12.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left: 14.0, right: 14.0 }),
    )
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(STATUS_HEIGHT))
    .into()
}

/// Глиф вида — тот же, которым он подписан в меню открытия.
fn glyph(kind: &ViewKind) -> &'static str {
    match kind {
        ViewKind::Search(_) => GLYPH_SEARCH,
        ViewKind::Browse(_) => GLYPH_FOLDER,
        ViewKind::Downloaded(_) => GLYPH_DOWNLOAD,
        ViewKind::Preview(_) => GLYPH_IMAGE,
        ViewKind::Globe(_) => GLYPH_GLOBE,
    }
}
