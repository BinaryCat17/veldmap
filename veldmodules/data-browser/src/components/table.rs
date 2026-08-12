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
use crate::module::components::{arrange::Line, format, Row, RowStatus};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::{theme, Msg};

/// Колонки таблицы по порядку. Один список на всю таблицу: сетка у шапки,
/// заголовка группы и строки обязана совпадать, а три её копии рано или поздно
/// разъедутся. Колонка имени тянется, остальные фиксированы.
const COLUMNS: [Length; 8] = [
    Length::Fixed(ICON),
    Length::Fill,
    Length::Fixed(FORMAT),
    Length::Fixed(DATE),
    Length::Fixed(SIZE),
    Length::Fixed(STATUS),
    Length::Fixed(PROGRESS),
    Length::Fixed(ACTIONS),
];

const ICON: f32 = 34.0;
/// Под тип продукта, а не под расширение файла: у радара он длинный
/// (`IW_GRDH_1SDV`), и по трём буквам «png» такую колонку не смеришь.
const FORMAT: f32 = 80.0;
const DATE: f32 = 88.0;
const SIZE: f32 = 84.0;
const STATUS: f32 = 104.0;
const PROGRESS: f32 = 132.0;
const ACTIONS: f32 = 64.0;

/// Отступ содержимого ячейки от её границ — по обе стороны.
const CELL_PADDING: f32 = 8.0;

/// Сколько места занимают все колонки, кроме имени, вместе с отступами экрана и
/// собственными полями колонки имени. По нему считается её ширина, а по ней —
/// сколько знаков в неё влезает.
pub const FIXED_WIDTH: f32 =
    ICON + FORMAT + DATE + SIZE + STATUS + PROGRESS + ACTIONS + theme::GUTTER * 2.0 + CELL_PADDING * 2.0;

/// Ступенька группировки.
const INDENT: f32 = 12.0;

/// Высота шапки — своя: в ней подписи, а не строки данных.
const HEADER_HEIGHT: f32 = 22.0;

/// Глифы Font Awesome (шрифт Icons).

/// Расширения, у которых есть превью, — по ним же выбрана иконка строки.
const IMAGE_FORMATS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "jp2", "webp"];

