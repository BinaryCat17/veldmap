//! task_manager.rs — менеджер фоновых задач
//! 
//! Заменяет разрозненные TaskStatus поля в GlobalState единым централизованным менеджером.
//! Позволяет отображать все активные задачи в боковой панели.

/// Тип фоновой задачи
#[derive(Debug, Clone)]
pub enum TaskKind {
    Download { task_id: String, s3_key: String, filename: String },
    Browse { path: String },
    ImageLoad { path: String, filename: String },
    Search { query: String },
}

impl TaskKind {
    pub fn title(&self) -> String {
        match self {
            TaskKind::Download { filename, .. } => format!("Downloading {}", filename),
            TaskKind::Browse { path } => format!("Browsing /{}", path),
            TaskKind::ImageLoad { filename, .. } => format!("Loading {}", filename),
            TaskKind::Search { query } => format!("Search: {}", query),
        }
    }
    
    /// Проверяет, относится ли задача к конкретному ключу (для active_download)
    pub fn matches_key(&self, key: &str) -> bool {
        match self {
            TaskKind::Download { s3_key, .. } => s3_key == key,
            _ => false,
        }
    }
}

/// Описание задачи
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: String,
    pub kind: TaskKind,
    pub progress: f32,
    /// Байтовый прогресс закачки — 0/0, пока событие ещё не пришло, или для
    /// задач, у которых его нет в принципе (Browse/Search/ImageLoad).
    pub bytes_downloaded: u64,
    /// 0, если сервер не прислал Content-Length — UI показывает только
    /// bytes_downloaded, без процента и без "из скольки".
    pub total_bytes: u64,
    pub is_finished: bool,
    pub error: Option<String>,
}

/// Менеджер фоновых задач. Хранилище — общий `Correlator<T>` (см. veldsdk),
/// а не самопальная HashMap-обвязка: тот же примитив, что используют
/// browse/search/downloaded для сопоставления запрос/ответ.
#[derive(Debug, Default, Clone)]
pub struct TaskManager {
    tasks: veldsdk::Correlator<TaskInfo>,
}

impl TaskManager {
    /// Создаёт новую задачу с заданным ID (обычно task_id из внешнего сервиса)
    pub fn spawn(&mut self, id: impl Into<String>, kind: TaskKind) -> String {
        let id = id.into();

        let info = TaskInfo {
            id: id.clone(),
            kind,
            progress: 0.0,
            bytes_downloaded: 0,
            total_bytes: 0,
            is_finished: false,
            error: None,
        };

        self.tasks.insert(id.clone(), info);
        id
    }

    /// Обновляет прогресс закачки — доля и байты вместе, единым событием
    /// (см. handlers::download::on_download_progress), чтобы не разъезжались.
    pub fn update_download_progress(&mut self, id: &str, progress: f32, bytes_downloaded: u64, total_bytes: u64) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.progress = progress.clamp(0.0, 1.0);
            task.bytes_downloaded = bytes_downloaded;
            task.total_bytes = total_bytes;
        }
    }

    /// Помечает задачу как завершённую
    pub fn finish(&mut self, id: &str) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.is_finished = true;
            task.progress = 1.0;
        }
    }

    /// Помечает задачу как failed
    pub fn fail(&mut self, id: &str, error: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.is_finished = true;
            task.error = Some(error);
        }
    }

    /// Удаляет задачу из списка
    pub fn remove(&mut self, id: &str) {
        self.tasks.remove(id);
    }

    /// Возвращает все активные (незавершённые) задачи
    pub fn active(&self) -> Vec<&TaskInfo> {
        self.tasks.values().filter(|t| !t.is_finished).collect()
    }

    /// Возвращает завершённые задачи
    pub fn completed(&self) -> Vec<&TaskInfo> {
        self.tasks.values().filter(|t| t.is_finished && t.error.is_none()).collect()
    }

    /// Количество активных задач
    pub fn active_count(&self) -> usize {
        self.active().len()
    }

    /// Проверяет есть ли активные задачи
    pub fn has_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Получает задачу по ID
    pub fn get(&self, id: &str) -> Option<&TaskInfo> {
        self.tasks.get(id)
    }

    /// Проверяет существует ли задача
    pub fn contains(&self, id: &str) -> bool {
        self.tasks.contains(id)
    }

    /// Очищает завершённые задачи
    pub fn cleanup_completed(&mut self) {
        let done: Vec<String> = self.tasks.values()
            .filter(|t| t.is_finished)
            .map(|t| t.id.clone())
            .collect();
        for id in done {
            self.tasks.remove(&id);
        }
    }

    /// Активная (незавершённая) задача закачки данного s3_key, если есть —
    /// источник и для флага "качается", и для прогресса/кнопок в browser_list.
    pub fn active_download(&self, key: &str) -> Option<&TaskInfo> {
        self.tasks.values().find(|t| !t.is_finished && t.kind.matches_key(key))
    }
}
