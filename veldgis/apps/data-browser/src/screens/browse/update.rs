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
            state.next_token = None;
            state.token_stack.clear();
            state.current_page_token = String::new();
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
            global.browse_task.handle(update.clone());
            state.is_loading = false;

            if let veldsdk::core::task::TaskStatus::Finished(response) = &global.browse_task {
                if !response.error.is_empty() {
                    global.error_message = Some(format!("S3 Error: {}", response.error));
                } else {
                    // Обновляем exists_locally через локальные файлы
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

                    state.next_token = if response.next_token.is_empty() {
                        None
                    } else {
                        Some(response.next_token.clone())
                    };

                    global.status_message = format!("Loaded {} items", state.items.len());
                    
                    // Если список пустой но есть next_token, автоматически загружаем следующую страницу
                    // (некоторые API возвращают пустую первую страницу с токеном)
                    if state.items.is_empty() && state.next_token.is_some() {
                        let token = state.next_token.clone().unwrap();
                        state.token_stack.push(state.current_page_token.clone());
                        state.current_page_token = token.clone();
                        
                        let req = ListPathRequest {
                            path: state.current_path.clone(),
                            token,
                        };
                        return host::start_browse(req);
                    }
                }
            } else if let Some(err) = global.browse_task.error() {
                global.error_message = Some(format!("Browse Task Failed: {}", err));
            }
            Command::none()
        }

        // === Пагинация Next ===
        Message::NextPage => {
            log::info!("NextPage clicked: next_token={:?}, current_page_token='{}'", state.next_token, state.current_page_token);
            if let Some(token) = state.next_token.clone() {
                state.token_stack.push(state.current_page_token.clone());
                state.current_page_token = token.clone();
                state.is_loading = true;

                let req = ListPathRequest {
                    path: state.current_path.clone(),
                    token: token.clone(),
                };
                log::info!("Starting browse with token='{}'", token);
                host::start_browse(req)
            } else {
                log::info!("NextPage: no next_token available");
                Command::none()
            }
        }

        // === Пагинация Prev ===
        Message::PrevPage => {
            if let Some(token) = state.token_stack.pop() {
                state.current_page_token = token.clone();
                state.is_loading = true;

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