/// Строка сетки: по ячейке на колонку, в порядке `COLUMNS`. Ячейка обрезает
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
fn grid(cells: [Element<Msg>; COLUMNS.len()], indent: f32) -> RowBuilder<Msg> {
    row(cells.into_iter().zip(COLUMNS).enumerate().map(|(column, (content, width))| {
        let (width, left) = match column {
            0 => (Length::Fixed(ICON + indent), CELL_PADDING + indent),
            _ => (width, CELL_PADDING),
        };
        container(content)
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
    /// Ширина колонки имени в точках разметки.
    pub name_width: f32,
    /// Папка, которую сейчас показывают; пусто — вид её не знает (скачанное,
    /// поиск). По ней видно, куда вести «показать в каталоге» бессмысленно.
    pub here: &'a str,
    /// Точка отсчёта для подписей времени. Одна на всю таблицу: строки одного
    /// кадра должны мериться от одного «сейчас», иначе соседние даты
    /// перескакивают через полночь по-разному.
    pub now: i64,
}

/// Шапка. Стоит вне прокрутки, поэтому не уезжает вместе со строками.
pub fn header() -> Element<Msg> {
    let label = |name: &str| {
        text::<Msg>(name.to_string())
            .size(theme::TEXT_HEADER)
            .color(theme::INK_DIM)
            .weight(FontWeight::WeightBold)
            .single_line()
    };
    let head = grid([
        theme::nothing(),
        label("ИМЯ").into(),
        label("ФОРМАТ").into(),
        label("ДАТА").into(),
        label("РАЗМЕР").into(),
        label("СОСТОЯНИЕ").into(),
        label("ЗАГРУЗКА").into(),
        theme::nothing(),
    ], 0.0)
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
pub fn body(lines: &[Line<'_>], context: Context<'_>) -> Element<Msg> {
    column(lines.iter().map(|line| match line {
        Line::Group { title, meta, depth } => group_line(title, meta, *depth, context),
        Line::Entry { row, depth } => entry_line(row, *depth, context),
    }))
    .width(Length::Fill)
    .into()
}

/// Заголовок группы: та же сетка, что у строки, но нажимать в нём нечего.
fn group_line(title: &str, meta: &str, depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid([
        glyph(theme::glyph::FOLDER, theme::ACCENT),
        text::<Msg>(format::ellipsize(title, format::mono_fit(context.name_width - indent, theme::TEXT_LABEL)))
            .size(theme::TEXT_LABEL)
            .color(theme::INK)
            .weight(FontWeight::WeightMedium)
            .single_line()
            .into(),
        theme::nothing(),
        theme::nothing(),
        theme::nothing(),
        text::<Msg>(meta.to_string()).size(theme::TEXT_SMALL).color(theme::INK_DIM).single_line().into(),
        theme::nothing(),
        theme::nothing(),
    ], indent)
    .height(Length::Fixed(theme::ROW_HEIGHT));

    column![
        gutters(cells).background(theme::SHELF),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(format!("group:{}:{}", depth, title))
    .into()
}

fn entry_line(row_data: &Row, depth: usize, context: Context<'_>) -> Element<Msg> {
    let (dot, label, label_color) = status_look(&row_data.status);
    let indent = indent(depth);

    // Имя моноширинным: колонка считается по знакам, а у пропорционального
    // шрифта их ширина разная. Папка — обычным: это подпись, а не значение.
    let title = format::ellipsize(&row_data.title, format::mono_fit(context.name_width - indent, theme::TEXT_MONO));
    let name: Element<Msg> = if row_data.is_folder {
        text(title).size(theme::TEXT_LABEL).color(theme::INK).weight(FontWeight::WeightMedium).single_line().into()
    } else {
        mono(title).size(theme::TEXT_MONO).color(theme::INK_SOFT).into()
    };

    let cells = grid([
        glyph(row_glyph(row_data), if row_data.is_folder { theme::ACCENT } else { theme::INK_FAINT }),
        name,
        // Моноширинным и с усечением по месту: это код, а не слово, и обрезать
        // его молча нельзя — ячейка обрежет по границе, и значение упрётся в
        // соседнюю колонку без единого знака о том, что оно неполное.
        mono::<Msg>(format::ellipsize(
            &row_data.format(),
            format::mono_fit(FORMAT - CELL_PADDING * 2.0, theme::TEXT_TAG),
        ))
        .size(theme::TEXT_TAG)
        .color(theme::INK_FAINT)
        .single_line()
        .into(),
        text::<Msg>(format::date(row_data.date, context.now))
            .size(theme::TEXT_SMALL)
            .color(theme::INK_DIM)
            .single_line()
            .into(),
        container(
            mono::<Msg>(if row_data.size > 0 { format::bytes(row_data.size) } else { String::new() })
                .size(theme::TEXT_SMALL)
                .color(theme::INK_MUTED),
        )
        .width(Length::Fill)
        .align_x(Alignment::End)
        .into(),
        row![
            theme::dot(dot),
            text::<Msg>(label).size(theme::TEXT_SMALL).color(label_color).single_line(),
        ]
        .spacing(6.0)
        .align_items(Alignment::Center)
        .into(),
        progress_cell(&row_data.status),
        actions(row_data, context),
    ], indent)
    .height(Length::Fixed(theme::ROW_HEIGHT));

    // Вся строка — кнопка: нажатие на неё делает то же, что её главная кнопка.
    // Отдельная кнопка при этом остаётся: по ней видно, что именно случится.
    let line = match primary(row_data) {
        Some((_, _, message)) => theme::row_button(cells).on_press(message),
        None => theme::row_button(cells),
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
    icon::<Msg>(glyph).size(13.0).color(color).into()
}

fn row_glyph(row: &Row) -> &'static str {
    if row.is_folder {
        theme::glyph::FOLDER
    } else if IMAGE_FORMATS.contains(&row.format().to_lowercase().as_str()) {
        theme::glyph::IMAGE
    } else {
        theme::glyph::FILE
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
fn primary(row: &Row) -> Option<(&'static str, &'static str, Msg)> {
    // Действие без ключа некому адресовать — лучше не предлагать его вовсе,
    // чем послать заведомо пустой запрос. Отсюда `?` на каждом: чем адресовать
    // это действие, известно раньше, чем оно построено.
    let remote = || (!row.identifier.is_empty()).then(|| row.identifier.clone());
    let local = || (!row.name.is_empty()).then(|| row.name.clone());

    Some(match &row.status {
        RowStatus::Downloading { .. } => (theme::glyph::PAUSE, "Пауза", Msg::Download(remote()?)),
        RowStatus::Paused { .. } => (theme::glyph::PLAY, "Продолжить", Msg::Download(remote()?)),
        _ if row.is_folder => (theme::glyph::ENTER, "Перейти", Msg::Enter(remote()?)),
        RowStatus::Remote => (theme::glyph::DOWNLOAD, "Скачать", Msg::Download(remote()?)),
        RowStatus::Complete => (theme::glyph::EYE, "Открыть", Msg::Preview(local()?)),
        // Про папку, часть которой скачана, главная кнопка сказать не может:
        // «скачать остальное» библиотека не умеет, а «открыть» тут нечего.
        RowStatus::Partial { .. } => return None,
    })
}

/// Две кнопки справа: главное действие и меню всего остального.
fn actions(entry: &Row, context: Context<'_>) -> Element<Msg> {
    let mut buttons = Vec::new();

    if let Some((glyph, hint, message)) = primary(entry) {
        buttons.push(
            tooltip(
                theme::surface_button(row_glyph_icon(glyph), false)
                    .width(Length::Fixed(theme::ROW_BUTTON))
                    .height(Length::Fixed(theme::ROW_BUTTON))
                    .on_press(message),
                hint,
                TooltipPosition::TooltipTop,
            )
            .style(theme::panel())
            .text_size(theme::TEXT_SMALL)
            .padding(6.0)
            .into(),
        );
    }

    let items = menu_items(entry, context.here);
    if !items.is_empty() {
        let menu = Menu::Row(entry.key().to_string());
        let open = context.listing.menu == menu;
        let anchor = theme::surface_button(row_glyph_icon(theme::glyph::MORE), open)
            .width(Length::Fixed(theme::ROW_BUTTON))
            .height(Length::Fixed(theme::ROW_BUTTON))
            .on_press(Msg::OpenMenu(menu));

        buttons.push(
            popover(anchor, super::menu::panel(items))
                .open(open)
                .align_x(Alignment::End)
                .gap(4.0)
                .on_dismiss(Msg::OpenMenu(Menu::Closed))
                .into(),
        );
    }

    container(row(buttons).spacing(6.0).align_items(Alignment::Center))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Center)
        .into()
}

/// Глиф на квадратной кнопке строки. По центру её ставит сама кнопка — это
/// свойство роли, а не забота содержимого (см. `theme::clickable`).
fn row_glyph_icon(glyph: &str) -> Element<Msg> {
    icon::<Msg>(glyph).size(11.0).color(theme::INK_SOFT).into()
}

/// Всё, что делают со строкой редко. Порядок общий: сначала переходы, потом
/// необратимое.
fn menu_items(row: &Row, here: &str) -> Vec<super::menu::Item> {
    use super::menu::Item;
    let mut items = Vec::new();

    // Место на Земле знает только каталог, поэтому пункт есть у найденного и
    // нет у скачанного: у файла на диске контура не осталось.
    if row.located && !row.identifier.is_empty() {
        items.push(Item::new("Показать на шаре", Msg::GlobeShow(row.identifier.clone())));
    }
    // Показывать папку, которая и так открыта, незачем: пункт вёл бы туда,
    // где пользователь уже стоит.
    if !row.folder().is_empty() && !here.trim_end_matches('/').ends_with(row.folder()) {
        items.push(Item::new("Показать в каталоге", Msg::Enter(format!("{}/", row.folder()))));
    }
    if !row.is_folder && !row.identifier.is_empty() && matches!(row.status, RowStatus::Remote) {
        items.push(Item::new("Смотреть без скачивания", Msg::PreviewRemote(row.identifier.clone())));
    }
    // Перекачка сносит имеющийся файл до старта — иначе рядом с ним лёг бы
    // второй, недокачанный (см. data-library::download). Отсюда и пометка.
    if !row.identifier.is_empty() && matches!(row.status, RowStatus::Complete) {
        items.push(Item::new("Скачать заново", Msg::Download(row.identifier.clone())).danger());
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

    items
}
