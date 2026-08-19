//! components/list_screen.rs — экран со списком целиком.
//!
//! Три вида отличаются заголовком, наличием пути и тем, откуда взялись строки.
//! Всё остальное — отбор, колонки, кнопки, страницы — у них общее, и живёт
//! здесь: разойтись им тогда негде.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, scrollable, text, Alignment, Element, FontWeight, Length, Padding,
    ScrollDirection, ScrollTo,
};
use crate::module::components::{arrange, controls, format, table, Row};
use crate::module::state::ViewId;
use crate::module::state::listing::{ListingState, Menu};
use crate::module::{theme, Msg, ViewMsg};

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
    /// Ключ снимка, выбранного щелчком по шару; пусто — не выбран ни один.
    /// Один на все списки: выбирают на шаре, а видно это должно быть везде, где
    /// этот снимок стоит строкой (см. `table::Context::picked`).
    pub picked: &'a str,
    /// Сколько отмеченного здесь очерчено на шаре. Считает его состояние
    /// модуля (`State::outlined_in`), а не сам список: отметка — намерение, а
    /// контур — то, что из него вышло, и совпадают они не всегда.
    pub outlined: usize,
    /// Раскрытое меню этого списка (см. `table::Context::menu`).
    pub menu: Option<&'a Menu>,
    /// Строка, к которой привёл переход в эту вкладку; пусто — переход был не
    /// сюда (см. `state::Highlight`).
    pub target: &'a str,
}

/// Сколько знаков подзаголовка помещается в строку заголовка. Числом, а не
/// шириной: подзаголовок стои́т между названием и кнопками, и ширина у него
/// та, что осталась от обоих, — а обрезать его должен клиент, потому что
/// коробка срезала бы вместо него кнопки.
const SUBTITLE_CHARS: usize = 72;

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
        // Подзаголовком приезжает и текст ошибки от провайдера — строка
        // произвольной длины. Нетронутая, она стои́т в строке перед распоркой и
        // выдавливает за край кнопки заголовка («Снять отметки»), а коробка их
        // потом срезает: длинная сетевая ошибка делала бы их ненажимаемыми.
        text::<Msg>(format::ellipsize(&subtitle, SUBTITLE_CHARS))
            .size(theme::TEXT_BODY)
            .color(theme::INK_DIM)
            .single_line(),
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

/// Куда прокрутить список, чтобы строка перехода оказалась на виду. `None` —
/// вести не к чему либо строки на этой странице нет.
///
/// Считается в строках, а не в пикселях от рендерера: шаг строки — наш
/// (`theme::ROW_PITCH`), и мерить его умеем только мы. Пара строк оставляется
/// сверху: строка, прижатая к верхнему краю, читается как начало списка, а не
/// как место, к которому привели.
///
/// Номер просьбы едет вместе со смещением: по одному смещению рендереру не
/// отличить «просят снова» от «просят то же самое» (см. `ListingState::aim`).
fn aim(arranged: &arrange::Arranged<'_>, listing: &ListingState, target: &str) -> Option<ScrollTo> {
    if target.is_empty() {
        return None;
    }
    let at = arranged.lines.iter().position(|line| match line {
        arrange::Line::Entry { row, .. } => row.named(target),
        arrange::Line::Group { .. } | arrange::Line::Waiting { .. } => false,
    })?;
    Some(ScrollTo {
        offset: at.saturating_sub(2) as f32 * theme::ROW_PITCH,
        request: listing.aim,
    })
}

/// Экран списка целиком.
///
/// `width` — ширина ПАНЕЛИ в логических точках, а не окна: по ней считается,
/// какие колонки помещаются и сколько знаков имени влезает в свою колонку.
/// Окно шире, и посчитанное по нему схлопнуло бы имя в ноль (см.
/// `State::pane_width` и [`table::fit`]).
pub fn view(
    view: ViewId,
    screen: Screen<'_>,
    listing: &ListingState,
    width: f32,
) -> Element<Msg> {
    let arranged = arrange::arrange(&screen.rows, listing);
    // Что поместится в отведённую ширину, решает сама таблица: половина
    // экрана вдвое уже окна, а колонки от этого не худеют (см. table::fit).
    //
    // Необязательные колонки нужны, только если есть что раскрывать и что
    // отмечать, — и считаются они по нарисованному, а не по верхнему ярусу:
    // снимки в каталоге лежат под папкой дня, то есть появляются только в
    // раскрытой строке. Считай мы по верхнему ярусу — колонка отметки ушла бы
    // целиком, и очертить лениво раскрытый снимок стало бы нечем, хотя он на
    // экране (см. `Arranged::marks`).
    let optional = table::Optional {
        twisty: arranged.shown().any(Row::expandable),
        checkable: arranged.marks().next().is_some(),
    };
    let table::Fit { columns, name: name_width, compact } = table::fit(width, optional);

    // Сколько снимков этого списка очерчено на шаре — и чем это снять. Без
    // такой подписи коробочки говорят только «отмечено», а отмечено-то ради
    // контура (см. handlers::outline).
    //
    // Считается по нарисованному, а не по отмеченному: у снимка без геометрии
    // контура не бывает вовсе, а пока продукт спрашивают у каталога — ещё нет,
    // и «1 на шаре» над пустым шаром было бы неправдой.
    let mut trailing: Vec<Element<Msg>> = Vec::new();
    if !listing.selected.is_empty() {
        let marked = listing.selected.len();
        let counts = match screen.outlined == marked {
            true => format!("{} на шаре", marked),
            false => format!("{} на шаре из {}", screen.outlined, marked),
        };
        trailing.push(
            text::<Msg>(counts)
                .size(theme::TEXT_LABEL)
                .color(theme::ACCENT_TEXT)
                .single_line()
                .into(),
        );
        trailing.push(
            theme::bar_button("Снять отметки")
                .on_press(Msg::In(view, ViewMsg::CheckClear))
                .into(),
        );
    }
    let heading = heading(screen.title, screen.subtitle, trailing);

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
                menu: screen.menu,
                columns: &columns,
                name_width,
                compact,
                shared: format::shared(
                    &arranged.shown().map(|row| row.title.as_str()).collect::<Vec<&str>>(),
                    format::mono_fit(name_width, theme::TEXT_MONO),
                ),
                here: screen.path.unwrap_or_default(),
                picked: screen.picked,
                target: screen.target,
                now: format::now(),
            },
        ))
            .direction(ScrollDirection::ScrollVertical)
            .scrollbar(theme::scrollbar())
            // Имя — своё у каждой вкладки: наводка адресуется им, а списков на
            // экране бывает два.
            .name(format!("rows:{}", view))
            .scroll_to(aim(&arranged, listing, screen.target))
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
    screen_rows.push(controls::toolbar(
        view, listing, screen.menu, &arranged.counts, screen.path.is_none(), width));
    screen_rows.push(table::header(view, &columns, arranged.all_marked(listing), compact));
    screen_rows.push(body);
    if arranged.pages > 1 {
        screen_rows.push(theme::hairline(theme::LINE_SOFT));
        screen_rows.push(controls::pager(view, &arranged, width));
    }

    column(screen_rows).width(Length::Fill).height(Length::Fill).into()
}
