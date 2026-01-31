use iced_widget::{
    button, column, container, row, scrollable, text,
};
use iced_core::{Element, Length, Color, Alignment, Theme};
use iced_tiny_skia::Renderer;
use crate::app::Message;
use crate::common::BrowserItem;

pub fn view<'a>(path: &'a str, items: &'a [BrowserItem], status: &'a str, can_prev: bool, can_next: bool) -> Element<'a, Message, Theme, Renderer> {
    column![
        row![
            text(format!("Path: /{}", path)).font(crate::common::APP_FONT).size(16).width(Length::Fill),
            button(text("↑ Up").font(crate::common::APP_FONT)).on_press(Message::BrowseUp).padding(5),
        ].spacing(10).align_y(Alignment::Center),
        text(status).font(crate::common::APP_FONT).size(12).color(Color::from_rgb(0.6, 0.6, 0.6)),
        scrollable(column(items.iter().map(|item| {
            let icon = if item.is_folder { "📁" } else { "📄" };
            button(row![
                text(icon).font(crate::common::EMOJI_FONT).size(15),
                text(&item.name).font(crate::common::APP_FONT).size(15),
            ].spacing(10))
                .on_press(Message::BrowsePath(item.s3_key.clone()))
                .width(Length::Fill)
                .padding(5)
                .into()
        }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(5)).height(Length::Fill),
        row![
            button(text("Previous").font(crate::common::APP_FONT)).on_press_maybe(if can_prev { Some(Message::PrevPage) } else { None }).padding(8),
            button(text("Next").font(crate::common::APP_FONT)).on_press_maybe(if can_next { Some(Message::NextPage) } else { None }).padding(8),
        ].spacing(20)
    ].spacing(10).into()
}
