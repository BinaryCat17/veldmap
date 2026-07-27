/// Экран предпросмотра. Показывается ровно один файл, поэтому актуален ровно
/// один запрос — ответы на всё остальное устарели.
///
/// Превью идёт в два шага одним correlation_id: открыть ресурс (fs или
/// data-provider) и отдать его image-loader. Тем же идентификатором заказчик
/// отменяет декодирование.
#[derive(Default)]
pub struct PreviewState {
    /// Текстура превью. Владелец — мы (image-loader передал владение),
    /// поэтому освобождаем её сами: при замене и при уходе с экрана.
    pub texture: Option<u64>,
    /// Открытый ресурс с файлом: наш, живёт только пока идёт декодирование.
    pub file: Option<u64>,
    pub current_path: String,
    /// Все незакрытые запросы на открытие — включая брошенные. Брошенный
    /// остаётся на учёте до ответа не по забывчивости: ресурс придёт всё
    /// равно и придёт нам во владение, а опознать его как свой (и освободить)
    /// можно только здесь — открывают нам двое, библиотека и провайдер.
    ///
    /// Это множество, а не «текущий запрос»: чем занят экран, говорит `phase`.
    pub opening: veldsdk::Correlator<()>,
    phase: Phase,
    pub error: Option<String>,
}

/// Чем занят экран.
///
/// Раньше то же самое хранилось тремя полями: `inflight` (какой запрос
/// показываем) и `decode_task` (заведена ли задача). Инвариант «задача
/// существует только после того, как ресурс открылся» держался на том, что
/// оба поля меняют согласованно, — здесь он выражен формой типа: у `Opening`
/// задачи нет по построению.
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// Ресурс запрошен, ждём `core.ResourceOpened`. Задачи ещё нет: отменять
    /// нечего, закрывать нечего.
    Opening(String),
    /// Ресурс открыт, задача декодирования заведена, ждём ответа загрузчика.
    Decoding(String),
}

impl Phase {
    fn id(&self) -> Option<&str> {
        match self {
            Phase::Idle => None,
            Phase::Opening(id) | Phase::Decoding(id) => Some(id),
        }
    }
}

impl PreviewState {
    pub fn is_loading(&self) -> bool {
        self.phase.id().is_some()
    }

    /// Ответ относится к тому, что мы сейчас показываем? Топики широковещательные,
    /// и пока ответ шёл, пользователь мог открыть другой файл или уйти с экрана.
    pub fn is_current(&self, correlation_id: &str) -> bool {
        self.phase.id() == Some(correlation_id)
    }

    /// Заводит новый запрос: ставит его на учёт открытий и делает текущим.
    pub fn begin(&mut self) -> String {
        let correlation_id = self.opening.begin(());
        self.phase = Phase::Opening(correlation_id.clone());
        correlation_id
    }

    /// Ресурс открыт, задача заведена — переходим ко второму шагу.
    pub fn decoding(&mut self) {
        if let Phase::Opening(correlation_id) = &self.phase {
            self.phase = Phase::Decoding(correlation_id.clone());
        }
    }

    /// Работа кончилась — успехом, ошибкой или отказом. Возвращает id
    /// заведённой задачи, если она успела появиться: закрыть её (`tasks/end`)
    /// или отменить (`tasks/cancel`) — дело вызывающего, исход знает он.
    pub fn finish(&mut self) -> Option<String> {
        match std::mem::take(&mut self.phase) {
            Phase::Decoding(task_id) => Some(task_id),
            Phase::Idle | Phase::Opening(_) => None,
        }
    }

    /// Освобождает ресурс с файлом — после декодирования он не нужен.
    pub fn close_file(&mut self) {
        if let Some(file) = self.file.take() {
            veldsdk::abi::arena_free(file);
        }
    }

    /// Готовит состояние под новый файл: старая текстура больше не показывается
    /// и освобождается, запрос в полёте перестаёт быть актуальным. Учёт в
    /// `opening` при этом не снимается — ресурс по нему ещё придёт, и освободит
    /// его обработчик ответа (см. handlers::preview), как и текстуру.
    ///
    /// Возвращает id задачи, которую стоит отменить, — то есть только той,
    /// что успела начаться. Публикует отмену вызывающий: состояние в шину не
    /// пишет, иначе исходящая связь модуля перестала бы быть видна в схеме.
    #[must_use]
    pub fn reset(&mut self) -> Option<String> {
        if let Some(old) = self.texture.take() {
            veldsdk::abi::arena_free(old);
        }
        self.close_file();
        self.error = None;
        self.current_path.clear();
        self.finish()
    }
}
