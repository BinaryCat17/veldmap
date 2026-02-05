use veld_ui::{column, row, text, button, Element};
use veldmap_gis_api::dataprovider::DataProduct;
use crate::{AppMessage as Message};

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

pub fn view(_state: &SearchState, results: &[DataProduct]) -> Element<Message> {
    column![
        text("Search Copernicus Data Space").size(20.0),
        row![
            button(text("Search")).on_press(Message::SearchPressed)
        ].spacing(10.0),
        column(results.iter().map(|p| {
            button(text(&p.name)).on_press(Message::ProductSelected(p.clone())).into()
        }))
    ].spacing(15.0).into()
}