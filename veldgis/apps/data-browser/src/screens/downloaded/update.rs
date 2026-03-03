//! downloaded/update.rs — вся бизнес-логика экрана скачанных файлов
//! (бывшие handle_local_search, handle_local_filter, handle_download,
//! handle_download_update, handle_delete, handle_view)

use veldsdk::core::Command;
use crate::{
    AppMessage,
    app::state::GlobalState,
    service::host,
};
use super::{DownloadedState, message::Message};

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
            global.downloading_key = Some(s3_key.clone());
            global.status_message = format!("Downloading {}...", s3_key.split('/').last().unwrap_or("file"));
            host::start_download(s3_key)
        }

        // Обработка обновления задачи скачивания
        Message::DownloadUpdate(update) => {
            global.download_task.handle(update);

            if let veldsdk::core::task::TaskStatus::Finished(res) = &global.download_task {
                global.downloading_key = None;

                if !res.error.is_empty() {
                    global.error_message = Some(format!("Download Error: {}", res.error));
                } else {
                    global.status_message = "Download complete".to_string();
                    // Обновляем список локальных файлов
                    global.local_files = host::refresh_local_files();
                }
            } else if let Some(err) = global.download_task.error() {
                global.downloading_key = None;
                global.error_message = Some(format!("Download Task Failed: {}", err));
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
            // Запускаем загрузку изображения
            let cmd = host::start_image_load(path);
            // Переключаем экран
            // (в реальной версии здесь можно было бы сделать SwitchMode, но пока просто запускаем задачу)
            cmd
        }
    }
}
