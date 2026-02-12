use veld_ui::{column, row, text, button, scrollable, Element};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, _loading: bool, has_more: bool) -> Element<Message> {
    let mut list = column(items.iter().map(|item| {
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

    if has_more {
        list = list.push(button(text("Load More")).width(veld_ui::Length::Fill).on_press(Message::LoadMore));
    }

    let header = row![
        button(text("⤴")).on_press(Message::BrowseUp),
        text(format!("Browsing /{}", path)).size(20.0),
    ].spacing(10.0).align_items(veld_ui::Alignment::Center);

    column![
        header,
        text(status).size(14.0),
        scrollable(list).height(veld_ui::Length::Fill)
    ].height(veld_ui::Length::Fill).into()
}
