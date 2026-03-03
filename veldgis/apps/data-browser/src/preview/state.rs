use veldsdk::rpc::core::ResourceHandle;

#[derive(Default)]
pub struct PreviewState {
    /// Загруженное изображение в GPU (ResourceHandle от хоста)
    pub current_gpu_image: Option<ResourceHandle>,

    /// Путь к файлу, который сейчас просматриваем (для отображения в заголовке)
    pub current_path: String,
}
