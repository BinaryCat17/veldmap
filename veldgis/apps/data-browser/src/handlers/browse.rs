use std::sync::{Arc, Mutex};
use veldmap_api::data_browser::BrowseRequest;
use veldsdk::core::Command;

use crate::state::State;

/// Браузинг запрошен
pub fn on_browse(
    _state: Arc<Mutex<State>>,
    request: BrowseRequest,
) -> Command<()> {
    // TODO: Переписать на pub/sub как download
    // Публикуем запрос к data-provider
    veldsdk::publish!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path: request.path,
        token: request.token,
    });
    
    Command::none()
}
