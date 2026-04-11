//! Handlers для data-browser

use std::sync::{Arc, Mutex};
use veldsdk::core::Command;
use veldsdk::core::task::TaskUpdate;
use veldmap_api::data_browser::HandleUiEventRequest;

pub mod search;
pub mod browse;
pub mod download;

use crate::state::State;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_screen: Option<String>,
}

pub fn handle_ui_event(
    state: Arc<Mutex<State>>,
    request: HandleUiEventRequest,
) -> Command<TaskUpdate<Vec<u8>>> {
    use veldmap_api::data_browser::ui_event::Payload;
    
    let event = request.event.unwrap_or_default();
    
    match event.payload {
        Some(Payload::ButtonPressed(btn)) => {
            handle_button(state, btn.id)
        }
        Some(Payload::TextInputChanged(input)) => {
            handle_input(state, input.widget_id, input.value)
        }
        _ => Command::none(),
    }
}

fn handle_button(
    state: Arc<Mutex<State>>,
    button_id: String,
) -> Command<TaskUpdate<Vec<u8>>> {
    match button_id.as_str() {
        "search" => {
            let query = state.lock().unwrap().search.query.clone();
            search::search(state, veldmap_api::data_browser::SearchRequest { query, filter: 0 })
        }
        "browse" => {
            let path = state.lock().unwrap().browse.current_path.clone();
            browse::browse(state, veldmap_api::data_browser::BrowseRequest { path, token: String::new() })
        }
        _ => Command::none(),
    }
}

fn handle_input(
    state: Arc<Mutex<State>>,
    widget_id: String,
    value: String,
) -> Command<TaskUpdate<Vec<u8>>> {
    match widget_id.as_str() {
        "search_query" => {
            state.lock().unwrap().search.query = value;
        }
        _ => {}
    }
    Command::none()
}
