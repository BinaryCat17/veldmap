use crate::state::{State, Screen};

pub fn on_nav_browse(state: &mut State, _value: String) -> anyhow::Result<()> {
    state.current_screen = Screen::Browse;
    Ok(())
}

pub fn on_nav_search(state: &mut State, _value: String) -> anyhow::Result<()> {
    state.current_screen = Screen::Search;
    Ok(())
}

pub fn on_nav_downloaded(state: &mut State, _value: String) -> anyhow::Result<()> {
    state.current_screen = Screen::Downloaded;
    Ok(())
}