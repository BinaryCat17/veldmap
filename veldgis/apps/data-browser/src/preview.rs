use iced_widget::{button, column, text, image as iced_image};
use iced_core::image::Handle;
use iced_core::{Element, Length, Theme};
use iced_tiny_skia::Renderer;
use crate::Message;

pub fn view<'a>(handle: &'a Handle) -> Element<'a, Message, Theme, Renderer> {
    column![
        button(text("Close Preview").font(crate::common::APP_FONT)).on_press(Message::ClosePreview).padding(5),
        iced_image(handle.clone()).width(Length::Fill).height(Length::Fill),
    ].spacing(10).into()
}