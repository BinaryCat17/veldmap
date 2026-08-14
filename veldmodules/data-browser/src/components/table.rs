//! components/table.rs — таблица списка: шапка, строки, меню строки.
//!
//! Одна на все три вида. Что можно сделать со строкой, выводится из неё самой,
//! а не из вида: папку открывают там, где она есть, скачанное смотрят там, где
//! оно скачано, — и вкладке нечего к этому добавить. Поэтому и колонки, и
//! кнопки во всех трёх видах одни и те же.

use veld_ui_service_wrap::{column, row, Keyed, Row as RowBuilder};
use crate::proto::ui_service::{
    container, icon, mono, popover, progress_bar, text, tooltip,
    Alignment, Color, Container, Element, FontWeight, Length, Padding, TooltipPosition,
};
use crate::module::components::{arrange::Line, format, Row, RowKind, RowStatus};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Колонка таблицы. Перечислением, а не списком ширин: колонок столько же,
/// сколько ячеек в строке, и связывать одно с другим позицией в массиве значит
/// однажды сдвинуть ячейку на соседнюю колонку.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Треугольник раскрытия у строки-снимка. Своя колонка, а не значок внутри
    /// имени: раскрытие — действие, и нажимать его надо мимо самой строки,
    /// иначе оно спорит с её главным действием.
    Twist,
    Icon,
    Name,
    Format,
    Date,
    Size,
    Status,
    Progress,
    Actions,
}

/// Все колонки в порядке показа. Один список на всю таблицу: сетка у шапки,
/// заголовка группы и строки обязана совпадать, а три её копии рано или поздно
/// разъедутся.
const COLUMNS: [Column; 9] = [
    Column::Twist,
    Column::Icon,
    Column::Name,
    Column::Format,
    Column::Date,
    Column::Size,
    Column::Status,
    Column::Progress,
    Column::Actions,
];

/// В каком порядке колонки уступают место, когда его мало. Первой уходит та,
/// без которой список читается легче всего: полоса загрузки дублирует
/// состояние, дата и размер — справка, а не действие.
///
/// Имени, значка и кнопок в этом списке нет: имя единственное тянущееся, и
/// сжатие доводит его до нулевой ширины (см. `Wrapping` в types.proto), а
/// строка без имени и без действий — не строка.
const DROP_ORDER: [Column; 5] =
    [Column::Progress, Column::Date, Column::Size, Column::Status, Column::Format];

/// Треугольник раскрытия — узкая колонка перед значком.
const TWIST: f32 = 22.0;
const ICON: f32 = 34.0;
/// Под тип продукта, а не под расширение файла: у радара он длинный
/// (`IW_GRDH_1SDV`), и по трём буквам «png» такую колонку не смеришь.
const FORMAT: f32 = 80.0;
const DATE: f32 = 88.0;
const SIZE: f32 = 84.0;
const STATUS: f32 = 104.0;
const PROGRESS: f32 = 132.0;
/// Сколько кнопок помещается в колонку действий. Больше всего их у снимка,
/// лежащего папкой: зайти внутрь, посмотреть, положить на шар и меню.
const ACTION_BUTTONS: f32 = 4.0;

/// Ширина колонки действий. Считается по числу кнопок, а не подбирается на
/// глаз, — приписанная кнопка иначе молча уедет под обрезку ячейки (см. `grid`).
const ACTIONS: f32 =
    theme::ROW_BUTTON * ACTION_BUTTONS + BUTTON_GAP * (ACTION_BUTTONS - 1.0) + CELL_PADDING * 2.0;

/// Сколько места занимает колонка. У имени своей ширины нет — оно забирает
/// остаток.
fn width_of(column: Column) -> f32 {
    match column {
        Column::Twist => TWIST,
        Column::Icon => ICON,
        Column::Name => 0.0,
        Column::Format => FORMAT,
        Column::Date => DATE,
        Column::Size => SIZE,
        Column::Status => STATUS,
        Column::Progress => PROGRESS,
        Column::Actions => ACTIONS,
    }
}

