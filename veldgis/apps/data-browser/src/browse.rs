use iced_widget::{
    button, column, row, scrollable, text,
};
use iced_core::{Element, Length, Alignment, Theme};
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
            let icon = if item.is_folder { "📁" } else { "📄" };
            button(iced_widget::container(row![
                text(icon).font(crate::common::EMOJI_FONT).size(15),
                text(&item.name).font(crate::common::APP_FONT).size(15).color(common::COLOR_TEXT),
            ].spacing(10)).padding(8))
                .on_press(Message::BrowsePath(item.s3_key.clone()))
                .style(common::ghost_button_style)
                .width(Length::Fill)
                .into()
        }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(2)).height(Length::Fill),
    ].spacing(15).into()
}
