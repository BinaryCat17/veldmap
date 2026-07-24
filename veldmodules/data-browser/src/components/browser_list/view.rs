//! components/browser_list/view.rs — рендеринг списка файлов

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, scrollable, Element, Length, Alignment, Padding};
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

pub fn render_item(item: &BrowserItem, active: Option<&TaskInfo>, actions: ItemActions) -> Element<()> {
    // Иконка и имя — раздельные Text: у иконки свой шрифт (Icons), имя файла
    // рисуется дефолтным — единая строка ломала бы одно из двух.
    let icon_glyph = if item.is_folder { "\u{f07b}" } else { "\u{f016}" };
    let title: Element<()> = row![
        text(icon_glyph).font_family("Icons"),
        text(item.name.clone()),
    ].spacing(6.0).align_items(Alignment::Center).into();

    // Открыть можно только по-настоящему скачанный файл — .part не читаем.
    let viewable = item.local_path.is_some() && !item.is_partial;

    let main_button: Element<()> = if item.is_folder {
        match actions.browse {
            Some(method) => styles::apply_file(button(title))
                .width(Length::Fill)
                .on_press_with(method, item.s3_key.clone())
                .into(),
            None => title,
        }
    } else if viewable {
        match actions.view {
            Some(method) => styles::apply_file(button(title))
                .width(Length::Fill)
                .on_press_with(method, item.local_path.clone().unwrap())
                .into(),
            None => title,
        }
    } else {
        title
    };

    let status: Element<()> = if let Some(task) = active {
        // Качается прямо сейчас — прогресс, кнопка "стоп" (тот же
        // on_download_pressed: повторное нажатие на активный s3_key отменяет,
        // см. handlers::download) и корзина (отменяет и удаляет .part, см.
        // handlers::download::on_delete_pressed/pending_delete_on_cancel).
        let mut r = row![
            text("\u{f254}").font_family("Icons"),
            text(progress_text(task)).size(13.0),
        ].spacing(6.0).align_items(Alignment::Center);

        if let Some(method) = actions.download {
            // Пауза, не стоп: повторное нажатие на скачиваемый s3_key просто
            // отменяет текущий поток, но .part остаётся на диске и следующее
            // нажатие "скачать" продолжит с оборванного байта (см.
            // network::download.rs) — по факту это пауза, а не отмена насовсем.
            r = r.push(styles::apply_icon(button(text("\u{f04c}").font_family("Icons")), styles::COLOR_WARNING).on_press_with(method, item.s3_key.clone()));
        }
        if let (Some(method), Some(path)) = (actions.delete, &item.local_path) {
            r = r.push(styles::apply_icon(button(text("\u{f1f8}").font_family("Icons")), styles::COLOR_DANGER).on_press_with(method, path.clone()));
        }
        r.into()
    } else if let Some(local_path) = &item.local_path {
        if item.is_partial {
            // Недокачан и сейчас не качается — докачка (тот же remote-ключ,
            // host сам подхватит .part с оборванного байта) и удаление.
            let mut r = row![
                text("\u{f071}").font_family("Icons").color(styles::COLOR_WARNING),
                text("Incomplete").size(13.0),
                text(format_bytes(item.size)).size(13.0),
            ].spacing(6.0).align_items(Alignment::Center);

            if !item.s3_key.is_empty() {
                if let Some(method) = actions.download {
                    r = r.push(styles::apply_icon(button(text("\u{f021}").font_family("Icons")), styles::COLOR_RELOAD).on_press_with(method, item.s3_key.clone()));
                }
            }
            if let Some(method) = actions.delete {
                r = r.push(styles::apply_icon(button(text("\u{f1f8}").font_family("Icons")), styles::COLOR_DANGER).on_press_with(method, local_path.clone()));
            }
            r.into()
        } else {
            // Просмотр всегда идёт с локального пути, re-download — с
            // remote-ключа: это разные значения (см. BrowserItem::local_path),
            // кнопка закачки скрыта, если remote-ключ для файла неизвестен.
            let mut r = row![
                text("\u{f00c}").font_family("Icons"),
                text(format_bytes(item.size)).size(13.0),
            ].spacing(6.0).align_items(Alignment::Center);

            if let Some(method) = actions.view {
                r = r.push(styles::apply_icon(button(text("\u{f06e}").font_family("Icons")), styles::COLOR_PRIMARY).on_press_with(method, local_path.clone()));
            }
            if !item.s3_key.is_empty() {
                if let Some(method) = actions.download {
                    r = r.push(styles::apply_icon(button(text("\u{f021}").font_family("Icons")), styles::COLOR_RELOAD).on_press_with(method, item.s3_key.clone()));
                }
            }
            if let Some(method) = actions.delete {
                r = r.push(styles::apply_icon(button(text("\u{f1f8}").font_family("Icons")), styles::COLOR_DANGER).on_press_with(method, local_path.clone()));
            }
            r.into()
        }
    } else if !item.is_folder {
        match actions.download {
            Some(method) => styles::apply_icon(button(text("\u{f019}").font_family("Icons")), styles::COLOR_PRIMARY).on_press_with(method, item.s3_key.clone()).into(),
            None => text("").into(),
        }
    } else {
        text("").into()
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
    column![
        column(header_rows).spacing(15.0).width(Length::Fill),
        scrollable(body).width(Length::Fill).height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
