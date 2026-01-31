use iced::widget::{
    button, column, row, scrollable, text, text_input, horizontal_space, pick_list, container
};
use iced::{Element, Length, Color, Alignment};
use crate::common::{icon_text, BrowserItem, is_previewable};
use crate::app::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFilter {
    All,
    Tiff,
    Images,
}

impl std::fmt::Display for FileFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            FileFilter::All => "All Files",
            FileFilter::Tiff => "TIFF only",
            FileFilter::Images => "Images (JPG/PNG)",
        })
    }
}

pub struct DownloadedState {
    pub search_query: String,
    pub filter: FileFilter,
}

impl Default for DownloadedState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            filter: FileFilter::All,
        }
    }
}

pub fn view<'a>(state: &'a DownloadedState, items: &'a [BrowserItem]) -> Element<'a, Message> {
    let query = state.search_query.to_lowercase();
    
    let filtered_items = items.iter().filter(|item| {
        let name_match = item.name.to_lowercase().contains(&query);
        let ext_match = match state.filter {
            FileFilter::All => true,
            FileFilter::Tiff => item.name.to_lowercase().ends_with(".tif") || item.name.to_lowercase().ends_with(".tiff"),
            FileFilter::Images => item.name.to_lowercase().ends_with(".jpg") || item.name.to_lowercase().ends_with(".png") || item.name.to_lowercase().ends_with(".jpeg"),
        };
        name_match && ext_match
    });

    let controls = row![
        text_input("Search local files...", &state.search_query)
            .on_input(Message::LocalSearchChanged)
            .padding(10),
        pick_list(&[FileFilter::All, FileFilter::Tiff, FileFilter::Images][..], Some(state.filter), Message::LocalFilterChanged)
            .padding(10),
        button("Refresh").on_press(Message::ScanLocalFiles).padding(10),
    ].spacing(10).align_y(Alignment::Center);

    let list = column(filtered_items.map(|item| {
        let previewable = is_previewable(&item.name);
        
        let view_btn: Element<Message> = if previewable {
            button("View").on_press(Message::ViewFile(item.s3_key.clone())).padding(5).into()
        } else {
            horizontal_space().width(0).into()
        };

        let delete_btn = button(text("Delete").color(Color::from_rgb(0.8, 0.2, 0.2)))
            .on_press(Message::DeleteLocalFile(item.s3_key.clone()))
            .padding(5);

        // UI Fix: Buttons are now fixed width and closer to text
        row![
            icon_text("📄", &item.name, Color::WHITE),
            horizontal_space().width(20),
            view_btn,
            delete_btn,
        ].spacing(10).align_y(Alignment::Center).into()
    }).collect::<Vec<Element<Message>>>()).spacing(8);

    column![
        text("Local Cache").size(20),
        controls,
        container(scrollable(list).height(Length::Fill)).padding(10)
    ].spacing(15).into()
}