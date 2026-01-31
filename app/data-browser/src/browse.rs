use iced::widget::{
    button, column, row, scrollable, text, horizontal_space, container
};
use iced::{Element, Length, Color, Alignment};
use crate::common::{icon_text, BrowserItem, is_previewable};
use crate::app::Message;

pub fn view<'a>(path: &'a str, items: &'a [BrowserItem], status: &'a str, can_prev: bool, can_next: bool) -> Element<'a, Message> {
    let nav_buttons = row![
        button("Root").on_press(Message::BrowsePath(String::new())).padding(8),
        button("UP").on_press(Message::BrowseUp).padding(8),
        horizontal_space().width(20),
        button("PREV").on_press_maybe(if can_prev { Some(Message::PrevPage) } else { None }).padding(8),
        button("NEXT").on_press_maybe(if can_next { Some(Message::NextPage) } else { None }).padding(8),
    ].spacing(10).align_y(Alignment::Center);

    let path_display = column![
        text(status).size(12).color(Color::from_rgb(0.6, 0.6, 0.6)),
        text(format!("Path: /{}", path)).size(14).width(Length::Fill),
    ].spacing(5);

    let list = column(items.iter().map(|item| {
        let label_icon = if item.is_folder { "📁" } else if item.exists_locally { "✅ 📄" } else { "📄" };
        let label_color = if item.exists_locally { Color::from_rgb(0.3, 0.8, 0.3) } else { Color::WHITE };
        let previewable = !item.is_folder && is_previewable(&item.name);

        let mut row_content = row![
            icon_text(label_icon, &item.name, label_color),
            horizontal_space().width(Length::Fill),
        ].spacing(10).align_y(Alignment::Center);

        if !item.is_folder {
            if previewable {
                row_content = row_content.push(
                    button("View").on_press(Message::ViewFile(item.s3_key.clone())).padding(5)
                );
            }
            if !item.exists_locally {
                row_content = row_content.push(
                    button("Download").on_press(Message::DownloadFile(item.s3_key.clone())).padding(5)
                );
            } else {
                row_content = row_content.push(text("Ready").size(12).color(Color::from_rgb(0.5, 0.5, 0.5)));
            }
        } else {
            // Folders are clickable as a whole row
            return button(row_content)
                .on_press(Message::BrowsePath(item.s3_key.clone()))
                .width(Length::Fill)
                .style(button::secondary)
                .padding(8)
                .into();
        }

        container(row_content).padding(5).into()
    }).collect::<Vec<Element<Message>>>()).spacing(5);

    column![
        nav_buttons,
        path_display,
        container(scrollable(list).height(Length::Fill)).padding(5)
    ].spacing(15).into()
}