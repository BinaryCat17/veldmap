#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    Search,
    Browse,
    Downloaded,
    Preview,
}

/// Общее для всех экранов. Состояние скачанного сюда НЕ входит: оно приходит
/// от data-library целиком (см. `LibraryState`) — всё, из чего выводится
/// строка списка, лежит в одном месте и приходит из одного источника.
pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
}
