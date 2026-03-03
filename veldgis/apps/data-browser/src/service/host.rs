//! service/host.rs — чистый сервисный слой
//! Все низкоуровневые вызовы (RPC, fs, image) вынесены сюда
//! Возвращает Command<AppMessage> с правильными вложенными сообщениями

use anyhow::Result;
use veldsdk::core::{Command, task::TaskStatus};
use veldmap_gis_api::dataprovider::{
    SearchRequest, ListPathRequest, DownloadRequest,
};
use crate::{AppMessage, state::GlobalState};

/// Запуск поиска (из search::update)
pub fn start_search(req: SearchRequest) -> Command<AppMessage> {
    veldmap_gis_api::raw::search_task(req, |update| {
        AppMessage::Search(crate::search::Message::Update(update))
    })
}

/// Запуск листинга S3 (из browse::update)
pub fn start_browse(req: ListPathRequest) -> Command<AppMessage> {
    veldmap_gis_api::raw::list_path_task(req, |update| {
        AppMessage::Browse(crate::browse::Message::Update(update))
    })
}

/// Запуск скачивания файла
pub fn start_download(s3_key: String) -> Command<AppMessage> {
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    let dest = format!("data/dem/source/{}", filename);
    let req = DownloadRequest { identifier: s3_key.clone(), destination: dest };

    veldmap_gis_api::raw::download_task(req, |update| {
        AppMessage::Downloaded(crate::downloaded::Message::DownloadUpdate(update))
    })
}

/// Загрузка изображения для предпросмотра
pub fn start_image_load(path: String) -> Command<AppMessage> {
    let req = veldsdk::rpc::core::ImageLoadRequest {
        path,
        target_width: 2048,
        target_height: 2048,
        preserve_aspect: true,
    };

    veldsdk::core::raw::image_load_task(req, |update| {
        AppMessage::Preview(crate::preview::Message::ImageUpdate(update))
    })
}

/// Обновление списка локальных файлов (вызывается после скачивания/удаления)
pub fn refresh_local_files() -> Vec<crate::common::BrowserItem> {
    let path = "data/dem/source";
    if let Ok(res) = veldsdk::core::raw::fs_list(&veldsdk::rpc::core::FsListRequest { path: path.into() }) {
        res.entries
            .into_iter()
            .map(|name| crate::common::BrowserItem {
                s3_key: format!("{}/{}", path, name),
                name,
                description: None,
                is_folder: false,
                exists_locally: true,
                is_downloading: false,
            })
            .collect()
    } else {
        vec![]
    }
}

/// Удаление локального файла
pub fn delete_local_file(path: String) -> Result<()> {
    let _ = veldsdk::core::raw::fs_delete(&veldsdk::rpc::core::FsDeleteRequest { path });
    Ok(())
}

/// Отмена текущей задачи скачивания (если нужно)
pub fn cancel_current_download(global: &mut GlobalState) {
    if let TaskStatus::Running { task_id: Some(id), .. } = &mut global.download_task {
        let req = veldsdk::rpc::core::TaskCancelRequest { task_id: id.clone() };
        let _ = veldsdk::core::raw::task_cancel(&req);
    }
    global.download_task = TaskStatus::Idle;
    global.downloading_key = None;
    global.status_message = "Download cancelled".to_string();
}