/// Сколько знаков имени помещается — то, ради чего вообще считаются ширины.
/// Ниже этого колонка имени не сжимается: вместо неё уходит соседняя.
const NAME_MIN: f32 = 150.0;

/// Зазор между кнопками строки.
pub const BUTTON_GAP: f32 = 6.0;

/// Сколько места занимает колонка действий целиком — три кнопки со своими
/// зазорами и полями ячейки. Наружу затем, что такой же ряд кнопок стоит в
/// списке слоёв, и вторая запись этой арифметики разошлась бы с первой.
pub const ACTIONS_WIDTH: f32 = ACTIONS;

/// Отступ содержимого ячейки от её границ — по обе стороны.
const CELL_PADDING: f32 = 8.0;

/// Какие колонки показывать в отведённой ширине и сколько достанется имени.
///
/// Половина экрана вдвое уже окна, а фиксированные колонки от этого не
/// худеют — в узком месте их сумма перекрывает всё место, и тянущееся имя
/// схлопывается в ноль. Поэтому лишние не сжимаются, а уходят: список без
/// даты читается, список без имени — нет.
pub fn fit(width: f32, twisty: bool) -> (Vec<Column>, f32) {
    let mut columns: Vec<Column> = COLUMNS.to_vec();
    // Раскрывать нечего — и колонки под это нет: пустой столбец сдвигал бы
    // весь список ради того, чего в нём не бывает.
    if !twisty {
        columns.retain(|column| *column != Column::Twist);
    }
    // Отступы экрана и собственные поля колонки имени тратятся всегда.
    let overhead = theme::GUTTER * 2.0 + CELL_PADDING * 2.0;
    let taken = |columns: &[Column]| -> f32 {
        columns.iter().map(|column| width_of(*column)).sum::<f32>() + overhead
    };

    for dropped in DROP_ORDER {
        if width - taken(&columns) >= NAME_MIN {
            break;
        }
        columns.retain(|column| *column != dropped);
    }
    let name = (width - taken(&columns)).max(NAME_MIN);
    (columns, name)
}

/// Ступенька группировки.
const INDENT: f32 = 12.0;

/// Высота шапки — своя: в ней подписи, а не строки данных.
const HEADER_HEIGHT: f32 = 22.0;

/// Расширения, у которых есть превью, — по ним же выбрана иконка строки.
const IMAGE_FORMATS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "jp2", "webp"];

/// Строка сетки: ячейка на каждую показываемую колонку. Ячейка обрезает
/// содержимое по своим границам — длинное имя иначе рисуется поверх соседней
/// колонки (см. `Wrapping` в types.proto).
///
/// `indent` — ступень группировки. Она расширяет первую колонку, а не сдвигает
/// иконку внутри неё: в колонке иконки ровно иконка и есть, и ступень, положенная
/// внутрь, упирается в её край — второй ярус обрезается, третий и дальше стоят
/// на одном месте. Расширение забирает колонка имени (она единственная
/// тянущаяся), поэтому едут вправо и иконка, и подпись, а всё правее остаётся
/// выровненным по своим местам.
///
/// Высоту ставит вызывающий: у шапки она своя.
fn grid(
    cells: impl Fn(Column) -> Element<Msg>,
    columns: &[Column],
    indent: f32,
) -> RowBuilder<Msg> {
    // Ступень группировки расширяет первую показанную колонку, какой бы она ни
    // была: колонка раскрытия есть не всегда, и привязка ступени к ней роняла
    // бы отступ группы там, где раскрывать нечего.
    let first = columns.first().copied();
    row(columns.iter().map(|column| {
        let step = if Some(*column) == first { indent } else { 0.0 };
        let (width, left) = match column {
            Column::Name => (Length::Fill, CELL_PADDING),
            other => (Length::Fixed(width_of(*other) + step), CELL_PADDING + step),
        };
        container(cells(*column))
            .width(width)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left, right: CELL_PADDING })
            .clip()
            .into()
    }))
    .width(Length::Fill)
}

