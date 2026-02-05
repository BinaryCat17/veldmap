use iced_widget::{
    button, column, row, scrollable, text, text_input, pick_list, Space, container
};
use iced_core::{Element, Length, Color, Alignment, Theme};
use veldsdk::prelude::GpuRenderer as Renderer;
use crate::Message;
use crate::common;
use veldmap_gis_api::dataprovider::DataProduct;

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

pub fn view<'a>(state: &'a SearchState, results: &'a [DataProduct]) -> Element<'a, Message, Theme, Renderer> {
    let controls = row![
        pick_list(&[SearchFilterType::General, SearchFilterType::GridId, SearchFilterType::Collection][..], Some(state.filter_type), Message::SearchFilterTypeChanged)
            .font(crate::common::APP_FONT)
            .padding(10),
        text_input("Enter search query...", &state.query)
            .on_input(Message::SearchInputChanged)
            .on_submit(Message::SearchPressed)
            .font(crate::common::APP_FONT)
            .padding(10),
        button(text("Find").font(crate::common::APP_FONT))
            .on_press(Message::SearchPressed)
            .style(common::primary_button_style)
            .padding(10),
    ].spacing(10).align_y(Alignment::Center);

    let list = column(results.iter().map(|item| {
        button(
            container(column![
                text(&item.name).font(crate::common::APP_FONT).size(15).color(common::COLOR_TEXT),
                row![
                    text(item.grid_id.as_str()).font(crate::common::APP_FONT).size(11).color(Color::from_rgb(0.4, 0.7, 0.4)),
                    Space::new().width(20),
                    text(&item.path).font(crate::common::APP_FONT).size(11).color(common::COLOR_TEXT_DIM),
                ]
            ]).padding(10)
        )
        .on_press(Message::ProductSelected(item.clone()))
        .width(Length::Fill)
        .style(common::ghost_button_style)
        .into()
    }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(5);

    column![
        controls,
        scrollable(list).height(Length::Fill)
    ].spacing(15).into()
}