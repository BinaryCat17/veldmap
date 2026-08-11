//! components/list_screen.rs — экран со списком целиком.
//!
//! Три вида отличаются заголовком, наличием пути и тем, откуда взялись строки.
//! Всё остальное — отбор, колонки, кнопки, страницы — у них общее, и живёт
//! здесь: разойтись им тогда негде.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, scrollable, text, Alignment, Element, FontWeight, Length, Padding, ScrollDirection,
};
use crate::module::components::{arrange, controls, format, table, Row};
use crate::module::state::listing::ListingState;
use crate::module::{theme, Msg};

/// Чем экран подписан и что показывает.
pub struct Screen<'a> {
    pub title: &'a str,
    /// Строка под заголовком: сколько всего и откуда.
    pub subtitle: String,
    /// Путь текущей папки; пусто — пути у вида нет. Он же признак «показана
    /// одна папка»: строки такого вида все лежат в ней, и это меняет не только
    /// заголовок, но и то, какие рычаги к списку имеют смысл.
    pub path: Option<&'a str>,
    /// Что показать вместо таблицы, когда строк нет: у пустого поиска и у
    /// пустой папки причины разные.
    pub empty: &'a str,
    pub rows: Vec<Row>,
}

/// `width` — ширина окна в логических точках: по ней считается, сколько знаков
/// имени влезает в свою колонку.
pub fn view(screen: Screen<'_>, listing: &ListingState, width: f32) -> Element<Msg> {
    let arranged = arrange::arrange(&screen.rows, listing);
    let counts = arrange::counts(&screen.rows, listing);
    let name_width = (width - table::FIXED_WIDTH).max(120.0);

    let heading = row![
        text::<Msg>(screen.title.to_string())
            .size(theme::TEXT_TITLE)
            .color(theme::INK)
            .weight(FontWeight::WeightBold)
            .single_line(),
        text::<Msg>(screen.subtitle).size(theme::TEXT_BODY).color(theme::INK_DIM).single_line(),
    ]
    .spacing(10.0)
    .align_items(Alignment::End)
    .padding(Padding { top: 13.0, bottom: 9.0, left: theme::GUTTER, right: theme::GUTTER });

    // Пустой список и список, из которого всё убрал отбор, — разные вещи:
    // «ничего не скачано» под выбранным фильтром было бы неправдой.
    let nothing = if screen.rows.is_empty() {
        screen.empty
    } else {
        "Под отбор ничего не подошло"
    };

    let body: Element<Msg> = if arranged.lines.is_empty() {
        container(text::<Msg>(nothing.to_string()).size(theme::TEXT_BODY).color(theme::INK_DIM))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .padding(Padding::new(40.0))
            .into()
    } else {
        scrollable(table::body(
            &arranged.lines,
            table::Context {
                listing,
                name_width,
                here: screen.path.unwrap_or_default(),
                now: format::now(),
            },
        ))
            .direction(ScrollDirection::ScrollVertical)
            .scrollbar(theme::scrollbar())
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    let mut screen_rows: Vec<Element<Msg>> = vec![heading.into()];
    if let Some(path) = screen.path {
        screen_rows.push(controls::path(path));
    }
    // Складывать по папкам есть смысл там, где строки собраны отовсюду.
    // Наличие пути ровно это и означает: вид с путём показывает содержимое
    // одной папки, все его строки лежат в ней, и заголовок над ними был бы
    // ровно один. Второго признака под это заводить не нужно — этот уже есть.
    screen_rows.push(controls::toolbar(listing, &counts, screen.path.is_none()));
    screen_rows.push(table::header());
    screen_rows.push(body);
    if arranged.pages > 1 {
        screen_rows.push(theme::hairline(theme::LINE_SOFT));
        screen_rows.push(controls::pager(&arranged));
    }

    column(screen_rows).width(Length::Fill).height(Length::Fill).into()
}
