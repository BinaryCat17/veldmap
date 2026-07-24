//! components/browser_list/view.rs — рендеринг списка файлов

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, container, scrollable, Element, Length, Alignment, Padding};
use crate::module::components::browser_list::BrowserItem;
use crate::module::components::task_manager::{TaskManager, TaskInfo};
use crate::module::styles;

/// Какие входные методы модуля вызывают кнопки элемента списка.
/// `None` — действие недоступно на этом экране. Значением события всегда
/// идёт `s3_key` элемента (для download/re-download/resume/cancel) или
/// `local_path` (для view/delete) — см. render_item.
#[derive(Clone, Copy, Default)]
pub struct ItemActions<'a> {
    pub browse: Option<&'a str>,
    pub view: Option<&'a str>,
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

/// Текст прогресса активной закачки: процент, если сервер прислал
/// Content-Length, иначе только счётчик байт (см. network::download.rs —
/// total_bytes == 0 для ответов без Content-Length).
fn progress_text(task: &TaskInfo) -> String {
    if task.total_bytes > 0 {
        format!("{} / {} ({:.0}%)", format_bytes(task.bytes_downloaded), format_bytes(task.total_bytes), task.progress * 100.0)
    } else if task.bytes_downloaded > 0 {
        format_bytes(task.bytes_downloaded)
    } else {
        "Downloading...".to_string()
    }
}

/// Три колонки правой части строки — КАЖДАЯ своей фиксированной ширины,
/// одинаковой для всех состояний строки (активная закачка / Incomplete /
/// готовый файл / ещё не скачан / папка). Именно фиксация каждой колонки по
/// отдельности (а не общего блока текста с "резиновым" промежутком внутри)
/// даёт настоящую сетку: начало каждой колонки — детерминированная сумма
/// ширин предыдущих колонок и отступов, не зависящая от того, что внутри.
///
/// - `label` — иконка + опциональная подпись ("Incomplete"), у левого края
///   своей колонки (см. `status_row`);
/// - `amount` — сколько скачано/сколько всего, у правого края своей колонки;
/// - кнопки — у правого края своей колонки, тоже независимо от того,
///   сколько их реально показано (2 у активной/недокачанной закачки, 3 у
///   готового файла, 1 у ещё не скачанного).
const STATUS_LABEL_WIDTH: f32 = 110.0;
/// Запас под самый длинный вероятный вариант — активная закачка с
/// процентом: "999.9 MB / 999.9 MB (100%)".
const STATUS_AMOUNT_WIDTH: f32 = 220.0;
const STATUS_BUTTONS_WIDTH: f32 = 110.0;

/// Общая обвязка правой части строки — см. STATUS_*_WIDTH выше.
fn status_row(label: Element<()>, amount: Element<()>, buttons: Vec<Element<()>>) -> Element<()> {
    row![
        container(label).width(Length::Fixed(STATUS_LABEL_WIDTH)),
        container(amount).width(Length::Fixed(STATUS_AMOUNT_WIDTH)).align_x(Alignment::End),
        container(row(buttons).spacing(6.0).align_items(Alignment::Center))
            .width(Length::Fixed(STATUS_BUTTONS_WIDTH))
            .align_x(Alignment::End),
    ].spacing(6.0).align_items(Alignment::Center).into()
}

/// То же самое что `progress_text`, но для недокачанного файла на паузе —
/// TaskInfo уже нет (задача finished/cancelled), есть только текущий размер
/// `.part` на диске (`item.size`) и, если он был виден хоть раз в этой
/// сессии, Content-Length (`item.total_size`, см. `DownloadedState::known_total_bytes`).
fn paused_progress_text(current: u64, total: u64) -> String {
    if total > 0 {
        format!("{} / {}", format_bytes(current), format_bytes(total))
    } else {
        format_bytes(current)
    }
}

