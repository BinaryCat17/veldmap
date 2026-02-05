use veld_ui::{column, text, button, Element};
use crate::{AppMessage as Message, common::BrowserItem};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub enum FileFilter {
    #[default]
    All,
    Images,
    Data,
}

#[derive(Default)]
pub struct DownloadedState {
    pub search_query: String,
    pub filter: FileFilter,
}

pub fn view(state: &DownloadedState, files: &[BrowserItem]) -> Element<Message> {
    column![
        text("Local Files").size(20.0),
        text(format!("Search: {}", state.search_query)).size(14.0),
        column(files.iter().map(|f| {
            button(text(&f.name)).on_press(Message::ViewFile(f.s3_key.clone())).into()
        }))
    ].spacing(15.0).into()
}