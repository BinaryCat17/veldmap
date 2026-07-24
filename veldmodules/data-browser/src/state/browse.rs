pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    pub is_loading: bool,
    /// Последняя ошибка list_path от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
    /// Ожидание ответа на data-provider/on_list_path — гасит устаревший/чужой ответ.
    pub pending: veldsdk::Correlator<()>,
}

pub struct BrowseItem {
    pub s3_key: String,
    pub name: String,
    pub is_folder: bool,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            is_loading: false,
            error: None,
            pending: veldsdk::Correlator::new(),
        }
    }
}
