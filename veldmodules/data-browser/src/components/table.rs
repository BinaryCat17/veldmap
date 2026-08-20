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
use crate::module::components::{arrange::Line, format, OnGlobe, OnOutline, Row, RowKind, RowStatus};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::state::overlay::Pace;
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Колонка таблицы. Перечислением, а не списком ширин: колонок столько же,
/// сколько ячеек в строке, и связывать одно с другим позицией в массиве значит
/// однажды сдвинуть ячейку на соседнюю колонку.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    /// Выбор строки: набор для пакетных действий (см. handlers::outline).
    /// Своя колонка по той же причине, что у раскрытия, — это отдельное
    /// действие, и нажимать его надо мимо главного действия строки.
    Check,
    /// Треугольник раскрытия. Своя колонка, а не значок внутри имени:
    /// раскрытие — действие, и нажимать его надо мимо самой строки, иначе оно
    /// спорит с её главным действием.
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
const COLUMNS: [Column; 10] = [
    Column::Check,
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
/// состояние, дата и размер — справка, а не действие. Значок уходит последним:
/// род строки виден и по её имени, но пока место есть, значок читается быстрее.
///
/// Имени и кнопок в этом списке нет: имя единственное тянущееся, и сжатие
/// доводит его до нулевой ширины (см. `Wrapping` в types.proto), а строка без
/// имени и без действий — не строка.
const DROP_ORDER: [Column; 6] =
    [Column::Progress, Column::Date, Column::Size, Column::Status, Column::Format, Column::Icon];

/// Отметка и треугольник раскрытия — узкие колонки перед значком. Полей у них
/// нет вовсе (см. [`padding_of`]), поэтому ширина здесь — это ровно то место,
/// которое достаётся кнопке.
///
/// Мерены они по знаку, а не на глаз: у строки, которой нечего отметить и
/// нечего раскрыть, обе стоят пустыми, и всё лишнее в них читается как отступ
/// — будто файл вложен туда, где вложенности нет.
const CHECK: f32 = 20.0;
const TWIST: f32 = 14.0;
const ICON: f32 = 34.0;
/// Под тип продукта, а не под расширение файла: у радара он длинный
/// (`IW_GRDH_1SDV`), и по трём буквам «png» такую колонку не смеришь.
const FORMAT: f32 = 80.0;
const DATE: f32 = 88.0;
const SIZE: f32 = 84.0;
const STATUS: f32 = 104.0;
const PROGRESS: f32 = 132.0;
/// Сколько кнопок помещается в колонку действий. Больше всего их у снимка,
/// лежащего папкой: скачать целиком, посмотреть, положить на шар и меню.
const ACTION_BUTTONS: f32 = 4.0;

/// Ширина колонки действий. Считается по числу кнопок, а не подбирается на
/// глаз, — приписанная кнопка иначе молча уедет под обрезку ячейки (см. `grid`).
const ACTIONS: f32 =
    theme::ROW_BUTTON * ACTION_BUTTONS + BUTTON_GAP * (ACTION_BUTTONS - 1.0) + CELL_PADDING * 2.0;

/// Та же колонка, когда в ней осталось одно меню.
const ACTIONS_COMPACT: f32 = theme::ROW_BUTTON + CELL_PADDING * 2.0;

/// Ниже этого имени не остаётся даже узнаваемого куска, и колонка действий
/// уступает свои значки меню (см. [`Fit::compact`]). Число мельче
/// [`NAME_MIN`]: то — ширина, ради которой уходят соседние колонки, а это —
/// граница, за которой строка перестаёт быть строкой.
const NAME_FLOOR: f32 = 64.0;

/// Сколько места занимает колонка. У имени своей ширины нет — оно забирает
/// остаток.
fn width_of(column: Column) -> f32 {
    match column {
        Column::Check => CHECK,
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

/// Ширина, ради которой колонки уступают место: пока имени достаётся меньше,
/// уходит следующая соседняя. Обещанием она не является — уступать бывает уже
/// нечему, и тогда имя получает то, что осталось (см. [`fit`]).
const NAME_MIN: f32 = 150.0;

/// Зазор между кнопками строки. Наружу затем, что такой же ряд кнопок стоит в
/// списке слоёв, и вторая запись этого зазора разошлась бы с первой.
pub const BUTTON_GAP: f32 = 6.0;

/// Отступ содержимого ячейки от её границ — по обе стороны.
const CELL_PADDING: f32 = 8.0;

/// Какие необязательные колонки нужны этому списку. Знает это тот, кто собрал
/// строки: раскрывать и отмечать бывает нечего, и пустой столбец тогда сдвигал
/// бы весь список ради того, чего в нём не бывает.
#[derive(Clone, Copy, Default)]
pub struct Optional {
    /// Есть что раскрыть.
    pub twisty: bool,
    /// Есть что выбрать — то есть в списке стои́т хоть одна выбираемая строка
    /// (см. `Row::choosable`).
    pub checkable: bool,
}

/// Какие колонки показывать в отведённой ширине и сколько достанется имени.
///
/// Половина экрана вдвое уже окна, а фиксированные колонки от этого не
/// худеют — в узком месте их сумма перекрывает всё место, и тянущееся имя
/// схлопывается в ноль. Поэтому лишние не сжимаются, а уходят: список без
/// даты читается, список без имени — нет.
///
/// Возвращаемая ширина имени — та, что достанется ему на самом деле, даже если
/// это меньше [`NAME_MIN`]: по ней считается многоточие (`format::mono_fit`), и
/// названное с запасом имя не влезло бы в свою ячейку — вместо многоточия его
/// обрывала бы обрезка, на середине знака.
pub fn fit(width: f32, optional: Optional) -> Fit {
    let mut columns: Vec<Column> = COLUMNS.to_vec();
    if !optional.checkable {
        columns.retain(|column| *column != Column::Check);
    }
    if !optional.twisty {
        columns.retain(|column| *column != Column::Twist);
    }
    // Отступы экрана и собственные поля колонки имени тратятся всегда.
    let overhead = theme::GUTTER * 2.0 + CELL_PADDING * 2.0;
    let taken = |columns: &[Column], compact: bool| -> f32 {
        columns
            .iter()
            .map(|column| match (*column, compact) {
                (Column::Actions, true) => ACTIONS_COMPACT,
                (other, _) => width_of(other),
            })
            .sum::<f32>()
            + overhead
    };

    for dropped in DROP_ORDER {
        if width - taken(&columns, false) >= NAME_MIN {
            break;
        }
        columns.retain(|column| *column != dropped);
    }
    // Уступать больше нечем, а имени не осталось даже на узнаваемый кусок:
    // тогда уступают значки строки. Уходят они не в никуда — то же действие
    // стои́т пунктом её меню, и разница только в том, есть ли под него место.
    let compact = width - taken(&columns, false) < NAME_FLOOR;
    let name = (width - taken(&columns, compact)).max(0.0);
    Fit { columns, name, compact }
}

/// Что вышло из [`fit`]: какие колонки показывать, сколько досталось имени и
/// не пришлось ли колонке действий ужаться до одного меню.
pub struct Fit {
    pub columns: Vec<Column>,
    pub name: f32,
    /// Значки строки не поместились и ушли в её меню.
    pub compact: bool,
}

/// Ступенька группировки.
const INDENT: f32 = 12.0;

/// Высота шапки — своя: в ней подписи, а не строки данных.
const HEADER_HEIGHT: f32 = 22.0;

/// Полоса хода укладки на шар под строкой снимка. Тонкая: она отвечает на один
/// вопрос — «идёт ли», — а сколько именно осталось, сказано словами в «На
/// просмотре».
const ONTO_GLOBE: f32 = 3.0;

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
    compact: bool,
) -> RowBuilder<Msg> {
    // Ступень группировки расширяет первую показанную колонку, какой бы она ни
    // была: колонки отметки и раскрытия есть не всегда, и привязка ступени к
    // ним роняла бы отступ группы там, где отмечать и раскрывать нечего.
    let first = columns.first().copied();
    row(columns.iter().map(|column| {
        let step = if Some(*column) == first { indent } else { 0.0 };
        let pad = padding_of(*column);
        let (width, left) = match (column, compact) {
            // Ступень достаётся и имени, когда первой показанной колонкой
            // оказалось оно: своей ширины у имени нет, поэтому ступень уходит
            // в отступ. Иначе ярусы группировки схлопывались бы в один — а
            // ширина имени всё равно считалась бы за вычетом ступени, то есть
            // подрезанной под отступ, которого нет (см. `entry_line`).
            (Column::Name, _) => (Length::Fill, pad + step),
            (Column::Actions, true) => (Length::Fixed(ACTIONS_COMPACT + step), pad + step),
            (other, _) => (Length::Fixed(width_of(*other) + step), pad + step),
        };
        container(cells(*column))
            .width(width)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left, right: pad })
            .into()
    }))
    .width(Length::Fill)
}

