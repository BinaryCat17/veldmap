pub struct PreviewState {
    /// Region id текстуры превью (владелец — мы, после transfer от
    /// image-loader); освобождается при смене/закрытии превью.
    pub current_image: Option<u64>,
    pub current_path: String,
    pub is_loading: bool,
    /// Ошибка загрузки/формата для экрана превью.
    pub error: Option<String>,
    /// Запрос к image-loader, ждущий ответа (broadcast → correlation_id).
    pub pending: veldsdk::Correlator<()>,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            current_image: None,
            current_path: String::new(),
            is_loading: false,
            error: None,
            pending: veldsdk::Correlator::new(),
        }
    }
}

impl PreviewState {
    /// Освобождает текстуру превью (после transfer от image-loader владелец
    /// — мы) и сбрасывает ошибку. Вызывается при смене файла и при уходе
    /// с экрана превью. Запрос в полёте (pending) не трогаем: его результат
    /// придёт позже и будет принят — иначе текстура утечёт без следа.
    pub fn clear(&mut self) {
        if let Some(old) = self.current_image.take() {
            veldsdk::abi::arena_free(old);
        }
        self.error = None;
    }
}
