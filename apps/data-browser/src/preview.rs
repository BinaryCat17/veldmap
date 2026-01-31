use iced_widget::{button, column, image as iced_image};
use iced_core::image::Handle;
use iced_core::{Element, Length, Theme};
use iced_tiny_skia::Renderer;
use crate::app::Message;

pub fn view<'a>(handle: &'a Handle) -> Element<'a, Message, Theme, Renderer> {
    column![
        button("Close Preview").on_press(Message::ClosePreview).padding(5),
        iced_image(handle.clone()).width(Length::Fill).height(Length::Fill),
    ].spacing(10).into()
}