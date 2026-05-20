#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    Search,
    Browse,
    Downloaded,
    Preview,
}

pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
    // Задачи управляются через components/task_manager
    pub task_manager: crate::module::components::task_manager::TaskManager,
    pub has_rendered: bool,
}
