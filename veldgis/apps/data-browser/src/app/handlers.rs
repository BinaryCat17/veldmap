//! app/handlers.rs

use anyhow::Result;
use veldsdk::core::Command;
use veldsdk::prost::Message;
use crate::{AppState, AppMessage};
use crate::screens::{SearchMessage, BrowseMessage, DownloadedMessage, PreviewMessage};
use crate::common::ViewMode;

pub fn module_init(_cfg: crate::common::LocalConfig) -> Result<(AppState, ())> {
    let mut state = AppState::default();
    state.global.status_message = "VeldMap Data Browser Ready".to_string();
    Ok((state, ()))
}

// ==================== ДЕЛЕГИРОВАНИЕ ====================

pub fn handle_search(state: &mut AppState, msg: SearchMessage) -> Command<AppMessage> {
    if let crate::app::state::Screen::Search(s) = &mut state.screen {
        crate::screens::search::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_browse(state: &mut AppState, msg: BrowseMessage) -> Command<AppMessage> {
    if let crate::app::state::Screen::Browse(s) = &mut state.screen {
        crate::screens::browse::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_downloaded(state: &mut AppState, msg: DownloadedMessage) -> Command<AppMessage> {
    if let crate::app::state::Screen::Downloaded(s) = &mut state.screen {
        crate::screens::downloaded::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_preview(state: &mut AppState, msg: PreviewMessage) -> Command<AppMessage> {
    if let crate::app::state::Screen::Preview(s) = &mut state.screen {
        crate::screens::preview::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_switch_mode(state: &mut AppState, mode: ViewMode) -> Command<AppMessage> {
    state.screen = match mode {
        ViewMode::Search => crate::app::state::Screen::Search(crate::screens::search::SearchState::default()),
        ViewMode::Browse => {
            let mut bs = crate::screens::browse::BrowseState::default();
            bs.current_path = String::new();
            crate::app::state::Screen::Browse(bs)
        }
        ViewMode::Downloaded => crate::app::state::Screen::Downloaded(crate::screens::downloaded::DownloadedState::default()),
        ViewMode::View => crate::app::state::Screen::Preview(crate::screens::preview::PreviewState::default()),
    };

    if let crate::app::state::Screen::Browse(browse_state) = &mut state.screen {
        if browse_state.current_path.is_empty() && browse_state.items.is_empty() {
            return crate::screens::browse::update(
                browse_state,
                crate::screens::browse::message::Message::BrowsePath(String::new()),
                &mut state.global,
            );
        }
    }

    if let crate::app::state::Screen::Downloaded(_) = &state.screen {
        state.global.local_files = crate::service::host::refresh_local_files();
    }

    Command::none()
}

pub fn handle_clear_error(state: &mut AppState) -> Command<AppMessage> {
    state.global.error_message = None;
    Command::none()
}

pub fn handle_cancel_download(state: &mut AppState) -> Command<AppMessage> {
    state.global.downloading_key = None;
    state.global.download_task = veldsdk::core::task::TaskStatus::Idle;
    state.global.status_message = "Download cancelled".to_string();
    Command::none()
}
