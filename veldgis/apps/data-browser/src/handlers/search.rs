use std::sync::{Arc, Mutex};
use veldsdk::core::{Command, task::TaskUpdate};
use veldsdk::prost::Message;
use veldmap_api::data_browser::SearchRequest;

use crate::state::State;
use crate::components::task_manager::TaskKind;

pub fn search(
    state: Arc<Mutex<State>>,
    request: SearchRequest,
) -> Command<TaskUpdate<Vec<u8>>> {
    let state_clone = state.clone();
    
    let provider_req = veldmap_api::dataprovider::SearchRequest {
        query: request.query.clone(),
        filters: vec![],
    };
    
    veldmap_api::data_provider::search(provider_req)
        .map(move |update| {
            match &update {
                TaskUpdate::Started(Some(_task_id)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.search.is_loading = true;
                    guard.global.task_manager.spawn(TaskKind::Search { 
                        query: request.query 
                    });
                }
                TaskUpdate::Progress(_p, _) => {}
                TaskUpdate::Finished(Ok(response)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.search.is_loading = false;
                    guard.search.results = response.products.clone();
                    guard.global.status_message = format!("Found {} products", response.products.len());
                }
                TaskUpdate::Finished(Err(e)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.search.is_loading = false;
                    guard.global.error_message = Some(e.clone());
                }
                _ => {}
            }
            
            match update {
                TaskUpdate::Started(id) => TaskUpdate::Started(id),
                TaskUpdate::Progress(p, id) => TaskUpdate::Progress(p, id),
                TaskUpdate::Finished(Ok(resp)) => TaskUpdate::Finished(Ok(resp.encode_to_vec())),
                TaskUpdate::Finished(Err(e)) => TaskUpdate::Finished(Err(e)),
            }
        })
}
