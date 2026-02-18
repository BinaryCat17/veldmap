use veld_ui::{column, row, text, button, scrollable, text_input, Element};
use veldmap_gis_api::dataprovider::DataProduct;
use crate::{AppMessage as Message, common::BrowserItem};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub enum SearchFilterType {
    #[default]
    General,
    Collection,
    GridId,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub filter_type: SearchFilterType,
}

pub fn view(state: &SearchState, results: &[DataProduct], local_files: &[BrowserItem], downloading_key: Option<&str>) -> Element<Message> {
    let display_items: Vec<BrowserItem> = results.iter().map(|p| {
        let exists_locally = local_files.iter().any(|f| f.name == p.name);
        let is_downloading = downloading_key == Some(&p.path);
        
        BrowserItem {
            s3_key: p.path.clone(),
            name: p.name.clone(),
            description: Some(format!("{} | {}", p.timestamp, p.grid_id)),
            is_folder: false,
            exists_locally,
            is_downloading,
        }
    }).collect();

    let results_list = crate::common::render_list(&display_items);

    column![
        text("Search Copernicus Data Space").size(20.0),
        row![
            text_input("Search query...", &state.query)
                .on_input(Message::SearchInputChanged)
                .on_submit(Message::SearchPressed)
                .width(veld_ui::Length::Fill)
                .padding(10.0),
            crate::styles::apply_primary(button(text("Search"))).on_press(Message::SearchPressed)
        ].spacing(10.0).width(veld_ui::Length::Fill).align_items(veld_ui::Alignment::Center),
        scrollable(results_list)
            .width(veld_ui::Length::Fill)
            .height(veld_ui::Length::Fill)
    ]
    .spacing(15.0)
    .width(veld_ui::Length::Fill)
    .height(veld_ui::Length::Fill)
    .into()
}
