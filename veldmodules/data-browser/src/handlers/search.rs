use crate::proto::ui_service::proto::UiEventResponse;
use crate::proto::data_provider::SearchResponse;
use crate::module::state::State;

pub fn on_search_input(state: &mut State, event: UiEventResponse) {
    state.search.query = event.value;
}

pub fn on_search(state: &mut State, _event: UiEventResponse) {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        state.search.error = None;
        let correlation_id = state.search.pending.begin(());
        crate::calls::data_provider::on_search(&crate::proto::data_provider::SearchRequest {
            query,
            filters: vec![],
            correlation_id,
        });
    }
}

/// Результат поиска. Broadcast-топик — сверяем correlation_id, чтобы не
/// принять устаревший или чужой ответ.
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    if state.search.pending.take(&response.correlation_id).is_none() {
        return;
    }
    state.search.is_loading = false;

    if !response.error.is_empty() {
        state.search.error = Some(response.error);
        state.search.results = Vec::new();
        return;
    }
    state.search.error = None;

    state.search.results = response.products;
    // Рендер происходит автоматически в on_frame
}
