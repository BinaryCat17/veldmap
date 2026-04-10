//! app/view.rs

use veld_ui::{column, row, text, button, container, Element, Color, Length, Alignment};
use crate::{
    AppState, AppMessage,
    common::ViewMode,
    styles::{COLOR_TEXT, COLOR_TEXT_DIM},
    widgets::task_panel,
};

pub fn view(state: &AppState) -> Element<AppMessage> {
    let title_bar = row![
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
    ]
    .spacing(30.0)
    .align_items(Alignment::Center);

    let error_view: Element<AppMessage> = if let Some(err) = &state.global.error_message {
        column![
            text(err).size(14.0).color(Color::from_rgb(1.0, 0.4, 0.4)),
            crate::styles::apply_primary(button(text("Clear"))).on_press(AppMessage::ClearError)
        ].spacing(10.0).into()
    } else {
        column![].into()
    };

    let status_view = text(&state.global.status_message).size(14.0).color(COLOR_TEXT_DIM);

    // Панель задач справа
    let task_sidebar: Element<AppMessage> = container(task_panel(&state.global.task_manager))
        .height(Length::Fill)
        .into();

    let main_content = match &state.screen {
        crate::app::state::Screen::Search(s) => crate::screens::search::view(s, &state.global).key(100),
        crate::app::state::Screen::Browse(s)  => crate::screens::browse::view(s, &state.global).key(200),
        crate::app::state::Screen::Downloaded(s) => crate::screens::downloaded::view(s, &state.global).key(300),
        crate::app::state::Screen::Preview(s) => crate::screens::preview::view(s, &state.global).key(400),
    };

    // Главный layout: контент слева + панель задач справа
    let content_row = row![
        column![status_view, error_view, main_content]
            .spacing(10.0)
            .width(Length::Fill)
            .height(Length::Fill),
        task_sidebar,
    ]
    .spacing(15.0)
    .width(Length::Fill)
    .height(Length::Fill);

    column![title_bar, content_row]
        .spacing(15.0)
        .padding(20.0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
