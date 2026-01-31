use iced_widget::{button, container, column, text};
use iced_core::{Element, Length, Theme, Color};
use iced_tiny_skia::Renderer;
use crate::app::Message;
use veldmap_rust_rpc::common::DataProduct;

pub struct SearchState {
    pub query: String,
    pub filter_type: SearchFilterType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterType {
    General,
    Collection,
    GridId,
}

impl Default for SearchState {
    fn default() -> Self {
        Self { query: String::new(), filter_type: SearchFilterType::General }
    }
}

pub fn view<'a>(_state: &'a SearchState, _results: &'a [DataProduct]) -> Element<'a, Message, Theme, Renderer> {
    column![
        container(text("VELD_MAP DATA BROWSER").size(40))
            .width(Length::Fill)
            .height(Length::Fixed(100.0))
            .center_x(Length::Fill)
            .style(|_| container::Style::default().background(Color::from_rgb(0.5, 0.0, 0.0))),
        
        container(
            button(container(text("CLICK TO SEARCH").size(20)).padding(20))
                .on_press(Message::SearchPressed)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style::default().background(Color::from_rgb(0.1, 0.1, 0.2)))
    ]
    .spacing(20)
    .into()
}
