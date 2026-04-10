//! preview/update.rs — вся бизнес-логика экрана предпросмотра
//! (бывшие handle_view и handle_image_update + ClosePreview)

use veldsdk::core::Command;
use crate::{
    AppMessage,
    app::state::GlobalState,
};
use super::{PreviewState, message::Message};

pub fn update(
    state: &mut PreviewState,
    msg: Message,
    global: &mut GlobalState,
) -> Command<AppMessage> {
    match msg {
        // === Загрузка изображения ===
        Message::ImageUpdate(update) => {
            global.image_task.handle(update);

            if let veldsdk::core::task::TaskStatus::Finished(handle) = &global.image_task {
                log::info!("Image loaded: handle.id = {}, handle.size = {}", handle.id, handle.size);
                state.current_gpu_image = Some(handle.clone());
                global.status_message = "Image loaded successfully".to_string();
            } else if let Some(err) = global.image_task.error() {
                log::error!("Image load failed: {}", err);
                global.error_message = Some(format!("Image load failed: {}", err));
                // Автоматически возвращаемся назад при ошибке
                return Command::perform(async {}, |_| AppMessage::SwitchMode(crate::common::ViewMode::Downloaded));
            }
            Command::none()
        }

        // === Закрытие предпросмотра ===
        Message::ClosePreview => {
            state.current_gpu_image = None;
            state.current_path = String::new();
            global.status_message = "Preview closed".to_string();
            
            Command::perform(async {}, |_| AppMessage::SwitchMode(crate::common::ViewMode::Downloaded))
        }
    }
}