/// Поля ячейки по бокам. У колонок-кнопок их нет: кнопка занимает ячейку
/// целиком, и восемь точек полей с каждой стороны оставили бы от двадцати
/// четыре — значок в такой ячейке рисуется огрызком, а нажать по нему негде.
fn padding_of(column: Column) -> f32 {
    match column {
        Column::Check | Column::Twist => 0.0,
        _ => CELL_PADDING,
    }
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
    /// Раскрытое меню этого списка; `None` — раскрыто что-то другое или
    /// ничего. Живёт оно в состоянии экрана (`State::open`), а не вида:
    /// раскрытым бывает одно на весь экран.
    pub menu: Option<&'a Menu>,
    /// Колонки, которые помещаются в отведённую ширину, — их и рисуем
    /// (см. [`fit`]). Набор один на всю таблицу: шапка, заголовок группы и
    /// строка обязаны совпасть по сетке.
    pub columns: &'a [Column],
    /// Ширина колонки имени в точках разметки.
    pub name_width: f32,
    /// Значки строки не поместились: у неё осталось одно меню, а то, что
    /// стояло значками, ушло в него пунктами (см. [`Fit::compact`]).
    pub compact: bool,
    /// Сколько знаков имени совпадает у всех строк страницы — в начале и в
    /// хвосте (см. [`format::shared`]). Одно на всю таблицу: режется по нему
    /// каждая строка, и посчитанное построчно резало бы соседей по-разному.
    pub shared: (usize, usize),
    /// Папка, которую сейчас показывают; пусто — вид её не знает (скачанное,
    /// поиск). По ней видно, куда вести «показать в каталоге» бессмысленно.
    pub here: &'a str,
    /// Ключ снимка, чей контур выбран на шаре; пусто — не выбран ни один.
    /// Живёт он в состоянии приложения (`State::pick`), а не вида: выбирают на
    /// шаре, а видно это должно быть в списке, из которого снимок родом.
    pub picked: &'a str,
    /// Строка, к которой привёл переход, — она подсвечена наравне с выбранной
    /// на шаре: обе отвечают на один и тот же вопрос «что здесь главное
    /// сейчас», и говорить о них по-разному было бы не о чем.
    pub target: &'a str,
    /// Точка отсчёта для подписей времени. Одна на всю таблицу: строки одного
    /// кадра должны мериться от одного «сейчас», иначе соседние даты
    /// перескакивают через полночь по-разному.
    pub now: i64,
}

/// Шапка. Стоит вне прокрутки, поэтому не уезжает вместе со строками.
///
/// `all` — отмечено ли всё, что видно под ней (см. `Arranged::all_marked`): в
/// колонке отметки стоит та же коробочка, что и в строках, и действует она на
/// тот же набор, о котором говорит.
pub fn header(view: ViewId, columns: &[Column], all: bool, compact: bool) -> Element<Msg> {
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
            // «Показанное», а не «всё»: коробочка берёт страницу со всем
            // раскрытым на ней, и подпись обязана сказать это — иначе она
            // обещает выбрать всю выдачу, а выберет двадцать строк.
            Column::Check => hinted(
                theme::row_check(Some(all)).on_press(Msg::In(view, ViewMsg::CheckShown(!all))),
                match all {
                    true => "Снять выбор с показанного",
                    false => "Выбрать показанное",
                },
            ),
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
        compact,
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
        Line::Waiting { depth } => waiting_line(*depth, context),
    }))
    .width(Length::Fill)
    .into()
}

