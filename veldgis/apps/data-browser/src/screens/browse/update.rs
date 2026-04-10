use veldsdk::core::Command;
use veldmap_api::dataprovider::ListPathRequest;
use crate::{
    AppMessage,
    app::state::GlobalState,
    service::host,
    common::BrowserItem,
};
use super::{BrowseState, message::Message};

pub fn update(
    state: &mut BrowseState,
    msg: Message,
    global: &mut GlobalState,
) -> Command<AppMessage> {
    match msg {
        // === Переход в конкретный путь ===
        Message::BrowsePath(path) => {
            global.status_message = format!("Listing /{}...", path);
            state.items.clear();
            state.page_tokens = vec![String::new()];
            state.current_page = 0;
            state.current_path = path.clone();
            state.is_loading = true;

            let req = ListPathRequest {
                path,
                token: String::new(),
            };
            host::start_browse(req)
        }

        // === Обработка обновления задачи ===
        Message::Update(update) => {
            log::info!("Browse Update: items before = {}, loading = {}", state.items.len(), state.is_loading);

            match update {
                veldsdk::core::task::TaskUpdate::Started(_) => {}
                veldsdk::core::task::TaskUpdate::Progress(..) => {}
                veldsdk::core::task::TaskUpdate::Finished(Ok(response)) => {
                    state.is_loading = false;
                    if !response.error.is_empty() {
                        global.error_message = Some(format!("S3 Error: {}", response.error));
                    } else {
                        let local_files = host::refresh_local_files();
                        state.items = response.items.iter().map(|s3_key| {
                            let is_folder = s3_key.ends_with('/');
                            let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(s3_key).to_string();
                            let exists_locally = !is_folder && local_files.iter().any(|f| f.name == name);

                            BrowserItem {
                                s3_key: s3_key.clone(),
                                name,
                                description: None,
                                is_folder,
                                exists_locally,
                            }
                        }).collect();

                        if !response.next_token.is_empty() {
                            if state.current_page == state.page_tokens.len() - 1 {
                                state.page_tokens.push(response.next_token.clone());
                            } else {
                                state.page_tokens[state.current_page + 1] = response.next_token.clone();
                            }
                        } else {
                            state.page_tokens.truncate(state.current_page + 1);
                        }

                        log::info!("Browse items updated: {} items", state.items.len());
                        global.status_message = format!("Loaded {} items", state.items.len());
                        
                        if state.items.is_empty() && state.current_page + 1 < state.page_tokens.len() {
                            state.current_page += 1;
                            let token = state.page_tokens[state.current_page].clone();
                            
                            let req = ListPathRequest {
                                path: state.current_path.clone(),
                                token,
                            };
                            return host::start_browse(req);
                        }
                    }
                }
                veldsdk::core::task::TaskUpdate::Finished(Err(err)) => {
                    state.is_loading = false;
                    global.error_message = Some(format!("Browse Task Failed: {}", err));
                }
            }
            Command::none()
        }

        // === Пагинация Next ===
        Message::NextPage => {
            log::info!("NextPage: current_page={}, items={}", state.current_page, state.items.len());
            if state.current_page + 1 < state.page_tokens.len() {
                state.current_page += 1;
                state.is_loading = true;
                let token = state.page_tokens[state.current_page].clone();

                let req = ListPathRequest {
                    path: state.current_path.clone(),
                    token: token.clone(),
                };
                log::info!("Starting browse with token='{}'", token);
                host::start_browse(req)
            } else {
                log::info!("NextPage: SKIPPED — no next token!");
                Command::none()
            }
        }

        // === Пагинация Prev ===
        Message::PrevPage => {
            if state.current_page > 0 {
                state.current_page -= 1;
                state.is_loading = true;
                let token = state.page_tokens[state.current_page].clone();

                let req = ListPathRequest {
                    path: state.current_path.clone(),
                    token,
                };
                host::start_browse(req)
            } else {
                Command::none()
            }
        }

        // === Клик "Вверх" ===
        Message::BrowseUp => {
            let current = state.current_path.trim_end_matches('/');
            if current.is_empty() {
                return Command::none();
            }

            let parent = if let Some(last_slash) = current.rfind('/') {
                format!("{}/", &current[..last_slash])
            } else {
                String::new()
            };

            // Переиспользуем BrowsePath
            update(state, Message::BrowsePath(parent), global)
        }
    }
}