/// Отступ сетки от краёв экрана. У строки его ставит сама кнопка строки
/// (`theme::row_button`) — она обязана быть нажимаемой и на полях, — поэтому
/// здесь он назначается только тем, кто не кнопка: шапке и заголовку группы.
/// Дважды назначенный отступ сужает колонки, а обрезка имени про это не знает
/// и рисует его поверх соседней.
fn gutters<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
}

/// Всё, что строке нужно знать помимо себя самой. Одним значением, а не
/// четырьмя параметрами на каждую функцию: список того, что «нужно знать»,
/// растёт, а сигнатуры от этого читаться не перестают.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub listing: &'a ListingState,
    /// Колонки, которые помещаются в отведённую ширину, — их и рисуем
    /// (см. [`fit`]). Набор один на всю таблицу: шапка, заголовок группы и
    /// строка обязаны совпасть по сетке.
    pub columns: &'a [Column],
    /// Ширина колонки имени в точках разметки.
    pub name_width: f32,
    /// Папка, которую сейчас показывают; пусто — вид её не знает (скачанное,
    /// поиск). По ней видно, куда вести «показать в каталоге» бессмысленно.
    pub here: &'a str,
    /// Ключ снимка, чей контур выделен на шаре; пусто — не выделен ни один.
    /// Живёт он в состоянии приложения (`State::shown`), а не вида: выделяют на
    /// шаре, а видно это должно быть в списке, из которого снимок родом.
    pub picked: &'a str,
    /// Точка отсчёта для подписей времени. Одна на всю таблицу: строки одного
    /// кадра должны мериться от одного «сейчас», иначе соседние даты
    /// перескакивают через полночь по-разному.
    pub now: i64,
}

/// Шапка. Стоит вне прокрутки, поэтому не уезжает вместе со строками.
pub fn header(columns: &[Column]) -> Element<Msg> {
    let label = |name: &str| {
        text::<Msg>(name.to_string())
            .size(theme::TEXT_HEADER)
            .color(theme::INK_DIM)
            .weight(FontWeight::WeightBold)
            .single_line()
            .into()
    };
    let head = grid(
        |column| match column {
            Column::Name => label("ИМЯ"),
            Column::Format => label("ФОРМАТ"),
            Column::Date => label("ДАТА"),
            Column::Size => label("РАЗМЕР"),
            Column::Status => label("СОСТОЯНИЕ"),
            Column::Progress => label("ЗАГРУЗКА"),
            Column::Twist | Column::Icon | Column::Actions => theme::nothing(),
        },
        columns,
        0.0,
    )
    .height(Length::Fixed(HEADER_HEIGHT));

    column![
        theme::hairline(theme::LINE_SOFT),
        gutters(head).background(theme::SHELF),
        theme::hairline(theme::LINE_SOFT),
    ]
    .width(Length::Fill)
    .into()
}