/// Раскрытая папка, чей листинг ещё идёт. Той же сеткой, что и строка, — она
/// стоит на её месте и уступит его первой же приехавшей записи; нажимать в ней
/// нечего.
fn waiting_line(depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Name => text::<Msg>("загружается…".to_string())
                .size(theme::TEXT_SMALL)
                .color(theme::INK_FAINT)
                .single_line()
                .into(),
            _ => theme::nothing(),
        },
        context.columns,
        indent,
        context.compact,
    )
    .height(Length::Fixed(theme::ROW_HEIGHT));

    column![
        gutters(cells),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(format!("waiting:{}", depth))
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
        context.compact,
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
            Column::Check => check(view, row_data, context),
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
                let title = format::distinct(
                    &row_data.title,
                    format::mono_fit(context.name_width - indent, theme::TEXT_MONO),
                    context.shared,
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
                let cell = row![
                    theme::dot(dot),
                    text::<Msg>(label).size(theme::TEXT_SMALL).color(label_color).single_line(),
                ]
                .spacing(6.0)
                .align_items(Alignment::Center);
                // Причина срыва длиннее колонки, а срезанная на полуслове она
                // не объясняет ничего. Целиком её показывает подсказка —
                // единственное место в строке, чья ширина не задана колонкой.
                match &row_data.status {
                    RowStatus::Paused { trouble, .. } | RowStatus::Partial { trouble, .. }
                        if !trouble.is_empty() => tooltip(
                        cell,
                        // Ужимаем: причину пишет не разметка, а тот, кто
                        // отказал, и длину её ничто не ограничивает — а
                        // подсказка идёт одной строкой и шире окна не влезет.
                        format::ellipsize(trouble, 90),
                        TooltipPosition::TooltipTop,
                    )
                    .style(theme::panel())
                    .text_size(theme::TEXT_SMALL)
                    .into(),
                    _ => cell.into(),
                }
            }
            Column::Progress => progress_cell(&row_data.status),
            Column::Actions => actions(view, row_data, context),
        },
        context.columns,
        indent,
        context.compact,
    );

    // Снимок едет на шар — под строкой полоса хода: нажали значок здесь, и
    // видно должно быть здесь же. Высота у неё занятая, а не добавленная: шаг
    // строки считается в одном месте (`theme::ROW_PITCH`), им же список
    // прокручивают к нужной строке, и подрасти он не может.
    //
    // Место под полосу занято, пока снимок на шаре, — а не пока по нему идёт
    // работа. Иначе строка подпрыгивает на три точки ровно в тот миг, когда
    // добыча кончилась, то есть на каждом слое и на глазах.
    // Полоса — снимку, а не всякому, кто о нём знает: о шаре отвечает и файл
    // внутри снимка (см. `rows::from_key`), но нажимали-то значок у снимка, и
    // видно должно быть там же.
    let onto_globe = row_data.globe.pace();
    let strip = row_data.globe.any() && row_data.is_snapshot();
    let height = theme::ROW_HEIGHT - if strip { ONTO_GLOBE } else { 0.0 };
    let cells = cells.height(Length::Fixed(height));

    // Вся строка — кнопка: нажатие на неё делает то же, что её главная кнопка,
    // а у той, чьё главное дело — раскрыться, оно и делает. Отдельная кнопка
    // при этом остаётся там, где она есть: по ней видно, что именно случится.
    let key = row_data.key();
    let press = primary(view, row_data).map(|action| action.message).or_else(|| {
        row_data
            .expandable()
            .then(|| Msg::In(view, ViewMsg::Expand(key.to_string())))
    });
    let line = match press {
        Some(message) => theme::row_button(cells, tint(row_data, context)).on_press(message),
        None => theme::row_button(cells, tint(row_data, context)),
    };

    let mut lines: Vec<Element<Msg>> =
        vec![line.width(Length::Fill).height(Length::Fixed(height)).into()];
    // Доли ещё нет, а работа идёт — полоса заливается целиком, но вполсилы:
    // пустая дорожка в три точки на этом месте не говорит ничего, а начинается
    // ею как раз самое долгое, что бывает со снимком, — описание растра по
    // сети, десятки секунд.
    let bar = match onto_globe {
        Pace::Share(share) => Some((theme::ACCENT, share)),
        Pace::Unknown => Some((theme::ACCENT_HALF, 1.0)),
        // Добыча кончилась, а место остаётся за ней: пустым, чтобы строка не
        // дрогнула. Что снимок на шаре, говорит зажжённый значок.
        Pace::Idle => None,
    };
    if strip {
        lines.push(match bar {
            Some((color, share)) => {
                // Подпись у полосы своя, а не только у значка в правом краю
                // строки: смотрят-то на полосу, и без подписи она — доля
                // неизвестно чего.
                tooltip(
                    progress_bar::<Msg>(0.0..=1.0, share)
                        .style(theme::progress(color))
                        .width(Length::Fill)
                        .height(Length::Fixed(ONTO_GLOBE)),
                    match row_data.globe.said() {
                        Some(said) => format!("Набирается пирамида — {}", said),
                        None => "Кладётся на глобус".to_string(),
                    },
                    TooltipPosition::TooltipTop,
                )
                .style(theme::panel())
                .text_size(theme::TEXT_SMALL)
                .padding(6.0)
                .into()
            }
            None => veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(ONTO_GLOBE)).into(),
        });
    }
    lines.push(theme::hairline(theme::LINE_ROW));

    column(lines).width(Length::Fill).key(row_data.key().to_string()).into()
}

