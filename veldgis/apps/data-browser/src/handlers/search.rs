use std::sync::{Arc, Mutex};
use veldmap_api::data_browser::SearchRequest;
use veldsdk::core::Command;

use crate::state::State;

/// Поиск запрошен
pub fn on_search(
    _state: Arc<Mutex<State>>,
    request: SearchRequest,
) -> Command<()> {
    // TODO: Переписать на pub/sub как download
    // Публикуем запрос к data-provider
    veldsdk::publish!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
        query: request.query,
        filters: vec![],
    });
    
    Command::none()
}
