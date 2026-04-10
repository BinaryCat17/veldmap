use veld_ui::{column, row, text, button, container, Element, Padding, Alignment, Length, Space};
use crate::styles;
use crate::AppMessage;
use crate::common::BrowserItem;

pub fn render_item(
    item: &BrowserItem,
    is_downloading: bool,  // Передаём извне из task_manager
    on_browse: impl Fn(String) -> AppMessage + Clone + 'static,
    on_view: impl Fn(String) -> AppMessage + Clone + 'static,
    on_download: impl Fn(String) -> AppMessage + Clone + 'static,
) -> Element<AppMessage> {
    let mut title_column = column![text(&item.name)];
    if let Some(desc) = &item.description {
        title_column = title_column.push(text(desc).size(12.0).color(styles::COLOR_TEXT_DIM));
    }
    title_column = title_column.spacing(2.0);

    let main_part: Element<AppMessage> = if item.is_folder {
        styles::apply_file(button(
            row![
                text("\u{f07b}").color(styles::COLOR_FOLDER),
                title_column
            ].spacing(10.0).align_items(Alignment::Center)
        ))
        .width(Length::Fill)
        .on_press(on_browse(item.s3_key.clone()))
        .into()
    } else {
        let content = row![
            text("\u{f15b}").color(styles::COLOR_FILE),
            title_column
        ].spacing(10.0).align_items(Alignment::Center);

        if item.exists_locally {
            styles::apply_file(button(content))
                .width(Length::Fill)
                .on_press(on_view(item.s3_key.clone()))
                .into()
        } else {
            container(content)
                .padding(Padding { left: 10.0, right: 10.0, top: 5.0, bottom: 5.0 })
                .width(Length::Fill)
                .into()
        }
    };

    // Статус/кнопки справа — упрощённая версия без прогресс-бара
    let status_element: Element<AppMessage> = if is_downloading {
        // Показываем иконку загрузки (без прогресса — он теперь в боковой панели)
        container(text("\u{f017}").color(styles::COLOR_WARNING))
            .width(Length::Fixed(80.0))
            .align_x(Alignment::Center)
            .into()
    } else if item.exists_locally {
        container(row![
            text("\u{f00c}").color(styles::COLOR_SUCCESS),
            styles::apply_icon(button(text("\u{f06e}")), styles::COLOR_TEXT)
                .on_press(on_view(item.s3_key.clone())),
            styles::apply_icon(button(text("\u{f021}")), styles::COLOR_RELOAD)
                .on_press(on_download(item.s3_key.clone()))
        ].spacing(10.0).align_items(Alignment::Center))
        .width(Length::Fixed(120.0))
        .align_x(Alignment::End)
        .into()
    } else if !item.is_folder {
        container(
            styles::apply_icon(button(text("\u{f019}")), styles::COLOR_SUCCESS)
                .on_press(on_download(item.s3_key.clone()))
        )
        .width(Length::Fixed(80.0))
        .align_x(Alignment::End)
        .into()
    } else {
        container(Space::with_width(80.0))
            .width(Length::Fixed(80.0))
            .into()
    };

    row![main_part, status_element]
        .width(Length::Fill)
        .spacing(10.0)
        .align_items(Alignment::Center)
        .into()
}

pub fn render_list(
    items: &[BrowserItem],
    task_manager: &crate::components::task_manager::TaskManager,
    path_prefix: &str,  // Уникальный префикс для ключей (например, путь + страница)
    on_browse: impl Fn(String) -> AppMessage + Clone + 'static,
    on_view: impl Fn(String) -> AppMessage + Clone + 'static,
    on_download: impl Fn(String) -> AppMessage + Clone + 'static,
) -> Element<AppMessage> {
    column(items.iter().enumerate().map(|(idx, item)| {
        let is_downloading = task_manager.is_downloading(&item.s3_key);
        // Уникальный ключ: хэш пути + индекс
        let unique_key = path_prefix.len() as u64 * 10000 + idx as u64;
        render_item(item, is_downloading, on_browse.clone(), on_view.clone(), on_download.clone())
            .key(unique_key)
    }))
    .width(Length::Fill)
    .spacing(8.0)
    .padding(Padding { right: 30.0, ..Default::default() })
    .into()
}