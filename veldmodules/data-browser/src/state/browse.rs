pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    /// Последняя ошибка list_path от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
    /// Ожидание ответа на data-provider/on_list_path. Актуален только
    /// последний: два быстрых перехода по папкам дают два ответа, и содержимое
    /// первого под заголовком второго — враньё, а не устаревшие данные.
    pub request: veldsdk::Latest,
}

pub struct BrowseItem {
    /// Путь продукта вместе с именем бакета (`eodata/Sentinel-2/…`) — в этом
    /// виде его принимают все топики data-provider. Ключ объекта в бакете
    /// живёт под этим префиксом, но срезает его провайдер, а не мы.
    pub identifier: String,
    pub name: String,
    pub is_folder: bool,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            error: None,
            request: veldsdk::Latest::new(),
        }
    }
}
