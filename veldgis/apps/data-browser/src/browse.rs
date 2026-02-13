use veld_ui::{column, row, text, button, scrollable, Element};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, has_prev: bool, has_next: bool, downloading_key: Option<&str>) -> Element<Message> {
    let list = column(items.iter().map(|item| {
        let is_downloading = downloading_key == Some(&item.s3_key);
        let icon = if item.is_folder { "📁 " } else { "📄 " };
        
        // Main Action
        let main_msg = if item.is_folder {
            Some(Message::BrowsePath(item.s3_key.clone()))
        } else if !item.exists_locally && !is_downloading {
            Some(Message::DownloadFile(item.s3_key.clone()))
        } else {
            None
        };

        let mut main_btn = button(
            text(format!("{}{}", icon, item.name))
                .horizontal_alignment(veld_ui::Alignment::Start)
        )
        .width(veld_ui::Length::Fill);
        
        if let Some(msg) = main_msg {
            main_btn = main_btn.on_press(msg);
        }

        // Status / Refresh Action
        let status_element: Element<Message> = if is_downloading {
            text(" ⏳").color(crate::common::COLOR_TEXT_DIM).into()
        } else if item.exists_locally {
            row![
                text(" ✅").color(veld_ui::Color::from_rgb(0.3, 0.8, 0.3)),
                button(text(" 🔄"))
                    .style("text")
                    .on_press(Message::DownloadFile(item.s3_key.clone()))
            ].width(veld_ui::Length::Shrink).spacing(5.0).align_items(veld_ui::Alignment::Center).into()
        } else {
            column![].width(veld_ui::Length::Shrink).into()
        };

        row![
            main_btn,
            status_element
        ].width(veld_ui::Length::Fill).spacing(10.0).align_items(veld_ui::Alignment::Center).into()
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