pub fn render_item(item: &BrowserItem, active: Option<&TaskInfo>, actions: ItemActions) -> Element<()> {
    // Иконка и имя — раздельные Text: у иконки свой шрифт (Icons), имя файла
    // рисуется дефолтным — единая строка ломала бы одно из двух.
    let icon_glyph = if item.is_folder { "\u{f07b}" } else { "\u{f016}" };
    // Имя — Length::Fill, а не Shrink по умолчанию: иначе при сужении окна
    // текст не переносится, а рисуется поверх границ карточки (Shrink меряет
    // себя без учёта родительских границ). Fill нужен на ОБОИХ уровнях — и
    // на самом Text, и на обёртывающем Row: Fill-текст внутри Shrink-строки
    // не получает реальной ширины от кнопки-родителя и схлопывается в
    // минимальную (отсюда вертикальный "по одной букве" — уже проверено).
    let title = row![
        text(icon_glyph).font_family("Icons"),
        text(item.name.clone()).width(Length::Fill),
    ].spacing(6.0).align_items(Alignment::Center).width(Length::Fill);

    // Открыть можно только по-настоящему скачанный файл — .part не читаем.
    let viewable = item.local_path.is_some() && !item.is_partial;

    // Одна и та же карточка (см. styles::file_button_style) для любой
    // строки — папка, готовый файл, недокачанный или ещё не скачанный:
    // `on_press_with` навешивается только там, где есть реальное действие,
    // остальные строки — та же кнопка, просто без обработчика (хост рисует
    // её в стиле `disabled`, см. styles.rs). Раньше некликабельные строки
    // были голым текстом без паддинга кнопки — отсюда и другой отступ от
    // края, и вообще другой виджет.
    let card = styles::apply_file(button(title)).width(Length::Fill);
    let main_button: Element<()> = if item.is_folder {
        match actions.browse {
            Some(method) => card.on_press_with(method, item.s3_key.clone()).into(),
            None => card.into(),
        }
    } else if viewable {
        match actions.view {
            Some(method) => card.on_press_with(method, item.local_path.clone().unwrap()).into(),
            None => card.into(),
        }
    } else {
        card.into()
    };

    let status: Element<()> = if let Some(task) = active {
        // Качается прямо сейчас — прогресс, кнопка "стоп" (тот же
        // on_download_pressed: повторное нажатие на активный s3_key отменяет,
        // см. handlers::download) и корзина (отменяет и удаляет .part, см.
        // handlers::download::on_delete_pressed/pending_delete_on_cancel).
        let label = text("\u{f254}").font_family("Icons").into();
        let amount = text(progress_text(task)).size(13.0).into();

        let mut buttons: Vec<Element<()>> = Vec::new();
        if let Some(method) = actions.download {
            // Пауза, не стоп: повторное нажатие на скачиваемый s3_key просто
            // отменяет текущий поток, но .part остаётся на диске и следующее
            // нажатие "скачать" продолжит с оборванного байта (см.
            // network::download.rs) — по факту это пауза, а не отмена насовсем.
            buttons.push(styles::icon_button("\u{f04c}", styles::COLOR_WARNING).on_press_with(method, item.s3_key.clone()).into());
        }
        if let (Some(method), Some(path)) = (actions.delete, &item.local_path) {
            buttons.push(styles::icon_button("\u{f1f8}", styles::COLOR_DANGER).on_press_with(method, path.clone()).into());
        }
        status_row(label, amount, buttons)
    } else if let Some(local_path) = &item.local_path {
        if item.is_partial {
            // Недокачан и сейчас не качается — докачка (тот же remote-ключ,
            // host сам подхватит .part с оборванного байта) и удаление.
            let label = row![
                text("\u{f071}").font_family("Icons").color(styles::COLOR_WARNING),
                text("Incomplete").size(13.0),
            ].spacing(6.0).align_items(Alignment::Center).into();
            let amount = text(paused_progress_text(item.size, item.total_size)).size(13.0).into();

            let mut buttons: Vec<Element<()>> = Vec::new();
            if !item.s3_key.is_empty() {
                if let Some(method) = actions.download {
                    // Play, не refresh: это продолжение (host сам подхватит
                    // .part), а не повторное скачивание с нуля — зрительно
                    // должно читаться как обратное действие "паузе" (f04c) выше.
                    buttons.push(styles::icon_button("\u{f04b}", styles::COLOR_PRIMARY).on_press_with(method, item.s3_key.clone()).into());
                }
            }
            if let Some(method) = actions.delete {
                buttons.push(styles::icon_button("\u{f1f8}", styles::COLOR_DANGER).on_press_with(method, local_path.clone()).into());
            }
            status_row(label, amount, buttons)
        } else {
            // Просмотр всегда идёт с локального пути, re-download — с
            // remote-ключа: это разные значения (см. BrowserItem::local_path),
            // кнопка закачки скрыта, если remote-ключ для файла неизвестен.
            let label = text("\u{f00c}").font_family("Icons").into();
            let amount = text(format_bytes(item.size)).size(13.0).into();

            let mut buttons: Vec<Element<()>> = Vec::new();
            if let Some(method) = actions.view {
                buttons.push(styles::icon_button("\u{f06e}", styles::COLOR_PRIMARY).on_press_with(method, local_path.clone()).into());
            }
            if !item.s3_key.is_empty() {
                if let Some(method) = actions.download {
                    buttons.push(styles::icon_button("\u{f021}", styles::COLOR_RELOAD).on_press_with(method, item.s3_key.clone()).into());
                }
            }
            if let Some(method) = actions.delete {
                buttons.push(styles::icon_button("\u{f1f8}", styles::COLOR_DANGER).on_press_with(method, local_path.clone()).into());
            }
            status_row(label, amount, buttons)
        }
    } else if !item.is_folder {
        let buttons = match actions.download {
            Some(method) => vec![styles::icon_button("\u{f019}", styles::COLOR_PRIMARY).on_press_with(method, item.s3_key.clone()).into()],
            None => Vec::new(),
        };
        status_row(text("").into(), text("").into(), buttons)
    } else {
        status_row(text("").into(), text("").into(), Vec::new())
    };

    row![main_button, status]
        .width(Length::Fill)
        .spacing(10.0)
        .align_items(Alignment::Center)
        .into()
}

