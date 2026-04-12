use veld_ui::proto::UiEventResponse;
use veldmap_api::dataprovider::SearchResponse;
use crate::state::State;

pub fn on_search_input(state: &mut State, event: UiEventResponse) {
    state.search.query = event.value;
}

pub fn on_search(state: &mut State, _event: UiEventResponse) {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        veldsdk::publish!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
            query,
            filters: vec![],
        });
    }
}

/// Результат поиска
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    state.search.is_loading = false;
    state.search.results = response.products;
    
    let root = crate::view::build_root(state);
    let (w, h) = state.last_layout.as_ref().map(|l| (l.width, l.height)).unwrap_or((1024, 768));
    veld_ui::app::render("data-browser", root, &mut state.last_layout, w, h);
}
