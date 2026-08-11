//! components/table.rs — таблица списка: шапка, строки, меню строки.
//!
//! Одна на все три вида. Что можно сделать со строкой, выводится из неё самой,
//! а не из вида: папку открывают там, где она есть, скачанное смотрят там, где
//! оно скачано, — и вкладке нечего к этому добавить. Поэтому и колонки, и
//! кнопки во всех трёх видах одни и те же.

use veld_ui_service_wrap::{column, row, Keyed, Row as RowBuilder};
use crate::proto::ui_service::{
    button, container, icon, mono, popover, progress_bar, space, text, tooltip,
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
const FORMAT: f32 = 56.0;
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
const GLYPH_FOLDER: &str = "\u{f07b}";
const GLYPH_IMAGE: &str = "\u{f1c5}";
const GLYPH_FILE: &str = "\u{f016}";
const GLYPH_ENTER: &str = "\u{f061}";
const GLYPH_DOWNLOAD: &str = "\u{f019}";
const GLYPH_PAUSE: &str = "\u{f04c}";
const GLYPH_PLAY: &str = "\u{f04b}";
const GLYPH_EYE: &str = "\u{f06e}";
const GLYPH_MORE: &str = "\u{f141}";

/// Расширения, у которых есть превью, — по ним же выбрана иконка строки.
const IMAGE_FORMATS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "jp2", "webp"];

/// Строка сетки: по ячейке на колонку, в порядке `COLUMNS`. Ячейка обрезает
/// содержимое по своим границам — длинное имя иначе рисуется поверх соседней
/// колонки (см. `Wrapping` в types.proto).
///
/// Высоту ставит вызывающий: у шапки она своя.
fn grid(cells: [Element<Msg>; COLUMNS.len()]) -> RowBuilder<Msg> {
    row(cells.into_iter().zip(COLUMNS).map(|(content, width)| {
        container(content)
            .width(width)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left: CELL_PADDING, right: CELL_PADDING })
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

/// Ячейка, в которой ничего не показано: у заголовка группы нет ни формата, ни
/// размера, а место под них остаётся.
fn empty() -> Element<Msg> {
    space::<Msg>(Length::Fixed(0.0), Length::Fixed(0.0)).into()
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
        empty(),
        label("ИМЯ").into(),
        label("ФОРМАТ").into(),
        label("ДАТА").into(),
        label("РАЗМЕР").into(),
        label("СОСТОЯНИЕ").into(),
        label("ЗАГРУЗКА").into(),
        empty(),
    ])
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
    let cells = grid([
        glyph(GLYPH_FOLDER, theme::ACCENT, depth),
        text::<Msg>(format::ellipsize(title, format::mono_fit(context.name_width, theme::TEXT_LABEL)))
            .size(theme::TEXT_LABEL)
            .color(theme::INK)
            .weight(FontWeight::WeightMedium)
            .single_line()
            .into(),
        empty(),
        empty(),
        empty(),
        text::<Msg>(meta.to_string()).size(theme::TEXT_SMALL).color(theme::INK_DIM).single_line().into(),
        empty(),
        empty(),
    ])
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

    // Имя моноширинным: колонка считается по знакам, а у пропорционального
    // шрифта их ширина разная. Папка — обычным: это подпись, а не значение.
    let title = format::ellipsize(&row_data.title, format::mono_fit(context.name_width, theme::TEXT_MONO));
    let name: Element<Msg> = if row_data.is_folder {
        text(title).size(theme::TEXT_LABEL).color(theme::INK).weight(FontWeight::WeightMedium).single_line().into()
    } else {
        mono(title).size(theme::TEXT_MONO).color(theme::INK_SOFT).into()
    };

    let cells = grid([
        glyph(row_glyph(row_data), if row_data.is_folder { theme::ACCENT } else { theme::INK_FAINT }, depth),
        name,
        text::<Msg>(row_data.format()).size(theme::TEXT_TAG).color(theme::INK_FAINT).single_line().into(),
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
    ])
    .height(Length::Fixed(theme::ROW_HEIGHT));

    // Вся строка — кнопка: нажатие на неё делает то же, что её главная кнопка.
    // Отдельная кнопка при этом остаётся: по ней видно, что именно случится.
    let line = match primary(row_data) {
        Some((_, _, message)) => theme::row_button(button(cells)).on_press(message),
        None => theme::row_button(button(cells)),
    };

    column![
        line.width(Length::Fill).height(Length::Fixed(theme::ROW_HEIGHT)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(row_data.key().to_string())
    .into()
}

/// Содержимое первой колонки: иконка, отодвинутая на свою ступень группировки.
fn glyph(glyph: &str, color: Color, depth: usize) -> Element<Msg> {
    container(icon::<Msg>(glyph).size(13.0).color(color))
        .padding(Padding { top: 0.0, bottom: 0.0, left: depth as f32 * INDENT, right: 0.0 })
        .into()
}

fn row_glyph(row: &Row) -> &'static str {
    if row.is_folder {
        GLYPH_FOLDER
    } else if IMAGE_FORMATS.contains(&row.format().to_lowercase().as_str()) {
        GLYPH_IMAGE
    } else {
        GLYPH_FILE
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
        return empty();
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
        RowStatus::Downloading { .. } => (GLYPH_PAUSE, "Пауза", Msg::Download(remote()?)),
        RowStatus::Paused { .. } => (GLYPH_PLAY, "Продолжить", Msg::Download(remote()?)),
        _ if row.is_folder => (GLYPH_ENTER, "Перейти", Msg::Enter(remote()?)),
        RowStatus::Remote => (GLYPH_DOWNLOAD, "Скачать", Msg::Download(remote()?)),
        RowStatus::Complete => (GLYPH_EYE, "Открыть", Msg::Preview(local()?)),
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
                theme::surface_button(button(centered_glyph(glyph)), false)
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
        let anchor = theme::surface_button(button(centered_glyph(GLYPH_MORE)), open)
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

/// Глиф по центру кнопки: при фиксированном размере кнопки он иначе прижат к
/// левому краю паддинга, а ширина у разных глифов своя.
fn centered_glyph(glyph: &str) -> Element<Msg> {
    container(icon::<Msg>(glyph).size(11.0).color(theme::INK_SOFT))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y()
        .into()
}

/// Всё, что делают со строкой редко. Порядок общий: сначала переходы, потом
/// необратимое.
fn menu_items(row: &Row, here: &str) -> Vec<super::menu::Item> {
    use super::menu::Item;
    let mut items = Vec::new();

    // Показывать папку, которая и так открыта, незачем: пункт вёл бы туда,
    // где пользователь уже стоит.
    if !row.folder().is_empty() && !here.trim_end_matches('/').ends_with(row.folder()) {
        items.push(Item::new("Показать в каталоге", Msg::Enter(format!("{}/", row.folder()))));
    }
    if !row.is_folder && !row.identifier.is_empty() && matches!(row.status, RowStatus::Remote) {
        items.push(Item::new("Смотреть без скачивания", Msg::PreviewRemote(row.identifier.clone())));
    }
    if !row.identifier.is_empty() && matches!(row.status, RowStatus::Complete) {
        items.push(Item::new("Скачать заново", Msg::Download(row.identifier.clone())));
    }
    if !row.name.is_empty() && matches!(row.status, RowStatus::Downloading { .. } | RowStatus::Paused { .. }) {
        items.push(Item::new("Отменить загрузку", Msg::Cancel(row.name.clone())).danger());
    }
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        items.push(Item::new("Удалить с диска", Msg::Delete(row.name.clone())).danger());
    }

    items
}
