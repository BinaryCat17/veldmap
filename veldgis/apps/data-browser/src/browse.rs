use veld_ui::{column, row, text, button, scrollable, Element};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, has_prev: bool, has_next: bool) -> Element<Message> {
    let list = column(items.iter().map(|item| {
        let icon = if item.is_folder { "📁 " } else { "📄 " };
        let msg = if item.is_folder {
            Message::BrowsePath(item.s3_key.clone())
        } else {
            Message::DownloadFile(item.s3_key.clone())
        };
        button(text(format!("{}{}", icon, item.name)))
            .width(veld_ui::Length::Fill)
            .on_press(msg)
            .into()
    })).spacing(5.0);

    let mut pagination = row![].spacing(10.0);
    if has_prev {
        pagination = pagination.push(button(text(" ← Previous ")).on_press(Message::PrevPage));
    }
    if has_next {
        pagination = pagination.push(button(text(" Next → ")).on_press(Message::NextPage));
    }

    let header = row![
        button(text("⤴")).on_press(Message::BrowseUp),
        text(format!("Browsing /{}", path)).size(20.0),
    ].spacing(10.0).align_items(veld_ui::Alignment::Center);

    column![
        header,
        text(status).size(14.0),
        pagination,
        scrollable(list).height(veld_ui::Length::Fill)
    ].spacing(10.0).height(veld_ui::Length::Fill).into()
}
