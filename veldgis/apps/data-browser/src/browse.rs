use iced_widget::{
    button, column, row, scrollable, text, container, Space
};
use iced_core::{Element, Length, Alignment, Theme, Color};
use iced_tiny_skia::Renderer;
use crate::Message;
use crate::common;
use crate::common::BrowserItem;

pub fn view<'a>(path: &'a str, items: &'a [BrowserItem], status: &'a str, _can_prev: bool, _can_next: bool) -> Element<'a, Message, Theme, Renderer> {
    column![
        row![
            text(format!("Path: /{}", path)).font(crate::common::APP_FONT).size(16).width(Length::Fill).color(common::COLOR_TEXT),
            button(text("↑ Up").font(crate::common::APP_FONT))
                .on_press(Message::BrowseUp)
                .style(common::ghost_button_style)
                .padding(5),
        ].spacing(10).align_y(Alignment::Center),
        text(status).font(crate::common::APP_FONT).size(12).color(common::COLOR_TEXT_DIM),
        scrollable(column(items.iter().map(|item| {
            if item.is_folder {
                button(iced_widget::container(row![
                    text("📁").font(crate::common::EMOJI_FONT).size(15),
                    text(&item.name).font(crate::common::APP_FONT).size(15).color(common::COLOR_TEXT),
                ].spacing(10)).padding(8))
                    .on_press(Message::BrowsePath(item.s3_key.clone()))
                    .style(common::ghost_button_style)
                    .width(Length::Fill)
                    .into()
            } else {
                let label_color = if item.exists_locally { Color::from_rgb(0.3, 0.8, 0.3) } else { common::COLOR_TEXT };
                
                let download_control: Element<Message, Theme, Renderer> = if item.exists_locally {
                    text("✅").font(common::EMOJI_FONT).size(18).into()
                } else {
                    button(text("Download").font(common::APP_FONT).size(12))
                        .on_press(Message::DownloadFile(item.s3_key.clone()))
                        .padding(5)
                        .style(common::primary_button_style)
                        .into()
                };

                let content = row![
                    text("📄").font(crate::common::EMOJI_FONT).size(15),
                    text(&item.name).font(crate::common::APP_FONT).size(15).color(label_color),
                    Space::new().width(Length::Fill),
                    download_control,
                ].spacing(10).align_y(Alignment::Center);

                if item.exists_locally && common::is_previewable(&item.name) {
                    button(container(content).padding(8))
                        .on_press(Message::ViewFile(format!("data/dem/source/{}", item.name)))
                        .style(common::ghost_button_style)
                        .width(Length::Fill)
                        .into()
                } else {
                    container(content)
                        .padding(8)
                        .width(Length::Fill)
                        .into()
                }
            }
        }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(2)).height(Length::Fill),
    ].spacing(15).into()
}