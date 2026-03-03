//! handlers.rs — чистый делегирующий слой новой архитектуры
//! Вся бизнес-логика будет перенесена в подмодули (search::update, browse::update и т.д.)
//! Здесь только маршрутизация + init + топ-уровневые действия

use anyhow::Result;
use veldsdk::core::{Command, task::TaskStatus};

use crate::{AppState, AppMessage};
use crate::state::Screen;
use crate::common::ViewMode;

/// Инициализация модуля (вызывается макросом)
pub fn module_init(_cfg: crate::common::LocalConfig) -> Result<(AppState, ())> {
    let mut state = AppState::default();
    
    // Стартовое состояние
    state.global.status_message = "VeldMap Data Browser Ready".to_string();
    state.screen = Screen::Search(crate::search::SearchState::default());
    
    Ok((state, ()))
}

// ==================== ДЕЛЕГИРУЮЩИЕ HANDLERS ====================

pub fn handle_search(state: &mut AppState, msg: crate::search::Message) -> Command<AppMessage> {
    if let Screen::Search(s) = &mut state.screen {
        crate::search::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_browse(state: &mut AppState, msg: crate::browse::Message) -> Command<AppMessage> {
    if let Screen::Browse(s) = &mut state.screen {
        crate::browse::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_downloaded(state: &mut AppState, msg: crate::downloaded::Message) -> Command<AppMessage> {
    if let Screen::Downloaded(s) = &mut state.screen {
        crate::downloaded::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_preview(state: &mut AppState, msg: crate::preview::Message) -> Command<AppMessage> {
    if let Screen::Preview(s) = &mut state.screen {
        crate::preview::update(s, msg, &mut state.global)
    } else {
        Command::none()
    }
}

pub fn handle_switch_mode(state: &mut AppState, mode: ViewMode) -> Command<AppMessage> {
    // Переключаем экран
    state.screen = match mode {
        ViewMode::Search => Screen::Search(crate::search::SearchState::default()),
        ViewMode::Browse => {
            let mut browse_state = crate::browse::BrowseState::default();
            browse_state.current_path = String::new();
            Screen::Browse(browse_state)
        }
        ViewMode::Downloaded => Screen::Downloaded(crate::downloaded::DownloadedState::default()),
        ViewMode::View => Screen::Preview(crate::preview::PreviewState::default()),
    };

    // === АВТОМАТИЧЕСКОЕ ОБНОВЛЕНИЕ СПИСКОВ ===
    match &mut state.screen {
        // Browse — уже работает
        Screen::Browse(browse_state) => {
            if browse_state.current_path.is_empty() && browse_state.items.is_empty() {
                return crate::browse::update(
                    browse_state,
                    crate::browse::message::Message::BrowsePath(String::new()),
                    &mut state.global,
                );
            }
        }

        // Downloaded — добавляем обновление списка локальных файлов
        Screen::Downloaded(_) => {
            state.global.local_files = crate::service::host::refresh_local_files();
            state.global.status_message = format!("Found {} local files", state.global.local_files.len());
        }

        _ => {}
    }

    Command::none()
}

pub fn handle_clear_error(state: &mut AppState) -> Command<AppMessage> {
    state.global.error_message = None;
    Command::none()
}

pub fn handle_cancel_download(state: &mut AppState) -> Command<AppMessage> {
    state.global.downloading_key = None;
    state.global.download_task = TaskStatus::Idle;
    state.global.status_message = "Download cancelled".to_string();
    Command::none()
}
