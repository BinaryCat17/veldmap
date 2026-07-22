use crate::proto::dataprovider::DataProduct;

pub struct SearchState {
    pub query: String,
    pub results: Vec<DataProduct>,
    pub is_loading: bool,
    /// Последняя ошибка search от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            is_loading: false,
            error: None,
        }
    }
}
