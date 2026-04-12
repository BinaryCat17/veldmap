//! View рендеринг для data-browser
//! 
//! Здесь функции для построения UI каждого экрана

pub mod browse;
pub mod search;
pub mod downloaded;
pub mod preview;

use veld_ui::proto::{SetViewRequest, set_view_request::Update, Layout};
use crate::state::State;
use veld_ui::{Element, column, row, text, button, Length, Padding};

pub fn build_root(state: &State) -> Element<()> {
    let main_content = match state.current_screen {
        crate::state::Screen::Browse => browse::view(state),
        crate::state::Screen::Search => search::view(state),
        crate::state::Screen::Downloaded => downloaded::view(state),
        crate::state::Screen::Preview => preview::view(state),
    };
    
    // Header navigation
    let header = row![
        button(text("Browse")).on_press_tag("data-browser/nav_browse"),
        button(text("Search")).on_press_tag("data-browser/nav_search"),
        button(text("Downloaded")).on_press_tag("data-browser/nav_downloaded")
    ].spacing(10.0).padding(Padding::new(10.0));
    
    let status_bar = row![
        text(&state.global.status_message).size(14.0),
        text(state.global.error_message.as_deref().unwrap_or("")).size(14.0)
    ].spacing(10.0).padding(Padding::new(5.0));

    column![
        header,
        main_content,
        status_bar
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