/// Чем строка выделена среди соседей. Старшинство названо в самой роли
/// (см. [`theme::RowTint`]): подсветка одна на весь экран и старше отметки,
/// которых в списке бывает полсотни.
///
/// Выбор читается здесь той же меркой, что и в коробочке (`check`), — тем же
/// ключом и тем же множеством: два выражения на этот вопрос однажды разошлись
/// бы, и залитая строка стояла бы с пустой коробочкой.
fn tint(row: &Row, context: Context<'_>) -> theme::RowTint {
    let picked = (!context.picked.is_empty() && row.snapshot_key() == context.picked)
        || row.named(context.target);
    if picked {
        return theme::RowTint::Picked;
    }
    let marked = context.listing.selected.contains_key(row.choice_key());
    match marked {
        true => theme::RowTint::Marked,
        false => theme::RowTint::Plain,
    }
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
        // Сорвавшаяся закачка говорит своим голосом. «Прервано» ниже — это всё
        // остальное: и остановленная человеком, и та, чья причина не пережила
        // перезапуск; молчать о причине там, где она есть, значит предложить
        // нажать «Продолжить» вслепую.
        RowStatus::Paused { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER_TEXT,
        ),
        RowStatus::Paused { done, total, .. } => (
            theme::WARN,
            if *total > 0 { format::progress(*done, *total) } else { "прервано".to_string() },
            theme::WARN_TEXT,
        ),
        // Отказ важнее счёта: сколько скачано, видно и по размеру, а «3 на
        // диске» зелёным — это спокойная надпись, за которой стоящее не видно.
        RowStatus::Partial { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER_TEXT,
        ),
        RowStatus::Partial { done, .. } => (theme::ACCENT, format!("{} на диске", done), theme::ACCENT_TEXT),
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

/// Главное действие строки: что случится по нажатию на неё.
struct Primary {
    glyph: &'static str,
    /// Подпись подсказки — она же говорит, что именно случится.
    hint: &'static str,
    message: Msg,
    /// Действие уводит внутрь — в другую папку или в другой список. Отдельным
    /// значком такое не стоит: нажатие на строку делает ровно то же, и стрелка
    /// рядом с ней повторяла бы её (см. `quick`).
    transition: bool,
}

impl Primary {
    fn new(glyph: &'static str, hint: &'static str, message: Msg) -> Self {
        Self { glyph, hint, message, transition: false }
    }

    fn transition(glyph: &'static str, hint: &'static str, message: Msg) -> Self {
        Self { glyph, hint, message, transition: true }
    }
}

/// Главная кнопка строки. `None` — делать со строкой нечего (например, ключ
/// провайдера неизвестен).
fn primary(view: ViewId, row: &Row) -> Option<Primary> {
    // Строка, сложенная из записей, сама записью не является: качать и
    // открывать нужно её файлы, а её ключ — путь снимка в хранилище, и послать
    // его в закачку значит попросить скачать папку одним объектом. Её
    // собственное действие — раскрыться (см. `twist`).
    if row.folded {
        return None;
    }
    // Действие без ключа некому адресовать — лучше не предлагать его вовсе,
    // чем послать заведомо пустой запрос. Отсюда `?` на каждом: чем адресовать
    // это действие, известно раньше, чем оно построено.
    let remote = || (!row.identifier.is_empty()).then(|| row.identifier.clone());
    let local = || (!row.name.is_empty()).then(|| row.name.clone());

    Some(match &row.status {
        // Папка спрашивается первой, чем бы она ни была занята: её ключ —
        // общий префикс, а не объект, и послать его в закачку значит попросить
        // скачать папку одним куском. Пауза и продолжение у неё свои и стоят
        // пунктами меню — там они названы снимком, а не файлом.
        _ if row.kind.is_folder() => Primary::transition(
            theme::glyph::ENTER,
            "Перейти",
            Msg::In(view, ViewMsg::Enter(remote()?)),
        ),
        RowStatus::Downloading { .. } => {
            Primary::new(theme::glyph::PAUSE, "Пауза", Msg::Download(remote()?, row.product.clone()))
        }
        RowStatus::Paused { .. } => Primary::new(
            theme::glyph::PLAY,
            "Продолжить",
            Msg::Download(remote()?, row.product.clone()),
        ),
        RowStatus::Remote => Primary::new(
            theme::glyph::DOWNLOAD,
            "Скачать",
            Msg::Download(remote()?, row.product.clone()),
        ),
        RowStatus::Complete => {
            Primary::new(theme::glyph::EYE, "Открыть", Msg::In(view, ViewMsg::Preview(local()?)))
        }
        // Про папку, часть которой скачана, главная кнопка сказать не может:
        // «скачать остальное» библиотека не умеет, а «открыть» тут нечего.
        RowStatus::Partial { .. } => return None,
    })
}

/// Кнопки справа: быстрые значки и меню всего остального.
fn actions(view: ViewId, entry: &Row, context: Context<'_>) -> Element<Msg> {
    // В тесноте значки уступают место имени: строка без имени не строка. Уходят
    // они не в никуда — то же действие становится пунктом меню, и значок с
    // пунктом здесь одно и то же, разница только в том, есть ли место.
    let shown = match context.compact {
        true => Vec::new(),
        false => quick(view, entry),
    };
    let mut buttons: Vec<Element<Msg>> = shown
        .into_iter()
        .map(|Quick { glyph, hint, message, tone, .. }| match message {
            Some(message) => icon_button(glyph, tone, &hint, message),
            None => hinted(theme::row_button_icon(glyph, tone), &hint),
        })
        .collect();

    let mut items = menu_items(view, entry, context.here);
    if context.compact {
        // Пунктами — впереди прочего: это главные действия строки, значками
        // они и стояли. Выключенный значок пунктом не становится: значок хотя
        // бы держит своё место в ряду и объясняется подсказкой, а пункт меню,
        // который ничего не делает, не делает и этого.
        let moved: Vec<super::menu::Item> = quick(view, entry)
            .into_iter()
            .filter_map(|Quick { glyph, label, message, .. }| {
                Some(super::menu::Item::new(label, message?).glyph(glyph))
            })
            .collect();
        items.splice(0..0, moved);
    }
    if !items.is_empty() {
        let menu = Menu::Row(entry.key().to_string());
        let open = context.menu == Some(&menu);
        let raised = match open {
            true => theme::IconTone::Raised,
            false => theme::IconTone::Rest,
        };
        let anchor = theme::row_button_icon(theme::glyph::MORE, raised)
            .on_press(Msg::In(view, ViewMsg::OpenMenu(Some(menu))));

        buttons.push(
            popover(anchor, open, || super::menu::panel(items))
                .align_x(Alignment::End)
                .gap(4.0)
                .on_dismiss(Msg::In(view, ViewMsg::OpenMenu(None)))
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

/// Быстрый значок строки: чем нарисован, что скажет подсказка, что пошлёт и
/// каким лицом стоит.
struct Quick {
    glyph: &'static str,
    hint: String,
    /// Чем он зовётся пунктом меню, куда уезжает в тесноте. Отдельно от
    /// подсказки: та рассказывает и состояние («спрашиваем каталог…»), а пункт
    /// меню обязан называть действие — состояние в списке пунктов не читается
    /// и предлагает непонятно что. Доводом конструктора, а не умолчанием:
    /// забытая подпись дала бы безымянный пункт, и компилятор бы промолчал.
    label: &'static str,
    /// Что пошлёт нажатие. `None` — послать нечего: значок стои́т выключенным
    /// и в меню тесноты не уезжает. Так стои́т наводка у снимка, которого на
    /// шаре нет вовсе: пропасть ей нельзя — соседние значки съехали бы на её
    /// место при каждом нажатии, — а обещать нажатие она не вправе.
    message: Option<Msg>,
    /// Каким лицом стои́т. Умолчание — покой: состояние есть у значков контура
    /// и шара, а у наводки — только «есть куда навести» и «некуда».
    tone: theme::IconTone,
}

impl Quick {
    fn new(glyph: &'static str, label: &'static str, hint: String, message: Msg) -> Self {
        Self { glyph, hint, label, message: Some(message), tone: theme::IconTone::Rest }
    }

    /// Значок без нажатия: делать нечего, а место за ним остаётся.
    fn idle(glyph: &'static str, label: &'static str, hint: String) -> Self {
        Self { glyph, hint, label, message: None, tone: theme::IconTone::Idle }
    }

    fn tone(self, tone: theme::IconTone) -> Self {
        Self { tone, ..self }
    }
}

/// Быстрые значки строки — только те три, что про шар: контур, растр и
/// наводка.
///
/// Первые два — переключатели, и механика у них одна: нажали — снимок на шаре,
/// нажали ещё раз — снят. Двух правил здесь быть не может: вопрос у них один
/// («лежит ли этот снимок на шаре»), разница только в подробности, с какой он
/// там лежит. Камеру ни один из них не двигает — это третье намерение, и стои́т
/// оно третьим значком: «покажи здесь» и «отвези меня туда» сведённые в одно
/// нажатие отбирают друг у друга ответ.
///
/// Всё остальное — скачать, приостановить, открыть, посмотреть — стои́т
/// пунктами меню (см. [`menu_items`]). Строка узкая, панель делят пополам, и
/// значок сверх этих отнимал бы место у имени; а каждый из трёх говорит о
/// своём состоянии цветом, чего пункт меню не умеет.
///
/// Перехода вглубь среди них нет: внутрь ведёт нажатие на саму
/// строку, и стрелка рядом с ней говорила бы то же самое второй раз.
///
/// Все — только у снимка: ни контура, ни растра у папки пути и у файла внутри
/// снимка не бывает, а кнопка, которой нечего сделать, врёт о том, что строка
/// умеет.
fn quick(view: ViewId, row: &Row) -> Vec<Quick> {
    let mut quick = Vec::new();
    let key = row.snapshot_key().to_string();
    if !row.is_snapshot() || key.is_empty() {
        return quick;
    }

    // Контур — своё состояние строки, не выбор и не показ (см.
    // handlers::outline). Горит зелёным, когда нарисован; вполсилы — пока
    // геометрия едет и когда её не оказалось вовсе.
    //
    // Подпись называет то, что случится по нажатию, а состояние объясняет
    // после тире: у не спросившегося нажатие переспрашивает, а не снимает
    // просьбу (см. `outline::toggle_outline`).
    let (tone, hint) = match row.outlined {
        OnOutline::Off => (theme::IconTone::Rest, "Очертить на шаре"),
        OnOutline::Asking => (theme::IconTone::Half, "Убрать контур — спрашиваем каталог…"),
        OnOutline::Blank => {
            (theme::IconTone::Half, "Убрать контур — геометрии у снимка нет")
        }
        OnOutline::Failed => (theme::IconTone::Half, "Переспросить контур — не спросился"),
        OnOutline::Drawn => (theme::IconTone::Lit, "Убрать контур с шара"),
    };
    quick.push(
        Quick::new(
            theme::glyph::OUTLINE,
            "Контур на шаре",
            hint.to_string(),
            Msg::OutlineToggle(key.clone()),
        )
        .tone(tone),
    );

    // `viewable` — не «покажется наверняка», а «есть смысл предлагать»
    // (см. `Row::viewable`): значок над сырьём уровня 0 или над архивом обещает
    // то, чего не бывает.
    if row.viewable {
        // Значок горит, когда снимок лежит на шаре растром. Подпись называет
        // то, что случится по нажатию: у лежащего это снятие, а не второе
        // наложение. Скрытый снят наравне с видимым — он остаётся слоем, и
        // прячет его свой глаз в списке «На просмотре».
        let (tone, hint) = match row.globe {
            OnGlobe::Off => (theme::IconTone::Rest, "Показать на шаре"),
            OnGlobe::Asked => (theme::IconTone::Half, "Отменить показ — спрашиваем каталог…"),
            OnGlobe::Assembling => (theme::IconTone::Half, "Отменить показ — кладётся на глобус…"),
            OnGlobe::Laid { hidden: true, .. } => {
                (theme::IconTone::Half, "Снять с шара — сейчас лежит скрытым")
            }
            OnGlobe::Laid { hidden: false, .. } => (theme::IconTone::Lit, "Снять с шара"),
        };
        // Полосу под строкой рисует ход добычи, и объяснить её больше негде:
        // подписи у полосы нет, а без неё она — доля неизвестно чего.
        let hint = match row.globe.said() {
            Some(said) => format!("{} · {}", hint, said),
            None => hint.to_string(),
        };
        quick.push(
            Quick::new(
                theme::glyph::GLOBE,
                "Показать на шаре",
                hint,
                Msg::In(view, ViewMsg::GlobeToggle(key.clone())),
            )
            .tone(tone),
        );
    }

    // Наводка. Своё нажатие, а не довесок к показу: положить второй снимок
    // рядом с первым, не улетая к нему, иначе было бы нельзя. Наводить при
    // этом можно только на то, у чего есть место на шаре, — нарисованный
    // контур или заведённый слой. Одной просьбы показать мало: пока каталог не
    // ответил, о снимке не известно даже того, где он, и нажатие увело бы
    // взгляд к шару, на котором ничего не изменилось.
    let aimable = matches!(row.outlined, OnOutline::Drawn) || row.globe.laid();
    quick.push(match aimable {
        true => Quick::new(
            theme::glyph::FOCUS,
            "Навести камеру",
            "Навести камеру и выделить".to_string(),
            Msg::OutlineFocus(key),
        ),
        false => Quick::idle(
            theme::glyph::FOCUS,
            "Навести камеру",
            "Наводить не на что: снимка на шаре нет".to_string(),
        ),
    });
    quick
}

/// Треугольник раскрытия. У строки, которой нечего показать, — пусто: место за
/// ней остаётся, поэтому соседи не съезжают, когда в списке есть и то, и другое.
///
/// Содержимое папки к этому моменту может быть ещё не спрошено — раскрытие его
/// и спросит (см. handlers::browse::request_children), поэтому треугольник
/// стоит у всякой папки, а не только у наполненной.
fn twist(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    if !row.expandable() {
        return theme::nothing();
    }
    let open = context.listing.expanded.contains(row.key());
    theme::chrome_icon(
        icon::<Msg>(if open { theme::glyph::CARET } else { theme::glyph::CARET_RIGHT })
            .size(10.0)
            .color(theme::INK_DIM),
    )
    // Поля у́же обычных — по той же причине, что у коробочки отметки.
    .padding(1.0)
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_BUTTON))
    .on_press(Msg::In(view, ViewMsg::Expand(row.key().to_string())))
    .into()
}

/// Коробочка выбора.
///
/// Выбор — набор строк для пакетных действий: выбранное удаляют и качают
/// (см. заголовок списка). Шара он не касается вовсе — ни контура, ни показа:
/// у тех свои значки и свои состояния (см. handlers::outline).
///
/// Нет её только у папки пути: в папку заходят, а выбирают то, что лежит в
/// каталоге или на диске. Снимок, лежащий каталогом (.SAFE, .SEN3), — тоже
/// «папка» по укладке, но выбирают как раз его.
///
/// Выключенной она бывает у строки без ключа, а такой в списке взяться неоткуда
/// (см. [`Row::key`]) — коробочка эта сторожевая: нарисовать её всё равно надо,
/// иначе колонка рвётся, а рычаг, который виден и ничего не делает, врёт ровно
/// так же, как и его отсутствие. Выключенный говорит правду и объясняет её
/// подсказкой.
///
/// С подсказкой, и это не украшение: действует выбор не здесь, а кнопками в
/// заголовке списка, и без подписи коробочка предлагает «отметить» неизвестно
/// для чего.
fn check(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    if matches!(row.kind, RowKind::Folder) {
        return theme::nothing();
    }
    if !row.choosable() {
        return hinted(
            theme::row_check(None),
            "Выбрать нечем: у записи нет ключа, которым её адресуют",
        );
    }
    let key = row.choice_key();
    let marked = context.listing.selected.contains_key(key);
    hinted(
        theme::row_check(Some(marked)).on_press(Msg::In(view, ViewMsg::Check(key.to_string()))),
        match marked {
            false => "Выбрать",
            true => "Снять выбор",
        },
    )
}

/// Значок-кнопка: чем нарисован, каким лицом стоит, что скажет подсказка и что
/// пошлёт. Ряды таких кнопок стоят справа в строке таблицы, в списке слоёв и в
/// заголовке списка — собираются они поэтому здесь, а не в каждом месте
/// по-своему: иначе одинаковые с виду значки разъезжаются лицом и подсказкой.
pub fn icon_button(
    glyph: &'static str,
    tone: theme::IconTone,
    hint: &str,
    message: Msg,
) -> Element<Msg> {
    hinted(theme::row_button_icon(glyph, tone).on_press(message), hint)
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

    // Главное действие строки — скачать, приостановить, открыть. Значком оно
    // не стои́т: в строке остались только те два, что кладут снимок на шар
    // (см. [`quick`]). Переход вглубь пунктом не идёт по той же причине, по
    // какой не шёл значком, — внутрь ведёт нажатие на саму строку.
    if let Some(action) = primary(view, row).filter(|action| !action.transition) {
        // Со значком: пункт этот главный, и знак у него тот же, каким действие
        // называют везде — иначе одно и то же читалось бы как разное.
        items.push(Item::new(action.hint, action.message).glyph(action.glyph));
    }
    let key = row.snapshot_key().to_string();
    // Снимок, лежащий папкой, скачивается целиком: его файлы разложены по
    // ярусам, и качать их по одному, обойдя каталог руками, — работа, которую
    // приложение умеет сделать само (см. handlers::library::on_download_snapshot).
    // Доведённому это предлагать незачем: он уже весь на диске.
    if row.kind.is_product()
        && row.kind.is_folder()
        && !key.is_empty()
        && !matches!(row.status, RowStatus::Complete)
    {
        // Вес — в подписи: одно нажатие ставит в очередь весь снимок, а это
        // гигабайты, и узнавать об этом по счётчику закачек поздно. Каталог
        // размера папки не знает (в S3 за префиксом его нет), и тогда сказано
        // просто, что качается снимок целиком.
        let label = match row.size > 0 {
            true => format!("Скачать снимок целиком — {}", format::bytes(row.size)),
            false => "Скачать снимок целиком".to_string(),
        };
        items.push(Item::new(label, Msg::DownloadSnapshot(key.clone())).glyph(theme::glyph::DOWNLOAD));
    }
    // Смотреть снимок — его собственное действие, чем бы снимок ни лежал:
    // папкой ярусов или единственным файлом. Условие то же, что у значка шара,
    // и это не совпадение — вопрос у них один («можно ли это показать»), и два
    // ответа на него разошлись бы молча.
    // Кроме случая, когда главным действием уже стои́т «Открыть»: это тот же
    // глаз и тот же смысл, и два таких пункта подряд читались бы как два
    // разных действия.
    let opens = matches!(row.status, RowStatus::Complete) && !row.kind.is_folder();
    if row.is_snapshot() && row.viewable && !key.is_empty() && !opens {
        items.push(
            Item::new("Смотреть снимок", Msg::In(view, ViewMsg::PreviewProduct(key.clone())))
                .glyph(theme::glyph::EYE),
        );
    }
    // Показать на шаре можно всё, у чего есть ключ провайдера: у найденного
    // продукт с контуром уже под рукой, у строки каталога или скачанного его
    // восстанавливает провайдер по ключу (см. handlers::overlay). У снимка
    // этот пункт стоит значком в самой строке и здесь не повторяется.
    //
    // Кладётся при этом снимок, а не файл: ни контура, ни растра у отдельного
    // файла не бывает. Отсюда и ключ, и подпись: она называет то, что случится
    // по нажатию, а знает это строка потому же, почему знает значок снимка, —
    // о шаре она отвечает снимком, которому принадлежит (см.
    // `rows::from_key`).
    //
    // Папке пути этот пункт не достаётся: снимка за ней нет, и каталог на её
    // имя отвечает «нет такого продукта» — предлагать это значит обещать
    // отказ.
    if !row.identifier.is_empty() && !row.is_snapshot() && !matches!(row.kind, RowKind::Folder) {
        let label = match row.globe.any() {
            true => "Снять снимок с шара",
            false => "Показать снимок на шаре",
        };
        items.push(Item::new(label, Msg::In(view, ViewMsg::GlobeToggle(row.product_key().to_string()))));
    }
    // Показывать папку, которая и так открыта, незачем: пункт вёл бы туда,
    // где пользователь уже стоит.
    if !row.folder().is_empty() && !crate::module::components::row::bare(here).ends_with(row.folder())
    {
        items.push(Item::new(
            "Показать в каталоге",
            Msg::In(view, ViewMsg::InCatalog(row.snapshot_key().to_string())),
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
    // Сложенной строке качать нечего: её ключ — путь снимка в хранилище, и
    // послать его в закачку значит попросить скачать папку одним объектом.
    // Снимок целиком качает свой значок (см. `quick`).
    if !row.identifier.is_empty() && !row.folded && matches!(row.status, RowStatus::Complete) {
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
    // Пауза на снимок целиком — по той же причине, по какой он целиком
    // качается: файлов у него под три десятка, и жать паузу на каждом значит
    // просить пользователя сделать за нас то, что мы же и сложили. Предлагается
    // только пока есть что останавливать.
    //
    // Идёт ли по снимку закачка, сложенная строка видит по своим файлам, а
    // строка каталога — по себе самой: файлов под ней не приезжало, и о них
    // она знает ровно то, что сказала библиотека про свой префикс (см.
    // `rows::from_key`).
    let downloading = match row.folded {
        true => row.children.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. })),
        false => row.kind.is_folder() && matches!(row.status, RowStatus::Downloading { .. }),
    };
    if downloading && !row.product.is_empty() {
        items.push(Item::new("Приостановить снимок", Msg::PauseSnapshot(row.product.clone())));
    }
    // У сложенной строки своего имени нет — она и не запись, а снимок.
    // Выбрасывается он целиком: раскрывать его, чтобы удалить файлы по одному,
    // значит просить пользователя сделать за нас то, что мы же и сложили.
    if row.folded && !row.product.is_empty() {
        items.push(
            Item::new("Удалить снимок с диска", Msg::DeleteSnapshot(row.product.clone())).danger(),
        );
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::state::{BrowseState, State, ViewKind};

    fn state_view() -> (State, ViewId) {
        let mut state =
            State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние");
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
        (state, view)
    }

    fn snapshot(outlined: OnOutline) -> Row {
        Row {
            outlined,
            ..Row::container_row(
                "eodata/store/A.SAFE".to_string(),
                "A.SAFE".to_string(),
                RowStatus::Remote,
                RowKind::Product { folder: false },
            )
        }
    }

    /// В строке стоят ровно три значка, и все три про шар: контур, растр и
    /// наводка.
    ///
    /// Остальное уехало пунктами меню, и вернувшийся значок отнял бы место у
    /// имени. У не-снимка нет и этих трёх: ни контура, ни растра, ни своего
    /// места на шаре у файла и у папки пути не бывает.
    #[test]
    fn the_row_offers_only_the_three_globe_icons() {
        let (_state, view) = state_view();

        let icons = quick(view, &snapshot(OnOutline::Off));
        let glyphs: Vec<&str> = icons.iter().map(|q| q.glyph).collect();
        assert_eq!(glyphs, vec![theme::glyph::OUTLINE, theme::glyph::GLOBE, theme::glyph::FOCUS]);
        // В тесноте значок становится пунктом меню и обязан называть действие.
        assert!(icons.iter().all(|q| !q.label.is_empty()), "безымянный пункт меню");

        let file = Row::container_row(
            "eodata/store/dem.tif".to_string(),
            "dem.tif".to_string(),
            RowStatus::Remote,
            RowKind::File,
        );
        assert!(quick(view, &file).is_empty(), "у файла шара не бывает");
    }

    /// Показ и контур — переключатели с одной механикой: нажатие кладёт,
    /// второе снимает. Наводка к ним не примешана — это третье нажатие, и
    /// нажать его можно только тогда, когда снимок на шаре есть.
    #[test]
    fn the_globe_icons_toggle_and_the_aim_stands_apart() {
        let (_state, view) = state_view();
        let quick_of = |row: &Row| quick(view, row);

        let idle = quick_of(&snapshot(OnOutline::Off));
        assert!(matches!(idle[0].message, Some(Msg::OutlineToggle(_))), "контур — переключатель");
        assert!(
            matches!(idle[1].message, Some(Msg::In(_, ViewMsg::GlobeToggle(_)))),
            "растр — тоже переключатель"
        );
        assert!(idle[2].message.is_none(), "наводить не на что, и нажатия нет");
        assert_eq!(idle[2].tone, theme::IconTone::Idle);

        // Очерченному наводка есть на что: контур на шаре нарисован.
        let drawn = quick_of(&snapshot(OnOutline::Drawn));
        assert!(matches!(drawn[2].message, Some(Msg::OutlineFocus(_))));

        // И лежащему растром — тоже, даже если контура ему никто не просил.
        let laid = Row {
            globe: OnGlobe::Laid { hidden: false, progress: Default::default() },
            ..snapshot(OnOutline::Off)
        };
        assert!(matches!(quick_of(&laid)[2].message, Some(Msg::OutlineFocus(_))));

        // Слой ещё собирается — рамка у него уже посчитана, наводить есть на
        // что.
        let assembling = Row { globe: OnGlobe::Assembling, ..snapshot(OnOutline::Off) };
        assert!(matches!(quick_of(&assembling)[2].message, Some(Msg::OutlineFocus(_))));

        // А одной просьбы мало: пока каталог не ответил, о снимке не известно
        // даже того, где он.
        let asked = Row { globe: OnGlobe::Asked, ..snapshot(OnOutline::Off) };
        assert!(quick_of(&asked)[2].message.is_none(), "наводка обещает несуществующее место");
        // Значок шара при этом горит вполсилы и снимает просьбу: нажатие
        // принято, и отменяется оно тем же значком.
        assert_eq!(quick_of(&asked)[1].tone, theme::IconTone::Half);
        assert!(matches!(quick_of(&asked)[1].message, Some(Msg::In(_, ViewMsg::GlobeToggle(_)))));
    }

    /// Зелёным горит сделанное, вполсилы — начатое и несбывшееся, покоем —
    /// нетронутое. Подсказки при этом все разные: одинаковая на двух лицах
    /// значит, что одно из них необъяснимо.
    #[test]
    fn the_outline_icon_burns_only_when_drawn() {
        let (_state, view) = state_view();
        let tone = |outlined| quick(view, &snapshot(outlined))[0].tone;

        assert_eq!(tone(OnOutline::Drawn), theme::IconTone::Lit);
        assert_eq!(tone(OnOutline::Off), theme::IconTone::Rest);
        for half in [OnOutline::Asking, OnOutline::Blank, OnOutline::Failed] {
            assert_eq!(tone(half), theme::IconTone::Half, "{:?} горит не вполсилы", half);
        }

        let hints: std::collections::HashSet<String> =
            [OnOutline::Off, OnOutline::Asking, OnOutline::Blank, OnOutline::Failed, OnOutline::Drawn]
                .into_iter()
                .map(|outlined| quick(view, &snapshot(outlined))[0].hint.clone())
                .collect();
        assert_eq!(hints.len(), 5, "два лица объяснены одной подсказкой");
    }

    /// «Смотреть снимок» не встаёт рядом с «Открыть»: это тот же глаз и тот же
    /// смысл, и два таких пункта подряд читались бы как два разных действия.
    #[test]
    fn the_menu_does_not_offer_the_same_eye_twice() {
        let (_state, view) = state_view();
        let named = |row: &Row| -> Vec<String> {
            menu_items(view, row, "").iter().map(|item| item.named().to_string()).collect()
        };

        let mut done = snapshot(OnOutline::Off);
        done.status = RowStatus::Complete;
        done.name = "A.SAFE".to_string();
        let items = named(&done);
        assert!(items.contains(&"Открыть".to_string()));
        assert!(!items.contains(&"Смотреть снимок".to_string()), "два глаза подряд");

        // А у того, что на диске не лежит, «Смотреть снимок» — единственный
        // способ его увидеть, и он остаётся.
        let items = named(&snapshot(OnOutline::Off));
        assert!(items.contains(&"Смотреть снимок".to_string()));
    }

    /// Колонки отметки и раскрытия появляются только там, где им есть что
    /// показать: пустой столбец сдвигал бы весь список ради того, чего в нём
    /// не бывает.
    #[test]
    fn optional_columns_appear_only_where_they_are_needed() {
        let wide = 1400.0;
        let bare = fit(wide, Optional::default()).columns;
        assert!(!bare.contains(&Column::Check) && !bare.contains(&Column::Twist));

        let full = fit(wide, Optional { twisty: true, checkable: true }).columns;
        assert_eq!(full.first(), Some(&Column::Check));
        assert_eq!(full.get(1), Some(&Column::Twist));
    }

    /// В узком месте колонки уступают место по одной, а имя не сжимается ниже
    /// своего минимума: список без даты читается, список без имени — нет.
    #[test]
    fn narrow_panes_drop_columns_instead_of_squeezing_the_name() {
        let optional = Optional { twisty: true, checkable: true };
        let fit = fit(420.0, optional);
        assert!(fit.name >= NAME_MIN, "имени досталось {}", fit.name);
        assert!(!fit.compact, "значки уступают только когда имени не остаётся вовсе");
        assert!(fit.columns.contains(&Column::Name) && fit.columns.contains(&Column::Actions));
        assert!(!fit.columns.contains(&Column::Progress), "справочное уходит первым");
    }

    /// Уступать бывает нечему: в половине узкого окна одни лишь кнопки строки
    /// занимают половину места. Тогда имя получает остаток — и говорит об этом
    /// честно, потому что по названной ширине считается его многоточие.
    #[test]
    fn a_pane_too_narrow_for_the_minimum_reports_what_is_left() {
        let optional = Optional { twisty: false, checkable: true };
        let fit = fit(272.0, optional);

        assert!(fit.name < NAME_MIN, "проверяется именно нехватка, а досталось {}", fit.name);
        assert!(!fit.columns.contains(&Column::Icon), "значок уступает последним");
        // Ровно то, что достанется тянущейся колонке: всё место минус
        // фиксированные колонки, отступы экрана и собственные поля ячейки.
        let fixed: f32 = fit.columns.iter().map(|column| width_of(*column)).sum();
        assert_eq!(fit.name, 272.0 - fixed - theme::GUTTER * 2.0 - CELL_PADDING * 2.0);
    }

    /// Панель в свой предел (`MIN_PANE`) не оставляла имени ни точки: одни лишь
    /// кнопки строки шире её. Строка из одних значков не строка — поэтому
    /// уступают и значки, уходя пунктами в меню.
    #[test]
    fn the_narrowest_pane_keeps_the_name_and_moves_the_icons_into_the_menu() {
        let optional = Optional { twisty: true, checkable: true };
        let tight = fit(crate::module::state::MIN_PANE, optional);
        assert!(tight.compact, "значки не уступили");
        assert!(tight.name > 0.0, "имени опять не осталось");
        assert!(tight.columns.contains(&Column::Name) && tight.columns.contains(&Column::Actions));

        // Просторной панели это не касается: там значки на месте.
        assert!(!fit(900.0, optional).compact);
    }
}
