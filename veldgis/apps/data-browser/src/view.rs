//! view.rs — чистый декларативный рендер новой архитектуры
//! Никакого God Object, только match по Screen + делегирование в подмодули

use veld_ui::{
    column, row, text, button, Element, Color, Length,
};
use crate::{
    AppState, AppMessage,
    state::Screen,
    common::ViewMode,
    styles::{COLOR_TEXT, COLOR_TEXT_DIM},
};

pub fn view(state: &AppState) -> Element<AppMessage> {
    // ==================== Title Bar ====================
    let title_bar = column![
        text("VeldMap Tools").size(32.0).color(COLOR_TEXT),
        row![
            crate::styles::apply_nav(button(text("Search")))
                .on_press(AppMessage::SwitchMode(ViewMode::Search)),
            crate::styles::apply_nav(button(text("Browse")))
                .on_press(AppMessage::SwitchMode(ViewMode::Browse)),
            crate::styles::apply_nav(button(text("Downloaded")))
                .on_press(AppMessage::SwitchMode(ViewMode::Downloaded)),
            crate::styles::apply_nav(button(text("View")))
                .on_press(AppMessage::SwitchMode(ViewMode::View)),
        ].spacing(15.0),
    ].spacing(20.0);

    // ==================== Error ====================
    let error_view: Element<AppMessage> = if let Some(err) = &state.global.error_message {
        column![
            text(err).size(14.0).color(Color::from_rgb(1.0, 0.4, 0.4)),
            crate::styles::apply_primary(button(text("Clear"))).on_press(AppMessage::ClearError)
        ].spacing(10.0).into()
    } else {
        column![].into()
    };

    // ==================== Status & Progress ====================
    let status_view = text(&state.global.status_message).size(14.0).color(COLOR_TEXT_DIM);

    let (active_progress, task_name) = if state.global.download_task.is_running() {
        (Some(state.global.download_task.progress()), "Downloading")
    } else {
        (None, "")
    };

    let background_task = if state.global.search_task.is_running() {
        Some("Searching...")
    } else if state.global.image_task.is_running() {
        Some("Loading image...")
    } else {
        None
    };

    let progress_view: Element<AppMessage> = if let Some(progress) = active_progress {
        column![
            text(format!("{}: {:.1}%", task_name, progress * 100.0)).size(12.0),
            veld_ui::progress_bar(0.0..=1.0, progress).height(Length::Fixed(8.0)),
            button(text("Cancel")).on_press(AppMessage::CancelDownload)
        ].spacing(5.0).into()
    } else if let Some(task_info) = background_task {
        row![text(task_info).size(12.0).color(COLOR_TEXT_DIM)].into()
    } else {
        column![].into()
    };

    // ==================== Main Content ====================
    let main_content: Element<AppMessage> = match &state.screen {
        Screen::Search(s) => crate::search::view(s, &state.global),
        Screen::Browse(s) => crate::browse::view(s, &state.global),
        Screen::Downloaded(s) => crate::downloaded::view(s, &state.global),
        Screen::Preview(s) => crate::preview::view(s, &state.global),
    };

    // ==================== Финальная сборка ====================
    column![
        title_bar,
        status_view,
        progress_view,
        error_view,
        main_content
    ]
    .spacing(20.0)
    .padding(20.0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
