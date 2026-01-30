use iced::widget::{button, column, Image};
use iced::widget::image::Handle;
use iced::{Element, Length};
use crate::app::Message;

pub fn view<'a>(handle: &'a Handle) -> Element<'a, Message> {
    column![
        button("Close Preview").on_press(Message::ClosePreview).padding(5),
        Image::new(handle.clone()).width(Length::Fill).height(Length::Fill)
    ].spacing(10).into()
}
