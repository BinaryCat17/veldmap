//! components/file_list.rs — рендеринг строки файла и списка из них.
//!
//! Ничего не решает про состояние файла: получает готовый `RowStatus` (см.
//! `components::row`) и только рисует. Поэтому здесь нет ни доступа к
//! закачкам, ни к снимку диска — раньше состояние реконструировалось прямо
//! тут вложенными if'ами по пяти переменным.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, container, Element, Length, Alignment};
use crate::module::components::{Row, RowStatus};
use crate::module::styles;

/// Какие входные методы модуля вызывают кнопки элемента списка.
/// `None` — действие недоступно на этом экране. Значением события идёт ключ
/// провайдера (для download/re-download/resume/cancel и просмотра ещё не
/// скачанного) или имя записи библиотеки (для view/delete) — см. render_item.
#[derive(Clone, Copy, Default)]
pub struct ItemActions<'a> {
    pub browse: Option<&'a str>,
    /// Просмотр скачанного файла — по имени записи библиотеки.
    pub view_local: Option<&'a str>,
    /// Просмотр ещё не скачанного файла: открывается по remote-ключу и
    /// читается по фрагментам прямо с той стороны.
    pub view_remote: Option<&'a str>,
    pub download: Option<&'a str>,
    pub delete: Option<&'a str>,
}

/// "12.3 MB" / "512 B" — байты человекочитаемо, без внешних крейтов.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{} {}", bytes, UNITS[0]);
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", size, UNITS[unit])
}

/// "X / Y (Z%)", если полный размер известен, иначе только скачанное —
/// сервер мог не прислать Content-Length (см. network::download.rs).
/// Процент считается здесь из байт, а не берётся готовым: два числа об одном
/// и том же неизбежно расходятся (см. `Download`).
fn amount_text(done: u64, total: u64, with_percent: bool) -> String {
    if total == 0 {
        // Ни сколько всего, ни сколько сделано — закачка зарегистрирована,
        // но первых байт ещё нет. Честное "неизвестно", а не ноль.
        return if done == 0 { "Starting...".to_string() } else { format_bytes(done) };
    }
    let head = format!("{} / {}", format_bytes(done), format_bytes(total));
    if with_percent {
        format!("{} ({:.0}%)", head, done as f64 / total as f64 * 100.0)
    } else {
        head
    }
}

/// Три колонки правой части строки — КАЖДАЯ своей фиксированной ширины,
/// одинаковой для всех состояний строки. Именно фиксация каждой колонки по
/// отдельности (а не общего блока текста с "резиновым" промежутком внутри)
/// даёт настоящую сетку: начало каждой колонки — детерминированная сумма
/// ширин предыдущих колонок и отступов, не зависящая от того, что внутри.
/// Колонка под иконку узкая — под один глиф, не под текст.
const STATUS_LABEL_WIDTH: f32 = 24.0;
/// Запас под самый длинный вероятный вариант — активная закачка с
/// процентом: "999.9 MB / 999.9 MB (100%)".
const STATUS_AMOUNT_WIDTH: f32 = 220.0;
const STATUS_BUTTONS_WIDTH: f32 = 110.0;

/// Общая обвязка правой части строки — см. STATUS_*_WIDTH выше.
fn status_row(label: Element<()>, amount: Element<()>, buttons: Vec<Element<()>>) -> Element<()> {
    row![
        container(label).width(Length::Fixed(STATUS_LABEL_WIDTH)),
        container(amount).width(Length::Fixed(STATUS_AMOUNT_WIDTH)),
        container(row(buttons).spacing(6.0).align_items(Alignment::Center))
            .width(Length::Fixed(STATUS_BUTTONS_WIDTH))
            .align_x(Alignment::End),
    ].spacing(6.0).align_items(Alignment::Center).into()
}

fn icon(glyph: &str) -> Element<()> {
    text(glyph).font_family(veld_ui_service_wrap::style::FONT_ICONS).into()
}

