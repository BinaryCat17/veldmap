use iced_widget::{
    button, column, container, row, scrollable, text, pick_list, text_input,
};
use iced_core::{Element, Length, Color, Alignment, Theme};
use iced_tiny_skia::Renderer;
use crate::app::Message;
use crate::common::BrowserItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileFilter {
    #[default]
    All,
    Tiff,
    Images,
}

impl std::fmt::Display for FileFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Default)]
pub struct DownloadedState {
    pub search_query: String,
    pub filter: FileFilter,
}

pub fn view<'a>(state: &'a DownloadedState, items: &'a [BrowserItem]) -> Element<'a, Message, Theme, Renderer> {
    column![
        row![
            text_input("Search local files...", &state.search_query).on_input(Message::LocalSearchChanged).font(crate::common::APP_FONT).padding(10),
            pick_list(&[FileFilter::All, FileFilter::Tiff, FileFilter::Images][..], Some(state.filter), Message::LocalFilterChanged).font(crate::common::APP_FONT).padding(10),
        ].spacing(10),
        scrollable(column(items.iter().filter(|i| i.name.contains(&state.search_query)).map(|item| {
            row![
                text(&item.name).font(crate::common::APP_FONT).size(15).width(Length::Fill),
                button(text("View").font(crate::common::APP_FONT)).on_press(Message::ViewFile(item.s3_key.clone())).padding(5),
                button(text("Delete").font(crate::common::APP_FONT)).on_press(Message::DeleteLocalFile(item.s3_key.clone())).padding(5),
            ].spacing(10).align_y(Alignment::Center).into()
        }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(8)).height(Length::Fill)
    ].spacing(15).into()
}
