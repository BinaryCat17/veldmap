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
            global.browse_task.handle(update);
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
                            is_downloading: false,
                        }
                    }).collect();

                    state.next_token = if response.next_token.is_empty() {
                        None
                    } else {
                        Some(response.next_token.clone())
                    };

                    global.status_message = format!("Loaded {} items", state.items.len());
                }
            } else if let Some(err) = global.browse_task.error() {
                global.error_message = Some(format!("Browse Task Failed: {}", err));
            }
            Command::none()
        }

        // === Пагинация Next ===
        Message::NextPage => {
            if let Some(token) = state.next_token.clone() {
                state.token_stack.push(state.current_page_token.clone());
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
