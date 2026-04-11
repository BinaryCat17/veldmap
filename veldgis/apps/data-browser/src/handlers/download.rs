use std::sync::{Arc, Mutex};
use veldsdk::core::{Command, task::TaskUpdate};
use veldsdk::prost::Message;
use veldmap_api::data_browser::DownloadRequest;

use crate::state::{State, downloaded::DownloadStatus};
use crate::components::task_manager::TaskKind;

pub fn download(
    state: Arc<Mutex<State>>,
    request: DownloadRequest,
) -> Command<TaskUpdate<Vec<u8>>> {
    let state_clone = state.clone();
    let filename = request.s3_key.split('/').last().unwrap_or("file").to_string();
    
    let provider_req = veldmap_api::dataprovider::DownloadRequest {
        identifier: request.s3_key.clone(),
        destination: format!("data/dem/source/{}", filename),
    };
    
    veldmap_api::data_provider::download(provider_req)
        .map(move |update| {
            match &update {
                TaskUpdate::Started(Some(_)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.global.task_manager.spawn(TaskKind::Download { 
                        s3_key: request.s3_key.clone(), 
                        filename: filename.clone() 
                    });
                    guard.downloaded.active_downloads.insert(request.s3_key.clone(), crate::state::downloaded::DownloadProgress {
                        s3_key: request.s3_key.clone(),
                        progress: 0.0,
                        status: DownloadStatus::Downloading,
                    });
                }
                TaskUpdate::Progress(p, _) => {
                    let mut guard = state_clone.lock().unwrap();
                    if let Some(dl) = guard.downloaded.active_downloads.get_mut(&request.s3_key) {
                        dl.progress = *p;
                    }
                }
                TaskUpdate::Finished(Ok(_)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.global.status_message = format!("Downloaded: {}", filename);
                    guard.downloaded.active_downloads.remove(&request.s3_key);
                }
                TaskUpdate::Finished(Err(e)) => {
                    let mut guard = state_clone.lock().unwrap();
                    guard.global.error_message = Some(format!("Download failed: {}", e));
                    guard.downloaded.active_downloads.remove(&request.s3_key);
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
