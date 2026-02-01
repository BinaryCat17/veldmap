use iced_widget::{
    button, column, container, row, text, progress_bar, scrollable, Space
};
use iced_core::{Alignment, Element, Length, Color, Theme};
use iced_tiny_skia::Renderer;
use crate::search;
use crate::downloaded;
use crate::preview;
use crate::browse;
use crate::common::{self, is_previewable, icon_text, ViewMode};
use crate::{LocalState, Message};

pub fn view(state: &LocalState) -> Element<'_, Message, Theme, Renderer> {
    let title_bar = column![
        text("VeldMap Tools").font(crate::common::APP_FONT).size(32).color(common::COLOR_TEXT),
        row![
            button(text("Search").font(crate::common::APP_FONT))
                .on_press(Message::SwitchMode(ViewMode::Search))
                .style(if state.view_mode == ViewMode::Search { common::primary_button_style } else { common::ghost_button_style })
                .padding(12),
            button(text("Browse").font(crate::common::APP_FONT))
                .on_press(Message::SwitchMode(ViewMode::Browse))
                .style(if state.view_mode == ViewMode::Browse { common::primary_button_style } else { common::ghost_button_style })
                .padding(12),
            button(text("Downloaded").font(crate::common::APP_FONT))
                .on_press(Message::SwitchMode(ViewMode::Downloaded))
                .style(if state.view_mode == ViewMode::Downloaded { common::primary_button_style } else { common::ghost_button_style })
                .padding(12),
        ].spacing(15),
    ].spacing(20);

    let error_view: Element<Message, Theme, Renderer> = if let Some(err) = &state.error_message {
        container(row![
            button(text("X").font(crate::common::APP_FONT)).on_press(Message::ClearError).padding(5),
            text(err).font(crate::common::APP_FONT).size(14).color(Color::from_rgb(1.0, 0.4, 0.4)).width(Length::Fill),
        ].spacing(15).align_y(Alignment::Center))
        .padding(12)
        .style(|_| container::Style::default().background(Color::from_rgb(0.25, 0.1, 0.1)))
        .into()
    } else { column![].into() };

    let status_view = text(&state.status_message).font(crate::common::APP_FONT).size(14).color(common::COLOR_TEXT_DIM);

    let progress_view: Element<Message, Theme, Renderer> = if let Some(p) = state.download_progress {
        column![
            text(format!("Processing... {:.0}%", p * 100.0)).font(crate::common::APP_FONT).size(14),
            row![
                progress_bar(0.0..=1.0, p),
                button(text("Cancel").font(crate::common::APP_FONT).size(12))
                    .on_press(Message::CancelDownload)
                    .padding(5)
                    .style(common::ghost_button_style),
            ].spacing(10).align_y(Alignment::Center)
        ].spacing(8).into()
    } else { column![].into() };

    let main_content: Element<Message, Theme, Renderer> = if let Some(handle) = &state.current_image {
        preview::view(handle)
    } else if let Some(product_name) = &state.selected_product {
        column![
            button(text("← Back").font(crate::common::APP_FONT)).on_press(Message::BackToList).padding(8).style(common::ghost_button_style),
            text(format!("Product: {}", product_name)).font(crate::common::APP_FONT).size(20),
            scrollable(column(state.product_files.iter().map(|item| {
                let previewable = is_previewable(&item.name);
                let label_color = if item.exists_locally { Color::from_rgb(0.3, 0.8, 0.3) } else { common::COLOR_TEXT };
                
                let controls: Element<Message, Theme, Renderer> = if previewable {
                     row![
                        button(text("Download").font(crate::common::APP_FONT)).on_press(Message::DownloadFile(item.s3_key.clone())).padding(8).style(common::primary_button_style),
                        button(text("View").font(crate::common::APP_FONT)).on_press(Message::ViewFile(item.s3_key.clone())).padding(8).style(common::primary_button_style)
                     ].spacing(8).into()
                } else {
                    button(text("Download").font(crate::common::APP_FONT)).on_press(Message::DownloadFile(item.s3_key.clone())).padding(8).style(common::primary_button_style).into()
                };

                container(row![
                    icon_text(if item.exists_locally { "✅" } else { "📄" }, &item.name, label_color),
                    Space::new().width(Length::Fill),
                    controls
                ].spacing(25).align_y(Alignment::Center))
                .padding(12)
                .style(common::surface_container_style)
                .into()
            }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(10)).height(Length::Fill)
        ].spacing(20).into()
    } else {
        match state.view_mode {
            ViewMode::Search => search::view(&state.search_state, &state.search_results),
            ViewMode::Browse => browse::view(&state.current_browse_path, &state.browse_items, &state.status_message, false, state.next_token.is_some()),
            ViewMode::Downloaded => downloaded::view(&state.downloaded_state, &state.local_files),
        }
    };

    container(column![title_bar, status_view, error_view, progress_view, main_content].spacing(20).padding(25))
        .width(Length::Fill).height(Length::Fill)
        .style(common::main_container_style)
        .into()
}
