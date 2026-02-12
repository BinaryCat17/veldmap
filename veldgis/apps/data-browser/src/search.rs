use veld_ui::{column, row, text, button, scrollable, Element};
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
    let results_list = column(results.iter().map(|p| {
        button(text(&p.name)).width(veld_ui::Length::Fill).on_press(Message::ProductSelected(p.clone())).into()
    })).spacing(5.0);

    column![
        text("Search Copernicus Data Space").size(20.0),
        row![
            button(text("Search")).on_press(Message::SearchPressed)
        ].spacing(10.0),
        scrollable(results_list).height(veld_ui::Length::Fill)
    ].spacing(15.0).height(veld_ui::Length::Fill).into()
}