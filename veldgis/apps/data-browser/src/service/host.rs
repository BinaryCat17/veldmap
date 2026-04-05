//! service/host.rs — все RPC-вызовы

use anyhow::Result;
use veldsdk::core::Command;
use veldmap_api::dataprovider::{
    SearchRequest, ListPathRequest, DownloadRequest,
};
use crate::AppMessage;
use crate::screens::{SearchMessage, BrowseMessage, DownloadedMessage, PreviewMessage};

pub fn start_search(req: SearchRequest) -> Command<AppMessage> {
    veldmap_api::raw::search_task(req, |update| {
        AppMessage::Search(SearchMessage::Update(update))
    })
}

pub fn start_browse(req: ListPathRequest) -> Command<AppMessage> {
    veldmap_api::raw::list_path_task(req, |update| {
        AppMessage::Browse(BrowseMessage::Update(update))
    })
}

pub fn start_download(s3_key: String) -> Command<AppMessage> {
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    let dest = format!("data/dem/source/{}", filename);
    let req = DownloadRequest { identifier: s3_key.clone(), destination: dest };

    veldmap_api::raw::download_task(req, |update| {
        AppMessage::Downloaded(DownloadedMessage::DownloadUpdate(update))
    })
}

pub fn start_image_load(path: String) -> Command<AppMessage> {
    let req = veldsdk::rpc::core::ImageLoadRequest {
        path,
        target_width: 2048,
        target_height: 2048,
        preserve_aspect: true,
    };

    veldsdk::core::raw::image_load_task(req, |update| {
        AppMessage::Preview(PreviewMessage::ImageUpdate(update))
    })
}

pub fn refresh_local_files() -> Vec<crate::common::BrowserItem> {
    let path = "data/dem/source";
    if let Ok(res) = veldsdk::core::raw::fs_list(&veldsdk::rpc::core::FsListRequest { path: path.into() }) {
        res.entries.into_iter().map(|name| crate::common::BrowserItem {
            s3_key: format!("{}/{}", path, name),
            name,
            description: None,
            is_folder: false,
            exists_locally: true,
            is_downloading: false,
        }).collect()
    } else {
        vec![]
    }
}

pub fn delete_local_file(path: String) -> Result<()> {
    let _ = veldsdk::core::raw::fs_delete(&veldsdk::rpc::core::FsDeleteRequest { path });
    Ok(())
}
