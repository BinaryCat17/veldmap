//! View рендеринг для data-browser
//! 
//! Здесь функции для построения UI каждого экрана

pub mod browse;
pub mod search;

use veld_ui::proto::{SetViewRequest, set_view_request::Update, Layout};
use crate::state::State;
use veld_ui::{Element, column, row, text, button, Length, Padding};

pub fn render(state: &State) {
    let root_element = build_root(state);
    
    // В FaF мы не делаем diffing на клиенте (data-browser), а шлем FullLayout.
    // ui-service сам сделает diffing, если нужно.
    // Устанавливаем plugin_id в "data-browser"
    
    let mut widget = root_element.widget;
    let mut idx = 0;
    let hash = veld_ui::diffing::assign_ids_and_hash(&mut widget, &mut idx);
    
    let req = SetViewRequest {
        plugin_id: "data-browser".to_string(),
        update: Some(Update::FullLayout(Layout {
            root: Some(widget),
            width: 1024,
            height: 768,
            hash,
        })),
    };
    
    veldsdk::publish!("ui-service/set_view", req);
}

pub fn build_root(state: &State) -> Element<()> {
    let main_content = match state.current_screen {
        crate::state::Screen::Browse => browse::view(state),
        crate::state::Screen::Search => search::view(state),
        _ => column![text("Not implemented yet").size(24.0)].into(),
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
