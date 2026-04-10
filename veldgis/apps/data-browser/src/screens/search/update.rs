//! search/update.rs — вся бизнес-логика экрана поиска
//! (бывшие handle_search_input, handle_search_filter, handle_search_press, handle_search_update)

use veldsdk::core::Command;
use veldmap_api::dataprovider::{SearchRequest, SearchFilter};
use crate::{
    AppMessage,
    app::state::GlobalState,
    service::host,
};
use super::{SearchState, message::Message};

pub fn update(
    state: &mut SearchState,
    msg: Message,
    global: &mut GlobalState,
) -> Command<AppMessage> {
    match msg {
        // Простые обновления состояния
        Message::InputChanged(query) => {
            state.query = query;
            Command::none()
        }

        Message::FilterChanged(filter) => {
            state.filter_type = filter;
            Command::none()
        }

        // Запуск поиска
        Message::Pressed => {
            global.status_message = "Searching...".to_string();
            state.is_loading = true;
            state.results.clear();

            let mut filters = Vec::new();
            match state.filter_type {
                super::state::SearchFilterType::GridId => {
                    filters.push(SearchFilter {
                        name: "gridId".into(),
                        value: state.query.clone(),
                    });
                }
                super::state::SearchFilterType::Collection => {
                    filters.push(SearchFilter {
                        name: "Collection".into(),
                        value: state.query.clone(),
                    });
                }
                _ => {}
            }

            let query = if state.filter_type == super::state::SearchFilterType::General {
                state.query.clone()
            } else {
                String::new()
            };

            let req = SearchRequest { query, filters };
            host::start_search(req)
        }

        // Обработка обновлений от задачи
        Message::Update(update) => {
            match update {
                veldsdk::core::task::TaskUpdate::Started(_) => {}
                veldsdk::core::task::TaskUpdate::Progress(..) => {}
                veldsdk::core::task::TaskUpdate::Finished(Ok(res)) => {
                    state.is_loading = false;
                    if !res.error.is_empty() {
                        global.error_message = Some(format!("Search API Error: {}", res.error));
                    } else {
                        state.results = res.products;
                        global.status_message = format!("Found {} results", state.results.len());
                    }
                }
                veldsdk::core::task::TaskUpdate::Finished(Err(err)) => {
                    state.is_loading = false;
                    global.error_message = Some(format!("Search Task Failed: {}", err));
                }
            }
            Command::none()
        }
    }
}
