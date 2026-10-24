use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{State, ViewId, ViewKind};
use crate::proto::data_provider::{SearchRequest, SearchResponse};

pub fn on_query(state: &mut State, view: ViewId, query: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.query = query;
}

pub fn on_mission(state: &mut State, view: ViewId, mission: Mission) {
    let Some(search) = state.search_mut(view) else { return };
    search.mission = mission;
    let dropped = !mission.clouded() && search.cloud != Cloud::default();
    if !mission.clouded() {
        search.cloud = Cloud::default();
    }
    if dropped {
        state.notice = Some("Cloud cover ceiling removed: this mission does not measure it".to_string());
    }
    state.refine(view);
    run(state, view);
}

pub fn on_from(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.from = value;
}

pub fn on_to(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.to = value;
}

pub fn on_period(state: &mut State, view: ViewId, period: Period) {
    let Some(search) = state.search_mut(view) else { return };
    search.period = period;
    state.refine(view);
    run(state, view);
}

pub fn on_cloud(state: &mut State, view: ViewId, cloud: Cloud) {
    let Some(search) = state.search_mut(view) else { return };
    search.cloud = cloud;
    state.refine(view);
    run(state, view);
}

pub fn run(state: &mut State, view: ViewId) {
    state.refine(view);
    let now = crate::module::components::format::now();
    let Some(search) = state.search_mut(view) else { return };
    let (from, to) = search.window(now);
    let request = SearchRequest {
        mission: search.mission.collection().to_string(),
        name: search.query.clone(),
        area: None,
        from,
        to,
        max_cloud: search.cloud.max(),
        limit: 0,
    };
    search.error = None;
    let correlation_id = search.request.begin();
    state.searches.insert(correlation_id.clone(), view);

    crate::calls::data_provider::on_search(&request, &correlation_id);
}

pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.searches.take(&correlation_id) else { return };
    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };

    if search.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        search.error = Some(response.error);
        search.results = Vec::new();
    } else {
        search.error = None;
        search.results = response.products;
    }
    search.children.clear();
    search.listing.expanded.clear();

    survivors(state, view);
}

fn survivors(state: &mut State, view: ViewId) {
    let alive: std::collections::HashSet<String> = {
        let Some(ViewKind::Search(search)) = state.get(view) else { return };
        if search.error.is_some() {
            return;
        }
        search.results.iter().map(|product| product.identifier.clone()).collect()
    };

    if let Some(listing) = state.listing_mut(view) {
        listing.selected.retain(|key, _| alive.contains(key));
    }
    super::overlay::keep_only(state, view, |identifier| alive.contains(identifier));
    super::outline::refresh(state);
}

pub fn on_source_closed(state: &mut State, id: ViewId) {
    super::overlay::source_closed(state, id);
    super::outline::refresh(state);
}

pub fn show(state: &mut State, view: ViewId, identifier: &str) -> bool {
    let Some(search) = state.search_mut(view) else { return false };
    let Some(product) = search
        .results
        .iter()
        .find(|product| product.identifier == identifier)
        .cloned()
    else {
        return false;
    };

    super::overlay::show(state, &product, Some(view));
    true
}
