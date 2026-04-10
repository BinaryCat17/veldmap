use veldsdk::core::Command;
use veldmap_api::dataprovider::{ListPathRequest, ListPathResponse};
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
        Message::BrowsePath(path) => {
            global.status_message = format!("Listing /{}...", path);
            state.items.clear();
            state.page_tokens = vec![String::new()];
            state.current_page = 0;
            state.current_path = path.clone();
            state.is_loading = true;

            host::start_browse(ListPathRequest { path, token: String::new() })
        }

        Message::Update(update) => {
            match update {
                veldsdk::core::task::TaskUpdate::Finished(Ok(response)) => {
                    handle_browse_success(state, global, response)
                }
                veldsdk::core::task::TaskUpdate::Finished(Err(err)) => {
                    state.is_loading = false;
                    global.error_message = Some(format!("Browse Task Failed: {}", err));
                    Command::none()
                }
                _ => Command::none(), // Started, Progress
            }
        }

        Message::NextPage => {
            if state.current_page + 1 < state.page_tokens.len() {
                state.current_page += 1;
                state.is_loading = true;
                let token = state.page_tokens[state.current_page].clone();
                host::start_browse(ListPathRequest { path: state.current_path.clone(), token })
            } else {
                Command::none()
            }
        }

        Message::PrevPage => {
            if state.current_page > 0 {
                state.current_page -= 1;
                state.is_loading = true;
                let token = state.page_tokens[state.current_page].clone();
                host::start_browse(ListPathRequest { path: state.current_path.clone(), token })
            } else {
                Command::none()
            }
        }

        Message::BrowseUp => {
            let current = state.current_path.trim_end_matches('/');
            if current.is_empty() {
                return Command::none();
            }
            let parent = current.rfind('/').map(|i| format!("{}/", &current[..i])).unwrap_or_default();
            update(state, Message::BrowsePath(parent), global)
        }
    }
}

fn handle_browse_success(
    state: &mut BrowseState,
    global: &mut GlobalState,
    response: ListPathResponse,
) -> Command<AppMessage> {
    state.is_loading = false;

    if !response.error.is_empty() {
        global.error_message = Some(format!("S3 Error: {}", response.error));
        return Command::none();
    }

    let local_files = host::refresh_local_files();
    state.items = response.items.into_iter().map(|s3_key| {
        let is_folder = s3_key.ends_with('/');
        let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(&s3_key).to_string();
        let exists_locally = !is_folder && local_files.iter().any(|f| f.name == name);

        BrowserItem { s3_key, name, description: None, is_folder, exists_locally }
    }).collect();

    // Сохраняем токены только до текущей страницы (отбрасываем устаревшее "будущее", если вернулись назад)
    state.page_tokens.truncate(state.current_page + 1);
    
    // Если есть следующая страница, добавляем её токен
    if !response.next_token.is_empty() {
        state.page_tokens.push(response.next_token);
    }

    global.status_message = format!("Loaded {} items", state.items.len());

    // Авто-пропуск пустых страниц (особенность S3 API, где папки могут возвращаться как пустые страницы с токеном)
    if state.items.is_empty() && state.current_page + 1 < state.page_tokens.len() {
        state.current_page += 1;
        let token = state.page_tokens[state.current_page].clone();
        return host::start_browse(ListPathRequest { path: state.current_path.clone(), token });
    }

    Command::none()
}