pub fn render_list(items: &[BrowserItem], task_manager: &TaskManager, actions: ItemActions) -> Element<()> {
    column(items.iter().map(|item| {
        let active = task_manager.active_download(&item.s3_key);
        render_item(item, active, actions)
    }))
    .width(Length::Fill)
    .spacing(8.0)
    .into()
}

/// Список файлов или текст-заглушка, если он пуст — этот выбор одинаков на
/// всех трёх экранах (Browse/Search/Downloaded), различается только
/// сообщение-заглушка.
pub fn items_or_message(items: &[BrowserItem], task_manager: &TaskManager, actions: ItemActions, empty_message: &str) -> Element<()> {
    if items.is_empty() {
        column![text(empty_message).size(16.0)].into()
    } else {
        render_list(items, task_manager, actions)
    }
}

/// Обёртка экрана со списком файлов: одинаковые spacing/padding/width/height
/// и скролл на всех трёх экранах — отличаются только `header_rows`
/// (заголовок, плюс что у каждого экрана есть своего: кнопка "Up" в Browse,
/// строка поиска в Search, секция недокачанных в Downloaded) и тело.
///
/// Колонку заголовка собирает сама (а не принимает готовый `Element`) —
/// каждый вложенный `column![]` без явного `.width(Fill)` схлопывается по
/// ширине до содержимого, а это тот же самый класс бага что и Length::Shrink
/// по умолчанию у самих виджетов (кнопок/текста/иконок) — там он осознанный
/// и его трогать не надо, но конкретно на экранном контейнере он неуместен.
/// Раз этот вызов — единственное место, где вложенная колонка вообще
/// создаётся, промахнуться мимо Fill здесь больше негде.
pub fn list_screen(header_rows: Vec<Element<()>>, body: Element<()>) -> Element<()> {
    // Правый паддинг у тела, а не у всей колонки — резервирует место под
    // полосу прокрутки: у `scrollable` в этом фреймворке нет своего API для
    // отступа скроллбара (в отличие от iced, где это Properties::margin), он
    // рисуется поверх правого края content-зоны — без запаса задевает
    // последнюю кнопку в каждой строке.
    let padded_body = container(body)
        .width(Length::Fill)
        .padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 0.0 });
    column![
        column(header_rows).spacing(15.0).width(Length::Fill),
        scrollable(padded_body).width(Length::Fill).height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