/// Строки таблицы. Каждая названа своим ключом: список переупорядочивается и
/// теряет элементы из середины, а состояние виджетов должно ехать за строкой,
/// а не за местом.
pub fn body(view: ViewId, lines: &[Line<'_>], context: Context<'_>) -> Element<Msg> {
    column(lines.iter().map(|line| match line {
        Line::Group { title, meta, depth } => group_line(title, meta, *depth, context),
        Line::Entry { row, depth } => entry_line(view, row, *depth, context),
    }))
    .width(Length::Fill)
    .into()
}

/// Заголовок группы: та же сетка, что у строки, но нажимать в нём нечего.
fn group_line(title: &str, meta: &str, depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Icon => glyph(theme::glyph::FOLDER, theme::ACCENT),
            Column::Name => text::<Msg>(format::ellipsize(
                title,
                format::mono_fit(context.name_width - indent, theme::TEXT_LABEL),
            ))
            .size(theme::TEXT_LABEL)
            .color(theme::INK)
            .weight(FontWeight::WeightMedium)
            .single_line()
            .into(),
            // Сколько внутри — говорится там, где у строк стоит их состояние:
            // это про содержимое группы, а не про её размер.
            Column::Status => text::<Msg>(meta.to_string())
                .size(theme::TEXT_SMALL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
            _ => theme::nothing(),
        },
        context.columns,
        indent,
    )
    .height(Length::Fixed(theme::ROW_HEIGHT));

    column![
        gutters(cells).background(theme::SHELF),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(format!("group:{}:{}", depth, title))
    .into()
}

fn entry_line(view: ViewId, row_data: &Row, depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Twist => twist(view, row_data, context),
            // Цветом сказано, чем запись является, а не чем она лежит: снимок
            // одним объектом и снимок каталогом — один и тот же снимок.
            Column::Icon => glyph(
                row_glyph(row_data),
                match row_data.kind {
                    RowKind::File => theme::INK_FAINT,
                    RowKind::Folder | RowKind::Product { .. } => theme::ACCENT,
                },
            ),
            // Имя моноширинным: колонка считается по знакам, а у
            // пропорционального шрифта их ширина разная. Папка — обычным: это
            // подпись, а не значение.
            Column::Name => {
                let title = format::ellipsize(
                    &row_data.title,
                    format::mono_fit(context.name_width - indent, theme::TEXT_MONO),
                );
                match row_data.kind.is_folder() {
                    true => text::<Msg>(title)
                        .size(theme::TEXT_LABEL)
                        .color(theme::INK)
                        .weight(FontWeight::WeightMedium)
                        .single_line()
                        .into(),
                    false => mono::<Msg>(title).size(theme::TEXT_MONO).color(theme::INK_SOFT).into(),
                }
            }
            // Моноширинным и с усечением по месту: это код, а не слово, и
            // обрезать его молча нельзя — ячейка обрежет по границе, и значение
            // упрётся в соседнюю колонку без единого знака о том, что оно
            // неполное.
            Column::Format => mono::<Msg>(format::ellipsize(
                &row_data.format(),
                format::mono_fit(FORMAT - CELL_PADDING * 2.0, theme::TEXT_TAG),
            ))
            .size(theme::TEXT_TAG)
            .color(theme::INK_FAINT)
            .single_line()
            .into(),
            Column::Date => text::<Msg>(format::date(row_data.date, context.now))
                .size(theme::TEXT_SMALL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
            Column::Size => container(
                mono::<Msg>(match row_data.size > 0 {
                    true => format::bytes(row_data.size),
                    false => String::new(),
                })
                .size(theme::TEXT_SMALL)
                .color(theme::INK_MUTED),
            )
            .width(Length::Fill)
            .align_x(Alignment::End)
            .into(),
            Column::Status => {
                let (dot, label, label_color) = status_look(&row_data.status);
                row![
                    theme::dot(dot),
                    text::<Msg>(label).size(theme::TEXT_SMALL).color(label_color).single_line(),
                ]
                .spacing(6.0)
                .align_items(Alignment::Center)
                .into()
            }
            Column::Progress => progress_cell(&row_data.status),
            Column::Actions => actions(view, row_data, context),
        },
        context.columns,
        indent,
    )
    .height(Length::Fixed(theme::ROW_HEIGHT));

    // Вся строка — кнопка: нажатие на неё делает то же, что её главная кнопка.
    // Отдельная кнопка при этом остаётся: по ней видно, что именно случится.
    let picked = !context.picked.is_empty() && row_data.identifier == context.picked;
    let line = match primary(view, row_data) {
        Some((_, _, message)) => theme::row_button(cells, picked).on_press(message),
        None => theme::row_button(cells, picked),
    };

    column![
        line.width(Length::Fill).height(Length::Fixed(theme::ROW_HEIGHT)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(row_data.key().to_string())
    .into()
}

/// Насколько отодвинут ярус группировки. Считается по глубине здесь и только
/// здесь: сдвиг знают двое — сетка (первая колонка шире на него) и обрезка
/// имени (колонка имени на него же уже), и разъехаться им нельзя.
fn indent(depth: usize) -> f32 {
    depth as f32 * INDENT
}

/// Содержимое первой колонки. Ступень группировки сюда не входит: её ставит
/// сетка шириной колонки, иначе иконка упирается в её край (см. `grid`).
fn glyph(glyph: &str, color: Color) -> Element<Msg> {
    theme::row_glyph::<Msg>(glyph, color)
}

/// Значок строки — по роду, а не по расширению. Снимок обозначается спутником
/// и тогда, когда лежит папкой: папка тут — способ хранения, а не то, чем эта
/// запись является. Расширение спрашивается последним, у того, о чём больше
/// сказать нечего.
fn row_glyph(row: &Row) -> &'static str {
    match row.kind {
        RowKind::Product { .. } => theme::glyph::SATELLITE,
        RowKind::Folder => theme::glyph::FOLDER,
        RowKind::File if IMAGE_FORMATS.contains(&row.format().to_lowercase().as_str()) => {
            theme::glyph::IMAGE
        }
        RowKind::File => theme::glyph::FILE,
    }
}

/// Кружок, подпись и её цвет — три способа сказать одно и то же состояние.
fn status_look(status: &RowStatus) -> (Color, String, Color) {
    match status {
        RowStatus::Complete => (theme::ACCENT, "на диске".to_string(), theme::ACCENT_TEXT),
        RowStatus::Remote => (theme::LINE, "в хранилище".to_string(), theme::INK_DIM),
        RowStatus::Downloading { done, total } => (
            theme::ACCENT,
            if *total > 0 { format::progress(*done, *total) } else { "скачивается".to_string() },
            theme::ACCENT_TEXT,
        ),
        RowStatus::Paused { done, total } => (
            theme::WARN,
            if *total > 0 { format::progress(*done, *total) } else { "прервано".to_string() },
            theme::WARN_TEXT,
        ),
        RowStatus::Partial { done } => (theme::ACCENT, format!("{} на диске", done), theme::ACCENT_TEXT),
    }
}

fn progress_cell(status: &RowStatus) -> Element<Msg> {
    let Some((done, total)) = status.progress() else {
        return theme::nothing();
    };
    let color = if matches!(status, RowStatus::Downloading { .. }) { theme::ACCENT } else { theme::WARN };
    container(
        progress_bar::<Msg>(0.0..=1.0, done as f32 / total as f32)
            .style(theme::progress(color))
            .width(Length::Fill)
            .height(Length::Fixed(5.0)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

/// Главная кнопка строки: глиф, подпись для подсказки и что она делает.
/// `None` — делать со строкой нечего (например, ключ провайдера неизвестен).
fn primary(view: ViewId, row: &Row) -> Option<(&'static str, &'static str, Msg)> {
    // Строка, собранная из записей, сама записью не является: качать и
    // открывать нужно её файлы, а её ключ — путь снимка в хранилище, и послать
    // его в закачку значит попросить скачать папку одним объектом. Её
    // собственное действие — раскрыться (см. `twist`).
    if !row.children.is_empty() {
        return None;
    }
    // Действие без ключа некому адресовать — лучше не предлагать его вовсе,
    // чем послать заведомо пустой запрос. Отсюда `?` на каждом: чем адресовать
    // это действие, известно раньше, чем оно построено.
    let remote = || (!row.identifier.is_empty()).then(|| row.identifier.clone());
    let local = || (!row.name.is_empty()).then(|| row.name.clone());

    Some(match &row.status {
        RowStatus::Downloading { .. } => (theme::glyph::PAUSE, "Пауза", Msg::Download(remote()?, row.product.clone())),
        RowStatus::Paused { .. } => (theme::glyph::PLAY, "Продолжить", Msg::Download(remote()?, row.product.clone())),
        _ if row.kind.is_folder() => {
            (theme::glyph::ENTER, "Перейти", Msg::In(view, ViewMsg::Enter(remote()?)))
        }
        RowStatus::Remote => (theme::glyph::DOWNLOAD, "Скачать", Msg::Download(remote()?, row.product.clone())),
        RowStatus::Complete => {
            (theme::glyph::EYE, "Открыть", Msg::In(view, ViewMsg::Preview(local()?)))
        }
        // Про папку, часть которой скачана, главная кнопка сказать не может:
        // «скачать остальное» библиотека не умеет, а «открыть» тут нечего.
        RowStatus::Partial { .. } => return None,
    })
}

/// Кнопки справа: быстрые значки и меню всего остального.
fn actions(view: ViewId, entry: &Row, context: Context<'_>) -> Element<Msg> {
    let mut buttons: Vec<Element<Msg>> = quick(view, entry)
        .into_iter()
        .map(|(glyph, hint, message)| {
            hinted(theme::row_button_icon(glyph, false).on_press(message), hint)
        })
        .collect();

    let items = menu_items(view, entry, context.here);
    if !items.is_empty() {
        let menu = Menu::Row(entry.key().to_string());
        let open = context.listing.menu == menu;
        let anchor = theme::row_button_icon(theme::glyph::MORE, open)
            .on_press(Msg::In(view, ViewMsg::OpenMenu(menu)));

        buttons.push(
            popover(anchor, super::menu::panel(items))
                .open(open)
                .align_x(Alignment::End)
                .gap(4.0)
                .on_dismiss(Msg::In(view, ViewMsg::OpenMenu(Menu::Closed)))
                .into(),
        );
    }

    container(row(buttons).spacing(BUTTON_GAP).align_items(Alignment::Center))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Center)
        .into()
}

/// Быстрые значки строки по порядку: сначала то, ради чего строку открывают,
/// потом показ на шаре.
///
/// Значок глобуса — только у снимка: класть на шар папку пути или файл внутри
/// снимка нечего, а кнопка, которой нечего сделать, врёт о том, что строка
/// умеет. Остальным этот пункт по-прежнему доступен из меню (см. `menu_items`).
fn quick(view: ViewId, row: &Row) -> Vec<(&'static str, &'static str, Msg)> {
    let mut quick = Vec::new();
    if let Some(action) = primary(view, row) {
        quick.push(action);
    }
    // У снимка-папки главного действия нет (внутрь ведёт треугольник), поэтому
    // просмотр стоит значком: иначе смотреть снимок можно было бы только через
    // меню, а это его основное занятие.
    if row.kind.is_product() && row.kind.is_folder() && !row.identifier.is_empty() {
        quick.push((
            theme::glyph::EYE,
            "Смотреть снимок",
            Msg::In(view, ViewMsg::PreviewProduct(row.identifier.clone())),
        ));
    }
    if row.kind.is_product() && !row.identifier.is_empty() {
        quick.push((
            theme::glyph::GLOBE,
            "На глобус",
            Msg::In(view, ViewMsg::GlobeShow(row.identifier.clone())),
        ));
    }
    quick
}

/// Треугольник раскрытия. У строки без файлов — пусто: место за ней остаётся,
/// поэтому соседи не съезжают, когда в списке есть и то, и другое.
fn twist(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    if row.children.is_empty() {
        return theme::nothing();
    }
    let open = context.listing.expanded.contains(row.key());
    theme::chrome_icon(
        icon::<Msg>(if open { theme::glyph::CARET } else { theme::glyph::RIGHT })
            .size(9.0)
            .color(theme::INK_DIM),
    )
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_BUTTON))
    .on_press(Msg::In(view, ViewMsg::Expand(row.key().to_string())))
    .into()
}

/// Кнопка с подсказкой: подсказка одинакова у всех значков строки, и написанная
/// на каждом call-сайте она разъезжается отступом и кеглем.
pub fn hinted(button: crate::proto::ui_service::Container<Msg>, hint: &str) -> Element<Msg> {
    tooltip(button, hint, TooltipPosition::TooltipTop)
        .style(theme::panel())
        .text_size(theme::TEXT_SMALL)
        .padding(6.0)
        .into()
}

/// Всё, что делают со строкой редко. Порядок общий: сначала переходы, потом
/// необратимое.
fn menu_items(view: ViewId, row: &Row, here: &str) -> Vec<super::menu::Item> {
    use super::menu::Item;
    let mut items = Vec::new();

    // Показать на шаре можно всё, у чего есть ключ провайдера: у найденного
    // продукт с контуром уже под рукой, у строки каталога или скачанного его
    // восстанавливает провайдер по ключу (см. handlers::overlay). У снимка
    // этот пункт стоит значком в самой строке и здесь не повторяется.
    if !row.identifier.is_empty() && !row.kind.is_product() {
        items.push(Item::new(
            "Показать на шаре",
            Msg::In(view, ViewMsg::GlobeShow(row.identifier.clone())),
        ));
    }
    // Показывать папку, которая и так открыта, незачем: пункт вёл бы туда,
    // где пользователь уже стоит.
    if !row.folder().is_empty() && !here.trim_end_matches('/').ends_with(row.folder()) {
        items.push(Item::new(
            "Показать в каталоге",
            Msg::In(view, ViewMsg::Enter(format!("{}/", row.folder()))),
        ));
    }
    // Снимок, лежащий папкой, тоже смотрят — только растр внутри выбирает
    // провайдер: сам путь каталога `GET` не открывает (см.
    // `handlers::preview::on_view_product_pressed`).
    if row.kind.is_product() && row.kind.is_folder() && !row.identifier.is_empty() {
        items.push(Item::new(
            "Смотреть снимок",
            Msg::In(view, ViewMsg::PreviewProduct(row.identifier.clone())),
        ));
    }
    if !row.kind.is_folder() && !row.identifier.is_empty() && matches!(row.status, RowStatus::Remote) {
        items.push(Item::new(
            "Смотреть без скачивания",
            Msg::In(view, ViewMsg::PreviewRemote(row.identifier.clone())),
        ));
    }
    // Перекачка сносит имеющийся файл до старта — иначе рядом с ним лёг бы
    // второй, недокачанный (см. data-library::download). Отсюда и пометка.
    // Строке, собранной из записей, качать нечего: её ключ — путь снимка в
    // хранилище, и послать его в закачку значит попросить скачать папку одним
    // объектом. Качают её файлы — по одному, из раскрытой строки.
    if !row.identifier.is_empty()
        && row.children.is_empty()
        && matches!(row.status, RowStatus::Complete)
    {
        items.push(
            Item::new("Скачать заново", Msg::Download(row.identifier.clone(), row.product.clone()))
                .danger(),
        );
    }
    // Только у того, что на диске: показывать в файловом менеджере лежащее в
    // хранилище нечего.
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        items.push(Item::new(
            "Показать в файловом менеджере",
            Msg::Reveal(row.name.clone()),
        ));
    }
    // Отказаться от начатого и удалить скачанное — одно действие: и то, и
    // другое оставляет после себя пустое место. Разной у них может быть только
    // подпись, потому что по-разному называется то, что пропадёт.
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        let label = match row.status {
            RowStatus::Downloading { .. } | RowStatus::Paused { .. } => "Отменить загрузку",
            _ => "Удалить с диска",
        };
        items.push(Item::new(label, Msg::Delete(row.name.clone())).danger());
    }
    // У строки, собранной из записей, своего имени нет — она и не запись, а
    // снимок. Выбрасывается он целиком: раскрывать его, чтобы удалить файлы по
    // одному, значит просить пользователя сделать за нас то, что мы же и
    // свернули.
    if !row.children.is_empty() && !row.product.is_empty() {
        items.push(
            Item::new("Удалить снимок с диска", Msg::DeleteSnapshot(row.product.clone())).danger(),
        );
    }

    items
}
