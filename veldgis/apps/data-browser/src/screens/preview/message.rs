use veldsdk::core::task::TaskUpdate;
use veldsdk::rpc::core::ResourceHandle;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Message {
    /// Обновление от задачи загрузки изображения (ImageUpdate)
    ImageUpdate(TaskUpdate<ResourceHandle>),

    /// Закрытие предпросмотра и возврат на экран Downloaded
    ClosePreview,
}
