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
use crate::module::components::{arrange::Line, format, OnGlobe, Row, RowKind, RowStatus};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Колонка таблицы. Перечислением, а не списком ширин: колонок столько же,
/// сколько ячеек в строке, и связывать одно с другим позицией в массиве значит
/// однажды сдвинуть ячейку на соседнюю колонку.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    /// Отметка снимка: отмеченные очерчены на шаре (см. handlers::outline).
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
    /// Есть что отметить — то есть в списке стоят снимки.
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
            // обещает очертить всю выдачу, а очертит двадцать строк.
            Column::Check => hinted(
                theme::row_check(all).on_press(Msg::In(view, ViewMsg::CheckShown(!all))),
                match all {
                    true => "Убрать контуры показанного",
                    false => "Очертить показанное на шаре",
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
                    RowStatus::Paused { trouble, .. } if !trouble.is_empty() => tooltip(
                        cell,
                        trouble.clone(),
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
    let onto_globe = row_data.globe.share();
    let height = theme::ROW_HEIGHT - if row_data.globe.any() { ONTO_GLOBE } else { 0.0 };
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
    match (row_data.globe.any(), onto_globe) {
        (true, Some(share)) => lines.push(
            // Подпись у полосы своя, а не только у значка в правом краю строки:
            // смотрят-то на полосу, и без подписи она — доля неизвестно чего.
            tooltip(
                progress_bar::<Msg>(0.0..=1.0, share)
                    .style(theme::progress(theme::ACCENT))
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
            .into(),
        ),
        // Добыча кончилась, а место остаётся за ней: пустым, чтобы строка не
        // дрогнула. Что снимок на шаре, говорит зажжённый значок.
        (true, None) => lines.push(
            veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(ONTO_GLOBE)).into(),
        ),
        (false, _) => {}
    }
    lines.push(theme::hairline(theme::LINE_ROW));

    column(lines).width(Length::Fill).key(row_data.key().to_string()).into()
}

/// Чем строка выделена среди соседей. Старшинство названо в самой роли
/// (см. [`theme::RowTint`]): подсветка одна на весь экран и старше отметки,
/// которых в списке бывает полсотни.
///
/// Отметка читается здесь той же меркой, что и в коробочке (`check`): у неё
/// один источник — `ListingState::selected`, — а два выражения на этот вопрос
/// однажды разошлись бы, и залитая строка стояла бы с пустой коробочкой.
fn tint(row: &Row, context: Context<'_>) -> theme::RowTint {
    let picked = (!context.picked.is_empty() && row.snapshot_key() == context.picked)
        || row.named(context.target);
    if picked {
        return theme::RowTint::Picked;
    }
    let marked = row.is_snapshot() && context.listing.selected.contains(row.snapshot_key());
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
        // Сорвавшаяся закачка говорит своим голосом: «прервано» — это про
        // остановленную человеком, и молчать о причине там, где он её не
        // выбирал, значит предложить ему нажать «Продолжить» вслепую.
        RowStatus::Paused { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER,
        ),
        RowStatus::Paused { done, total, .. } => (
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
        RowStatus::Downloading { .. } => {
            Primary::new(theme::glyph::PAUSE, "Пауза", Msg::Download(remote()?, row.product.clone()))
        }
        RowStatus::Paused { .. } => Primary::new(
            theme::glyph::PLAY,
            "Продолжить",
            Msg::Download(remote()?, row.product.clone()),
        ),
        _ if row.kind.is_folder() => Primary::transition(
            theme::glyph::ENTER,
            "Перейти",
            Msg::In(view, ViewMsg::Enter(remote()?)),
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
        .map(|Quick { glyph, hint, message, tone }| icon_button(glyph, tone, &hint, message))
        .collect();

    let mut items = menu_items(view, entry, context.here);
    if context.compact {
        // Пунктами — впереди прочего: это главные действия строки, значками
        // они и стояли.
        let moved: Vec<super::menu::Item> = quick(view, entry)
            .into_iter()
            .map(|Quick { hint, message, .. }| super::menu::Item::new(hint, message))
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
    message: Msg,
    /// Горит ли он. Умолчание — покой: состояние есть только у значка шара,
    /// остальные о мире ничего не рассказывают.
    tone: theme::IconTone,
}

impl Quick {
    fn new(glyph: &'static str, hint: String, message: Msg) -> Self {
        Self { glyph, hint, message, tone: theme::IconTone::Rest }
    }

    fn tone(self, tone: theme::IconTone) -> Self {
        Self { tone, ..self }
    }
}

/// Быстрые значки строки по порядку: сначала то, ради чего строку открывают,
/// потом показ на шаре.
///
/// Перехода вглубь среди них нет: внутрь ведёт нажатие на саму строку, и
/// стрелка рядом с ней говорила бы то же самое второй раз. Показать содержимое,
/// не уходя со списка, — дело треугольника (см. `twist`).
///
/// Значок глобуса — только у снимка: класть на шар папку пути или файл внутри
/// снимка нечего, а кнопка, которой нечего сделать, врёт о том, что строка
/// умеет. Остальным этот пункт по-прежнему доступен из меню (см. `menu_items`).
fn quick(view: ViewId, row: &Row) -> Vec<Quick> {
    let mut quick = Vec::new();
    let key = row.snapshot_key().to_string();
    let main = primary(view, row).filter(|action| !action.transition);
    // Показ уже стои́т главным действием — у скачанного файла это «открыть».
    let showing = main.as_ref().is_some_and(|action| action.glyph == theme::glyph::EYE);
    if let Some(action) = main {
        quick.push(Quick::new(action.glyph, action.hint.to_string(), action.message));
    }
    // Снимок, лежащий папкой, скачивается целиком: его файлы разложены по
    // ярусам, и качать их по одному, обойдя каталог руками, — работа, которую
    // приложение умеет сделать само (см. handlers::library::on_download_snapshot).
    // Доведённому это предлагать незачем: он уже весь на диске.
    if row.kind.is_product()
        && row.kind.is_folder()
        && !key.is_empty()
        && !matches!(row.status, RowStatus::Complete)
    {
        // Вес — в подсказке: одно нажатие ставит в очередь весь снимок, а это
        // гигабайты, и узнавать об этом по счётчику закачек поздно. Каталог
        // размера папки не знает (в S3 за префиксом его нет), и тогда сказано
        // просто, что качается снимок целиком.
        let hint = match row.size > 0 {
            true => format!("Скачать снимок целиком — {}", format::bytes(row.size)),
            false => "Скачать снимок целиком".to_string(),
        };
        quick.push(Quick::new(theme::glyph::DOWNLOAD, hint, Msg::DownloadSnapshot(key.clone())));
    }
    // Смотреть снимок — его собственное действие, и значок у него один, чем бы
    // снимок ни лежал: папкой ярусов или единственным файлом. Условие здесь то
    // же, что у значка глобуса, и это не совпадение — вопрос у них один
    // («можно ли это показать»), и два ответа на него разошлись бы молча: у
    // гранулы Sentinel-5P и у климатики значок глобуса стоял, а значка
    // просмотра не было, хотя смотреть их — основное занятие.
    if row.is_snapshot() && row.viewable && !key.is_empty() && !showing {
        quick.push(Quick::new(
            theme::glyph::EYE,
            "Смотреть снимок".to_string(),
            Msg::In(view, ViewMsg::PreviewProduct(key.clone())),
        ));
    }
    // `viewable` — не «покажется наверняка», а «есть смысл предлагать»
    // (см. `Row::viewable`): значок над сырьём уровня 0 или над архивом обещает
    // то, чего не бывает.
    if row.is_snapshot() && row.viewable && !key.is_empty() {
        // Значок горит, когда снимок лежит на шаре растром, — и это
        // единственное, по чему в списке видно, что именно там лежит. Подпись
        // при этом называет то, что случится по нажатию, а не то, что есть:
        // у лежащего это переход к нему, а не второе наложение.
        let (tone, hint) = match row.globe {
            OnGlobe::Off => (theme::IconTone::Rest, "На глобус"),
            OnGlobe::Assembling => (theme::IconTone::Half, "Кладётся на глобус…"),
            OnGlobe::Laid { hidden: true, .. } => {
                (theme::IconTone::Half, "На шаре, скрыт — показать и навести")
            }
            OnGlobe::Laid { hidden: false, .. } => {
                (theme::IconTone::Lit, "На шаре — навести и выделить")
            }
        };
        // Полосу под строкой рисует ход добычи, и объяснить её больше негде:
        // подписи у полосы нет, а без неё она — доля неизвестно чего.
        let hint = match row.globe.said() {
            Some(said) => format!("{} · {}", hint, said),
            None => hint.to_string(),
        };
        quick.push(
            Quick::new(theme::glyph::GLOBE, hint, Msg::In(view, ViewMsg::GlobeShow(key))).tone(tone),
        );
    }
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

/// Коробочка отметки. Отмеченные снимки очерчены на шаре, и другого способа
/// очертить их нет (см. handlers::outline) — поэтому она стоит только у
/// снимка: у папки пути и у файла внутри снимка контура не бывает.
///
/// С подсказкой, и это не украшение: коробочка — единственный рычаг, чьё
/// действие происходит не здесь, а на другой вкладке. Без подписи она
/// предлагает «отметить» неизвестно для чего.
fn check(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    let key = row.snapshot_key();
    if !row.is_snapshot() || key.is_empty() {
        return theme::nothing();
    }
    let marked = context.listing.selected.contains(key);
    hinted(
        theme::row_check(marked).on_press(Msg::In(view, ViewMsg::Check(key.to_string()))),
        match marked {
            true => "Убрать контур с шара",
            false => "Очертить на шаре",
        },
    )
}

/// Значок-кнопка строки: чем нарисован, каким лицом стоит, что скажет подсказка
/// и что пошлёт. Ряд таких кнопок стоит и справа в строке таблицы, и в списке
/// слоёв — собирается он поэтому здесь, а не в каждом списке по-своему: иначе
/// одинаковые с виду значки разъезжаются лицом и подсказкой.
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

    // Показать на шаре можно всё, у чего есть ключ провайдера: у найденного
    // продукт с контуром уже под рукой, у строки каталога или скачанного его
    // восстанавливает провайдер по ключу (см. handlers::overlay). У снимка
    // этот пункт стоит значком в самой строке и здесь не повторяется.
    if !row.identifier.is_empty() && !row.is_snapshot() {
        items.push(Item::new(
            "Показать на шаре",
            Msg::In(view, ViewMsg::GlobeShow(row.snapshot_key().to_string())),
        ));
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
    if row.folded
        && !row.product.is_empty()
        && row.children.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. }))
    {
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
