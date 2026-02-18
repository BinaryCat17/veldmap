use veld_ui::{
    column, row, text, button, Element, Color
};
use crate::common::ViewMode;
use crate::styles::{COLOR_TEXT, COLOR_TEXT_DIM};
use crate::{LocalState, AppMessage as Message};

pub fn view(state: &LocalState) -> Element<Message> {
    let title_bar = column![
        text("VeldMap Tools").size(32.0).color(COLOR_TEXT),
        row![
            crate::styles::apply_nav(button(text("Search")))
                .on_press(Message::SwitchMode(ViewMode::Search)),
            crate::styles::apply_nav(button(text("Browse")))
                .on_press(Message::SwitchMode(ViewMode::Browse)),
            crate::styles::apply_nav(button(text("Downloaded")))
                .on_press(Message::SwitchMode(ViewMode::Downloaded)),
        ].spacing(15.0),
    ].spacing(20.0);

    let error_view: Element<Message> = if let Some(err) = &state.error_message {
        column![
            text(err).size(14.0).color(Color::from_rgb(1.0, 0.4, 0.4)),
            crate::styles::apply_primary(button(text("Clear"))).on_press(Message::ClearError)
        ].spacing(10.0).into()
    } else { column![].into() };

    let status_view = text(&state.status_message).size(14.0).color(COLOR_TEXT_DIM);

    // Умная логика прогресса
    let (active_progress, task_name) = if state.download_task.is_running() {
        (Some(state.download_task.progress()), "Downloading")
    } else {
        (None, "")
    };

    let background_task = if state.search_task.is_running() {
        Some("Searching...")
    } else if state.image_task.is_running() {
        Some("Loading image...")
    } else {
        None
    };

    let progress_view: Element<Message> = if let Some(progress) = active_progress {
        column![
            text(format!("{}: {:.1}%", task_name, progress * 100.0)).size(12.0),
            veld_ui::progress_bar(0.0..=1.0, progress).height(veld_ui::Length::Fixed(8.0)),
            button(text("Cancel")).on_press(Message::CancelDownload)
        ].spacing(5.0).into()
    } else if let Some(task_info) = background_task {
        row![
            text(task_info).size(12.0).color(COLOR_TEXT_DIM),
            // В будущем здесь может быть кружок-спиннер
        ].into()
    } else {
        column![].into()
    };

    let main_content: Element<Message> = match state.view_mode {
        ViewMode::Search => crate::search::view(
            &state.search_state, 
            &state.search_results, 
            &state.local_files, 
            state.downloading_key.as_deref()
        ).key(100),
        
        ViewMode::Browse => crate::browse::view(
            &state.current_browse_path, 
            &state.browse_items, 
            &state.status_message, 
            !state.token_stack.is_empty(), 
            state.next_token.is_some(),
            state.downloading_key.as_deref()
        ).key(200),
        
        ViewMode::Downloaded => crate::downloaded::view(
            &state.downloaded_state, 
            &state.local_files, 
            state.downloading_key.as_deref()
        ).key(300),
    };

    column![title_bar, status_view, progress_view, error_view, main_content]
        .spacing(20.0)
        .padding(20.0)
        .width(veld_ui::Length::Fill)
        .height(veld_ui::Length::Fill)
        .into()
}
