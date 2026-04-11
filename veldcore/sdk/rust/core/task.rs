use std::fmt::Debug;
use serde::{Serialize, Deserialize};
use std::future::Future;
use futures_util::stream::{StreamExt, once};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskUpdate<T> {
    Started(Option<String>),
    Progress(f32, Option<String>),
    Finished(Result<T, String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TaskStatus<T> {
    #[default]
    Idle,
    Running { progress: f32, task_id: Option<String> },
    Finished(T),
    Failed(String),
}

impl<T> TaskStatus<T> {
    pub fn is_running(&self) -> bool {
        matches!(self, TaskStatus::Running { .. })
    }
    
    pub fn progress(&self) -> f32 {
        if let TaskStatus::Running { progress, .. } = self { *progress } else { 0.0 }
    }

    pub fn result(&self) -> Option<&T> {
        if let TaskStatus::Finished(res) = self { Some(res) } else { None }
    }

    pub fn error(&self) -> Option<&String> {
        if let TaskStatus::Failed(e) = self { Some(e) } else { None }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, TaskStatus::Failed(_))
    }

    pub fn handle(&mut self, update: TaskUpdate<T>) {
        match update {
            TaskUpdate::Started(id) => {
                *self = TaskStatus::Running { progress: 0.0, task_id: id };
            }
            TaskUpdate::Progress(p, id) => {
                if let TaskStatus::Running { progress, task_id } = self {
                    *progress = p;
                    if id.is_some() { *task_id = id; }
                }
            }
            TaskUpdate::Finished(Ok(res)) => {
                *self = TaskStatus::Finished(res);
            }
            TaskUpdate::Finished(Err(e)) => {
                *self = TaskStatus::Failed(e);
            }
        }
    }
}

/// Создает команду, которая выполняет Future и возвращает поток обновлений задачи.
pub fn spawn<F, T, M>(
    future: F, 
    msg_wrap: impl Fn(TaskUpdate<T>) -> M + Send + Sync + 'static
) -> crate::core::Command<M>
where
    F: Future<Output = Result<T, String>> + Send + Sync + 'static,
    T: Send + Sync + 'static,
    M: 'static,
{
    let msg_wrap = Arc::new(msg_wrap);
    let msg_wrap_c = Arc::clone(&msg_wrap);
    
    let s = once(async move { msg_wrap_c(TaskUpdate::Started(None)) })
        .chain(once(async move {
            match future.await {
                Ok(res) => msg_wrap(TaskUpdate::Finished(Ok(res))),
                Err(e) => msg_wrap(TaskUpdate::Finished(Err(e))),
            }
        }));
    crate::core::Command::stream(s)
}

/// Упрощенная версия spawn без трансформации (для внутреннего использования)
pub fn spawn_raw<F, T>(
    future: F
) -> crate::core::Command<TaskUpdate<T>>
where
    F: Future<Output = Result<T, String>> + Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    spawn(future, |u| u)
}
