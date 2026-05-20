//! task_manager.rs — менеджер фоновых задач
//! 
//! Заменяет разрозненные TaskStatus поля в GlobalState единым централизованным менеджером.
//! Позволяет отображать все активные задачи в боковой панели.

use std::collections::HashMap;

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
    
    /// Проверяет, относится ли задача к конкретному ключу (для проверки is_downloading)
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
    pub is_finished: bool,
    pub error: Option<String>,
}

/// Менеджер фоновых задач
#[derive(Debug, Default, Clone)]
pub struct TaskManager {
    tasks: HashMap<String, TaskInfo>,
}

impl TaskManager {
    /// Создаёт новую задачу с заданным ID (обычно task_id из внешнего сервиса)
    pub fn spawn(&mut self, id: impl Into<String>, kind: TaskKind) -> String {
        let id = id.into();
        
        let info = TaskInfo {
            id: id.clone(),
            kind,
            progress: 0.0,
            is_finished: false,
            error: None,
        };
        
        self.tasks.insert(id.clone(), info);
        id
    }
    
    /// Обновляет прогресс задачи
    pub fn update_progress(&mut self, id: &str, progress: f32) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.progress = progress.clamp(0.0, 1.0);
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
        self.tasks
            .values()
            .filter(|t| !t.is_finished)
            .collect()
    }
    
    /// Возвращает завершённые задачи
    pub fn completed(&self) -> Vec<&TaskInfo> {
        self.tasks
            .values()
            .filter(|t| t.is_finished && t.error.is_none())
            .collect()
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
        self.tasks.contains_key(id)
    }
    
    /// Очищает завершённые задачи
    pub fn cleanup_completed(&mut self) {
        self.tasks.retain(|_, t| !t.is_finished);
    }
    
    /// Проверяет, скачивается ли файл с данным ключом
    pub fn is_downloading(&self, key: &str) -> bool {
        self.tasks
            .values()
            .any(|t| !t.is_finished && t.kind.matches_key(key))
    }
    
    /// Обновляет прогресс задачи по ключу
    pub fn update_progress_by_key(&mut self, key: &str, progress: f32) {
        for task in self.tasks.values_mut() {
            if !task.is_finished && task.kind.matches_key(key) {
                task.progress = progress.clamp(0.0, 1.0);
                break;
            }
        }
    }
    
    /// Завершает задачу по ключу
    pub fn finish_by_key(&mut self, key: &str) {
        for task in self.tasks.values_mut() {
            if !task.is_finished && task.kind.matches_key(key) {
                task.is_finished = true;
                task.progress = 1.0;
                break;
            }
        }
    }
    
    /// Помечает задачу как failed по ключу
    pub fn fail_by_key(&mut self, key: &str, error: String) {
        for task in self.tasks.values_mut() {
            if !task.is_finished && task.kind.matches_key(key) {
                task.is_finished = true;
                task.error = Some(error);
                break;
            }
        }
    }
}
