/// Экран предпросмотра. Показывается ровно один файл, поэтому актуален ровно
/// один запрос — ответы на всё остальное устарели.
///
/// Превью идёт в два шага одной корреляцией: открыть ресурс (fs или
/// data-provider) и отдать его image-loader. Тем же идентификатором заказчик
/// отменяет декодирование.
#[derive(Default)]
pub struct PreviewState {
    /// Текстура превью. Владелец — мы (image-loader передал владение),
    /// поэтому освобождаем её сами: при замене и при уходе с экрана.
    pub texture: Option<veldsdk::OwnedResource>,
    /// Открытый ресурс с файлом: наш, живёт только пока идёт декодирование.
    pub file: Option<veldsdk::OwnedResource>,
    pub current_path: String,
    /// Запрос на показ. Актуален последний; вытесненный остаётся на учёте до
    /// ответа не по забывчивости: ресурс по нему придёт всё равно и придёт нам
    /// во владение, а опознать его как свой (и освободить) можно только здесь —
    /// открывают нам двое, библиотека и провайдер.
    pub request: veldsdk::Latest,
    /// Задача декодирования в реестре платформы (id — корреляция запроса).
    /// Появляется только после того, как ресурс открылся: пока идёт открытие,
    /// отменять и закрывать нечего.
    task: Option<veldsdk::TaskGuard>,
    pub error: Option<String>,
}

impl PreviewState {
    pub fn is_loading(&self) -> bool {
        self.request.is_pending()
    }

    /// Заводит новый запрос: он становится актуальным, предыдущий — нет.
    pub fn begin(&mut self) -> String {
        let id = self.request.begin();
        self.task = Some(veldsdk::TaskGuard::new(id.clone()));
        id
    }

    /// Ресурс открыт — заводим задачу декодирования и переходим ко второму
    /// шагу. Публикацию выполняет вызывающий своим стабом: состояние в шину
    /// не пишет, иначе исходящая связь модуля перестала бы быть видна в схеме.
    pub fn begin_task(&mut self, kind: &str, executor: &str,
                      emit: impl FnOnce(&veldsdk::proto::tasks::TaskBeginRequest)) {
        let label = self.current_path.clone();
        if let Some(task) = &mut self.task {
            task.begin(kind, &label, executor, emit);
        }
    }

    /// Декодирование кончилось: закрываем задачу с исходом ответа, если она
    /// успела появиться (guard сам следит, чтобы `on_end` ушёл ровно один раз).
    pub fn end_task(&mut self, error: &str,
                    emit: impl FnOnce(&veldsdk::proto::tasks::TaskEndRequest)) {
        if let Some(task) = &mut self.task {
            task.end(error, emit);
        }
    }

    /// Освобождает ресурс с файлом — после декодирования он не нужен.
    pub fn close_file(&mut self) {
        // Drop у OwnedResource освобождает регион.
        self.file = None;
    }

    /// Готовит состояние под новый файл: старая текстура больше не показывается
    /// и освобождается, запрос в полёте перестаёт быть актуальным. Учёт при
    /// этом не снимается — ресурс по нему ещё придёт, и освободит его
    /// обработчик ответа (см. handlers::preview), как и текстуру.
    ///
    /// Задача декодирования отменяется, если успела начаться (guard сам решает,
    /// есть ли что отменять); отмену публикует вызывающий своим стабом.
    pub fn reset(&mut self, cancel: impl FnOnce(&veldsdk::proto::tasks::TaskCancelRequest)) {
        // Старая текстура больше не показывается и освобождается (Drop).
        self.texture = None;
        self.close_file();
        self.error = None;
        self.current_path.clear();

        self.request.abandon();
        if let Some(task) = &mut self.task {
            task.cancel(cancel);
        }
    }
}
