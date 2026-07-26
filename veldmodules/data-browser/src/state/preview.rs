/// Экран предпросмотра. Одновременно актуален ровно один запрос: пользователь
/// смотрит один файл, и ответы на всё остальное — устаревшие.
///
/// Превью идёт в два шага одним correlation_id: открыть ресурс (fs) и отдать
/// его image-loader. Тем же идентификатором заказчик отменяет декодирование.
#[derive(Default)]
pub struct PreviewState {
    /// Текстура превью. Владелец — мы (image-loader передал владение),
    /// поэтому освобождаем её сами: при замене и при уходе с экрана.
    pub texture: Option<u64>,
    /// Открытый ресурс с файлом: наш, живёт только пока идёт декодирование.
    pub file: Option<u64>,
    pub current_path: String,
    /// correlation_id запроса, ответ на который ещё ждём.
    pub inflight: Option<String>,
    pub error: Option<String>,
}

impl PreviewState {
    pub fn is_loading(&self) -> bool {
        self.inflight.is_some()
    }

    /// Освобождает ресурс с файлом — после декодирования он не нужен.
    pub fn close_file(&mut self) {
        if let Some(file) = self.file.take() {
            veldsdk::abi::arena_free(file);
        }
    }

    /// Готовит состояние под новый файл: старая текстура больше не показывается
    /// и освобождается, запрос в полёте перестаёт быть актуальным (текстуру
    /// его ответа освободит обработчик — см. handlers::preview).
    ///
    /// Возвращает id брошенного запроса: его декодирование ещё идёт, и его
    /// стоит отменить. Публикует отмену вызывающий — состояние в шину не
    /// пишет, иначе исходящая связь модуля перестала бы быть видна в схеме.
    #[must_use]
    pub fn reset(&mut self) -> Option<String> {
        if let Some(old) = self.texture.take() {
            veldsdk::abi::arena_free(old);
        }
        self.close_file();
        self.error = None;
        self.current_path.clear();
        self.inflight.take()
    }
}
