use iced::widget::{
    button, column, row, scrollable, text, text_input, horizontal_space, pick_list
};
use iced::{Element, Length, Color, Alignment};
use crate::gui::Message;
use veldmap_core::DataProduct;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterType {
    General,
    GridId,
    Collection,
}

impl std::fmt::Display for SearchFilterType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            SearchFilterType::General => "Name / Text",
            SearchFilterType::GridId => "Grid ID (N55_E037)",
            SearchFilterType::Collection => "Collection Name",
        })
    }
}

pub struct SearchState {
    pub query: String,
    pub filter_type: SearchFilterType,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            filter_type: SearchFilterType::General,
        }
    }
}

pub fn view<'a>(state: &'a SearchState, results: &'a [DataProduct]) -> Element<'a, Message> {
    let controls = row![
        pick_list(&[SearchFilterType::General, SearchFilterType::GridId, SearchFilterType::Collection][..], Some(state.filter_type), Message::SearchFilterTypeChanged)
            .padding(10),
        text_input("Enter search query...", &state.query)
            .on_input(Message::SearchInputChanged)
            .on_submit(Message::SearchPressed)
            .padding(10),
        button("Find").on_press(Message::SearchPressed).padding(10),
    ].spacing(10).align_y(Alignment::Center);

    let list = column(results.iter().map(|item| {
        button(
            column![
                text(&item.name).size(15),
                row![
                    text(item.grid_id.as_deref().unwrap_or("")).size(10).color(Color::from_rgb(0.4, 0.7, 0.4)),
                    horizontal_space().width(20),
                    text(&item.path).size(10).color(Color::from_rgb(0.6, 0.6, 0.6)),
                ]
            ]
        )
        .on_press(Message::ProductSelected(item.clone()))
        .width(Length::Fill)
        .padding(8).into()
    }).collect::<Vec<Element<Message>>>()).spacing(5);

    column![
        controls,
        scrollable(list).height(Length::Fill)
    ].spacing(15).into()
}
