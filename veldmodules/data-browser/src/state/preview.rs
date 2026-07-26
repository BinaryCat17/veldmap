/// Экран предпросмотра. Одновременно актуален ровно один запрос: пользователь
/// смотрит один файл, и ответы на всё остальное — устаревшие.
#[derive(Default)]
pub struct PreviewState {
    /// Текстура превью. Владелец — мы (image-loader передал владение),
    /// поэтому освобождаем её сами: при замене и при уходе с экрана.
    pub texture: Option<u64>,
    pub current_path: String,
    /// correlation_id запроса, ответ на который ещё ждём.
    pub inflight: Option<String>,
    pub error: Option<String>,
}

impl PreviewState {
    pub fn is_loading(&self) -> bool {
        self.inflight.is_some()
    }

    /// Готовит состояние под новый файл: старая текстура больше не показывается
    /// и освобождается, запрос в полёте перестаёт быть актуальным (его текстуру
    /// освободит обработчик ответа — см. handlers::preview).
    pub fn reset(&mut self) {
        if let Some(old) = self.texture.take() {
            veldsdk::abi::arena_free(old);
        }
        self.inflight = None;
        self.error = None;
        self.current_path.clear();
    }
}
