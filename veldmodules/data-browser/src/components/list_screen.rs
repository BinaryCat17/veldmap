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
use crate::module::state::ViewId;
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
    /// Рычаги, которые есть только у этого вида, — над общей полосой отбора.
    /// Общее у трёх видов то, как они показывают строки; откуда строки берутся,
    /// у каждого своё, и спрашивают они об этом по-разному.
    pub controls: Option<Element<Msg>>,
    pub rows: Vec<Row>,
    /// Ключ снимка, выделенного на шаре; пусто — либо не выделен, либо этот вид
    /// к нему отношения не имеет (см. `table::Context::picked`).
    pub picked: &'a str,
}

/// `width` — ширина окна в логических точках: по ней считается, сколько знаков
/// имени влезает в свою колонку.
/// Заголовок экрана: название, подпись под ним и кнопки справа.
///
/// Роль, а не сборка на месте: заголовок стоит над каждым списком, и
/// написанный по разу на экран он расходится кеглем подписи, выключкой и
/// высотой — что и случилось, пока их было два.
pub fn heading(title: &str, subtitle: String, trailing: Vec<Element<Msg>>) -> Element<Msg> {
    let mut line = row![
        text::<Msg>(title.to_string())
            .size(theme::TEXT_TITLE)
            .color(theme::INK)
            .weight(FontWeight::WeightBold)
            .single_line(),
        text::<Msg>(subtitle).size(theme::TEXT_BODY).color(theme::INK_DIM).single_line(),
        // Кнопки прижимаются вправо; без них распорка ничего не меняет.
        container(veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(0.0)))
            .width(Length::Fill),
    ];
    for element in trailing {
        line = line.push(element);
    }

    container(
        line.spacing(10.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(theme::HEADING_HEIGHT))
    .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

pub fn view(
    view: ViewId,
    screen: Screen<'_>,
    listing: &ListingState,
    width: f32,
) -> Element<Msg> {
    let arranged = arrange::arrange(&screen.rows, listing);
    // Что поместится в отведённую ширину, решает сама таблица: половина
    // экрана вдвое уже окна, а колонки от этого не худеют (см. table::fit).
    // Колонка раскрытия нужна, только если есть что раскрывать.
    let twisty = screen.rows.iter().any(|row| !row.children.is_empty());
    let (columns, name_width) = table::fit(width, twisty);

    let heading = heading(screen.title, screen.subtitle, Vec::new());

    // Пустой список и список, из которого всё убрал отбор, — разные вещи:
    // «ничего не скачано» под выбранным фильтром было бы неправдой.
    let nothing = if screen.rows.is_empty() {
        screen.empty
    } else {
        "Под отбор ничего не подошло"
    };

    let body: Element<Msg> = if arranged.lines.is_empty() {
        theme::empty(nothing).into()
    } else {
        scrollable(table::body(
            view,
            &arranged.lines,
            table::Context {
                listing,
                columns: &columns,
                name_width,
                here: screen.path.unwrap_or_default(),
                picked: screen.picked,
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
        screen_rows.push(controls::path(view, path));
    }
    if let Some(own) = screen.controls {
        screen_rows.push(own);
    }
    // Складывать по папкам есть смысл там, где строки собраны отовсюду.
    // Наличие пути ровно это и означает: вид с путём показывает содержимое
    // одной папки, все его строки лежат в ней, и заголовок над ними был бы
    // ровно один. Второго признака под это заводить не нужно — этот уже есть.
    screen_rows.push(controls::toolbar(view, listing, &arranged.counts, screen.path.is_none()));
    screen_rows.push(table::header(&columns));
    screen_rows.push(body);
    if arranged.pages > 1 {
        screen_rows.push(theme::hairline(theme::LINE_SOFT));
        screen_rows.push(controls::pager(view, &arranged));
    }

    column(screen_rows).width(Length::Fill).height(Length::Fill).into()
}
