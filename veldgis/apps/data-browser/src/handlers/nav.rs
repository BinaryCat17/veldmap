use std::sync::{Arc, Mutex};
use veldmap_api::ui::UiEventResponse;
use veldsdk::core::Command;

use crate::state::{State, Screen};

pub fn on_nav_browse(state: Arc<Mutex<State>>, _request: UiEventResponse) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Browse;
    crate::view::render(&guard);
    Command::none()
}

pub fn on_nav_search(state: Arc<Mutex<State>>, _request: UiEventResponse) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Search;
    crate::view::render(&guard);
    Command::none()
}

pub fn on_nav_downloaded(state: Arc<Mutex<State>>, _request: UiEventResponse) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Downloaded;
    crate::view::render(&guard);
    Command::none()
}