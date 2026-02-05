use veld_ui::{column, text, button, Element};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, _loading: bool, _has_more: bool) -> Element<Message> {
    column![
        text(format!("Browsing /{}", path)).size(20.0),
        text(status).size(14.0),
        column(items.iter().map(|item| {
            button(text(&item.name)).on_press(Message::BrowsePath(item.s3_key.clone())).into()
        }))
    ].spacing(15.0).into()
}