pub fn render_item(item: &Row, actions: ItemActions) -> Element<()> {
    let is_folder = matches!(item.status, RowStatus::Folder);

    // Иконка и имя — раздельные Text: у иконки свой шрифт (Icons), имя файла
    // рисуется дефолтным — единая строка ломала бы одно из двух.
    // Имя — Length::Fill, а не Shrink по умолчанию: иначе при сужении окна
    // текст не переносится, а рисуется поверх границ карточки (Shrink меряет
    // себя без учёта родительских границ). Fill нужен на ОБОИХ уровнях — и
    // на самом Text, и на обёртывающем Row: Fill-текст внутри Shrink-строки
    // не получает реальной ширины от кнопки-родителя и схлопывается в
    // минимальную (отсюда вертикальный "по одной букве" — уже проверено).
    let title = row![
        icon(if is_folder { "\u{f07b}" } else { "\u{f016}" }),
        text(item.title.clone()).width(Length::Fill),
    ].spacing(6.0).align_items(Alignment::Center).width(Length::Fill);

    // Одна и та же карточка (см. styles::file_button_style) для любой строки:
    // `on_press_with` навешивается только там, где есть реальное действие,
    // остальные — та же кнопка без обработчика (хост рисует её в стиле
    // `disabled`, см. styles.rs).
    let card = styles::apply_file(button(title)).width(Length::Fill);
    let main_button: Element<()> = match (&item.status, actions.browse, actions.view_local) {
        (RowStatus::Folder, Some(m), _) => card.on_press_with(m, item.identifier.clone()).into(),
        // Открыть можно только по-настоящему скачанную запись — недокачанную
        // библиотека и не отдаст.
        (RowStatus::Complete { .. }, _, Some(m)) => card.on_press_with(m, item.name.clone()).into(),
        _ => card.into(),
    };

    // Кнопка закачки скрыта, если remote-ключ неизвестен — лучше не предлагать
    // действие, чем слать заведомо неверный запрос.
    let download_button = |glyph: &str, color| {
        (!item.identifier.is_empty()).then(|| actions.download.map(|m| {
            styles::icon_button(glyph, color).on_press_with(m, item.identifier.clone()).into()
        })).flatten()
    };
    // Просмотр без скачивания — там же, где и скачивание: обе кнопки живут
    // от remote-ключа, локального файла для них ещё нет.
    let view_remote_button = || {
        (!item.identifier.is_empty()).then(|| actions.view_remote.map(|m| -> Element<()> {
            styles::icon_button("\u{f06e}", styles::COLOR_PRIMARY).on_press_with(m, item.identifier.clone()).into()
        })).flatten()
    };
    // Удалять есть что, пока запись в библиотеке есть — даже если данных на
    // диске ещё нет: намерение тоже удаляется.
    let delete_button = || {
        (!item.name.is_empty()).then(|| actions.delete.map(|m| -> Element<()> {
            styles::icon_button("\u{f1f8}", styles::COLOR_DANGER).on_press_with(m, item.name.clone()).into()
        })).flatten()
    };

    let status: Element<()> = match &item.status {
        RowStatus::Folder => status_row(text("").into(), text("").into(), Vec::new()),

        RowStatus::Remote => status_row(
            text("").into(), text("").into(),
            view_remote_button().into_iter()
                .chain(download_button("\u{f019}", styles::COLOR_PRIMARY)).collect(),
        ),

        // Пауза, не стоп: повторное нажатие на скачиваемый ключ обрывает
        // текущий поток, но скачанное библиотека сохраняет и следующее
        // "скачать" продолжит с оборванного байта — по факту это пауза.
        RowStatus::Downloading { done, total } => status_row(
            icon("\u{f254}"),
            text(amount_text(*done, *total, true)).size(13.0).into(),
            download_button("\u{f04c}", styles::COLOR_WARNING).into_iter()
                .chain(delete_button()).collect(),
        ),

        // Play, не refresh: это продолжение (библиотека сама подхватит
        // недокачанное), а не скачивание с нуля — зрительно должно читаться
        // как обратное действие "паузе" выше. Только иконка, без подписи
        // "Incomplete" — цвет и форма уже достаточно говорят о статусе.
        RowStatus::Paused { done, total } => status_row(
            text("\u{f071}").font_family(veld_ui_service_wrap::style::FONT_ICONS).color(styles::COLOR_WARNING).into(),
            text(amount_text(*done, *total, false)).size(13.0).into(),
            download_button("\u{f04b}", styles::COLOR_PRIMARY).into_iter()
                .chain(delete_button()).collect(),
        ),

        // Просмотр — по имени записи, re-download — по ключу провайдера:
        // это разные значения (см. Row).
        RowStatus::Complete { size } => status_row(
            icon("\u{f00c}"),
            text(format_bytes(*size)).size(13.0).into(),
            actions.view_local.map(|m| -> Element<()> {
                    styles::icon_button("\u{f06e}", styles::COLOR_PRIMARY).on_press_with(m, item.name.clone()).into()
                })
                .into_iter()
                .chain(download_button("\u{f021}", styles::COLOR_RELOAD))
                .chain(delete_button())
                .collect(),
        ),
    };

    row![main_button, status]
        .width(Length::Fill)
        .spacing(10.0)
        .align_items(Alignment::Center)
        .into()
}

/// Принимает итератор, а не срез: экран Downloaded делит строки на секции
/// через `partition` и держит `Vec<&Row>`, копировать их ради рендера незачем.
pub fn render_list<'a>(items: impl IntoIterator<Item = &'a Row>, actions: ItemActions) -> Element<()> {
    column(items.into_iter().map(|item| render_item(item, actions)))
        .width(Length::Fill)
        .spacing(8.0)
        .into()
}

/// Список файлов или текст-заглушка, если он пуст — этот выбор одинаков на
/// всех трёх экранах (Browse/Search/Downloaded), различается только
/// сообщение-заглушка.
pub fn items_or_message(items: &[Row], actions: ItemActions, empty_message: &str) -> Element<()> {
    if items.is_empty() {
        column![text(empty_message).size(16.0)].into()
    } else {
        render_list(items, actions)
    }
}
