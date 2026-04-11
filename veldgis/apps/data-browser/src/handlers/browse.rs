use std::sync::{Arc, Mutex};
use veldsdk::core::{Command, task::TaskUpdate};
use veldsdk::prost::Message;
use veldmap_api::data_browser::BrowseRequest;

use crate::state::State;
use crate::components::task_manager::TaskKind;

pub fn browse(
    state: Arc<Mutex<State>>,
    request: BrowseRequest,
) -> Command<TaskUpdate<Vec<u8>>> {
    let state_clone = state.clone();
    
    let provider_req = veldmap_api::dataprovider::ListPathRequest {
        path: request.path,
        token: request.token,
    };
    
    veldmap_api::data_provider::list_path(provider_req)
        .map(move |update| {
            match &update {
                TaskUpdate::Started(Some(_)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.browse.is_loading = true;
                    guard.global.task_manager.spawn(TaskKind::Browse { 
                        path: request.path.clone() 
                    });
                }
                TaskUpdate::Finished(Ok(response)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.browse.is_loading = false;
                    guard.browse.items = response.items.into_iter().map(|path| {
                        let name = path.split('/').last().unwrap_or(&path).to_string();
                        crate::state::browse::BrowseItem {
                            s3_key: path.clone(),
                            name,
                            is_folder: path.ends_with('/'),
                        }
                    }).collect();
                }
                TaskUpdate::Finished(Err(e)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.browse.is_loading = false;
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
