use veld_ui::{column, text, scrollable, Element};
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

pub fn view(state: &DownloadedState, files: &[BrowserItem], downloading_key: Option<&str>) -> Element<Message> {
    let mut display_items = files.to_vec();
    for item in &mut display_items {
        if downloading_key == Some(&item.s3_key) {
            item.is_downloading = true;
        }
    }

    let file_list = crate::common::render_list(&display_items);

    column![
        text("Local Files").size(20.0),
        text(format!("Search: {}", state.search_query)).size(14.0),
        scrollable(file_list)
            .width(veld_ui::Length::Fill)
            .height(veld_ui::Length::Fill)
    ]
    .spacing(15.0)
    .width(veld_ui::Length::Fill)
    .height(veld_ui::Length::Fill)
    .into()
}