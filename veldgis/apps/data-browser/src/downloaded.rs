use veld_ui::{column, text, button, scrollable, Element};
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
    let file_list = column(files.iter().map(|f| {
        button(text(&f.name)).width(veld_ui::Length::Fill).on_press(Message::ViewFile(f.s3_key.clone())).into()
    })).spacing(5.0);

    column![
        text("Local Files").size(20.0),
        text(format!("Search: {}", state.search_query)).size(14.0),
        scrollable(file_list).height(veld_ui::Length::Fill)
    ].spacing(15.0).height(veld_ui::Length::Fill).into()
}