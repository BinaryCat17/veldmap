//! downloaded/update.rs — вся бизнес-логика экрана скачанных файлов
//! (с TaskManager)

use veldsdk::core::Command;
use crate::{
    AppMessage,
    app::state::GlobalState,
    service::host,
    service::task_manager::TaskKind,
};
use super::{DownloadedState, message::Message};

/// Запуск скачивания файла (может быть вызван с любого экрана)
/// Теперь создаёт задачу в TaskManager и возвращает Command для отслеживания
pub fn update_download_file(global: &mut GlobalState, s3_key: String) -> Command<AppMessage> {
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    global.status_message = format!("Downloading {}...", filename);
    
    // Сохраняем текущую загрузку для обновления TaskManager
    global.current_download = Some(s3_key.clone());
    
    // Создаём задачу в TaskManager
    let _task_id = global.task_manager.spawn(TaskKind::Download { 
        s3_key: s3_key.clone(), 
        filename: filename.clone() 
    });
    
    // Запускаем скачивание
    host::start_download(s3_key)
}

pub fn update(
    state: &mut DownloadedState,
    msg: Message,
    global: &mut GlobalState,
) -> Command<AppMessage> {
    match msg {
        // Простые обновления состояния
        Message::LocalSearchChanged(query) => {
            state.search_query = query;
            Command::none()
        }

        Message::LocalFilterChanged(filter) => {
            state.filter = filter;
            Command::none()
        }

        // Запуск скачивания
        Message::DownloadFile(s3_key) => {
            update_download_file(global, s3_key)
        }

        // Обработка обновления задачи скачивания
        Message::DownloadUpdate(update) => {
            global.download_task.handle(update);

            // Обновляем TaskManager на основе статуса задачи
            if let Some(s3_key) = &global.current_download {
                match &global.download_task {
                    veldsdk::core::task::TaskStatus::Running { progress, .. } => {
                        global.task_manager.update_progress_by_key(s3_key, *progress);
                    }
                    veldsdk::core::task::TaskStatus::Finished(res) => {
                        global.task_manager.finish_by_key(s3_key);
                        global.current_download = None;
                        
                        if !res.error.is_empty() {
                            global.error_message = Some(format!("Download Error: {}", res.error));
                        } else {
                            global.status_message = "Download complete".to_string();
                            global.local_files = host::refresh_local_files();
                        }
                    }
                    veldsdk::core::task::TaskStatus::Failed(err) => {
                        global.task_manager.fail_by_key(s3_key, err.clone());
                        global.current_download = None;
                        global.error_message = Some(format!("Download Task Failed: {}", err));
                    }
                    _ => {}
                }
            }
            Command::none()
        }

        // Удаление локального файла
        Message::DeleteLocalFile(path) => {
            let _ = host::delete_local_file(path);
            global.local_files = host::refresh_local_files();
            global.status_message = "File deleted".to_string();
            Command::none()
        }

        // Просмотр файла → переключаемся на экран Preview
        Message::ViewFile(path) => {
            global.status_message = format!("Loading preview for {}...", path);
            // Создаём задачу загрузки изображения
            let filename = path.split('/').last().unwrap_or("image").to_string();
            let _task_id = global.task_manager.spawn(TaskKind::ImageLoad { 
                path: path.clone(), 
                filename 
            });
            
            // Запускаем загрузку изображения
            let cmd_load = host::start_image_load(path);
            // Переключаем экран
            let cmd_switch = Command::perform(async {}, |_| AppMessage::SwitchMode(crate::common::ViewMode::View));
            Command::batch(vec![cmd_load, cmd_switch])
        }
    }
}
