//! View рендеринг для data-browser
//! 
//! Здесь функции для построения UI каждого экрана

pub mod browse;
pub mod search;
pub mod downloaded;
pub mod preview;

use crate::module::state::State;
use crate::module::styles;
use veld_ui_service_wrap::{column, row, container};
use crate::proto::ui_service::{Element, text, button, Length, Padding, Color};

pub fn build_root(state: &State) -> Element<()> {
    let main_content = match state.current_screen {
        crate::module::state::Screen::Browse => browse::view(state),
        crate::module::state::Screen::Search => search::view(state),
        crate::module::state::Screen::Downloaded => downloaded::view(state),
        crate::module::state::Screen::Preview => preview::view(state),
    };

    // Header navigation
    let header = row![
        styles::apply_nav(button(text("Browse"))).on_press("on_nav_browse"),
        styles::apply_nav(button(text("Search"))).on_press("on_nav_search"),
        styles::apply_nav(button(text("Downloaded"))).on_press("on_nav_downloaded")
    ].spacing(10.0).padding(Padding::new(10.0));

    let status_bar = row![
        text(&state.global.status_message).size(14.0),
        text(state.global.error_message.as_deref().unwrap_or("")).size(14.0)
    ].spacing(10.0).padding(Padding::new(5.0));

    // Тёмный фон окна: корневой Container (#1e1e24)
    container(
        column![
            header,
            main_content,
            status_bar
        ]
        .width(Length::Fill)
        .height(Length::Fill)
    )
    .background(Color::from_rgb(0.118, 0.118, 0.141))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
