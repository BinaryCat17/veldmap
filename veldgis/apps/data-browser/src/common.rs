use veld_ui::{column, row, text, button, container, Element, Padding, Alignment, Length, Space};
use crate::{AppMessage as Message, styles};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    Search,
    Browse,
    Downloaded,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub description: Option<String>,
    pub is_folder: bool,
    pub exists_locally: bool,
    pub is_downloading: bool,
}

pub fn render_item(item: &BrowserItem) -> Element<Message> {
    let mut title_column = column![text(&item.name)];
    if let Some(desc) = &item.description {
        title_column = title_column.push(text(desc).size(12.0).color(styles::COLOR_TEXT_DIM));
    }
    title_column = title_column.spacing(2.0);

    // Основная часть (Папка - кнопка перехода, Файл - текст или кнопка просмотра)
    let main_part: Element<Message> = if item.is_folder {
        styles::apply_file(button(
            row![
                text("\u{f07b}").color(styles::COLOR_FOLDER),
                title_column
            ].spacing(10.0).align_items(Alignment::Center)
        ))
        .width(Length::Fill)
        .on_press(Message::BrowsePath(item.s3_key.clone()))
        .into()
    } else {
        let content = row![
            text("\u{f15b}").color(styles::COLOR_FILE),
            title_column
        ].spacing(10.0).align_items(Alignment::Center);

        if item.exists_locally {
            styles::apply_file(button(content))
                .width(Length::Fill)
                .on_press(Message::ViewFile(item.s3_key.clone()))
                .into()
        } else {
            container(content)
                .padding(Padding { left: 10.0, right: 10.0, top: 5.0, bottom: 5.0 })
                .width(Length::Fill)
                .into()
        }
    };

    // Кнопки действий (Скачать/Обновить/Индикатор загрузки)
    let status_element: Element<Message> = if item.is_downloading {
        container(text("\u{f017}").color(styles::COLOR_WARNING))
            .width(Length::Fixed(80.0))
            .align_x(Alignment::Center)
            .into()
    } else if item.exists_locally {
        container(row![
            text("\u{f00c}").color(styles::COLOR_SUCCESS),
            styles::apply_icon(button(text("\u{f021}")), styles::COLOR_RELOAD)
                .on_press(Message::DownloadFile(item.s3_key.clone()))
        ].spacing(10.0).align_items(Alignment::Center))
        .width(Length::Fixed(80.0))
        .align_x(Alignment::End)
        .into()
    } else if !item.is_folder {
        container(
            styles::apply_icon(button(text("\u{f019}")), styles::COLOR_SUCCESS)
                .on_press(Message::DownloadFile(item.s3_key.clone()))
        )
        .width(Length::Fixed(80.0))
        .align_x(Alignment::End)
        .into()
    } else {
        container(Space::with_width(80.0))
            .width(Length::Fixed(80.0))
            .into()
    };

    row![
        main_part,
        status_element
    ]
    .width(Length::Fill)
    .spacing(10.0)
    .align_items(Alignment::Center)
    .into()
}

pub fn render_list(items: &[BrowserItem]) -> Element<Message> {
    column(items.iter().map(render_item))
        .width(Length::Fill)
        .spacing(8.0)
        .padding(Padding { right: 30.0, ..Default::default() })
        .into()
}
