pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    /// Как показывать этот список — отбор, порядок, страница.
    pub listing: super::listing::ListingState,
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
    /// Снимок, внутри которого лежит запись; пусто — она в пути к снимкам.
    /// Совпал с `identifier` без завершающего слэша — запись и есть снимок.
    /// Сказал провайдер: вывести границу снимка из ключа здесь не из чего
    /// (см. `ListEntry.product`).
    pub product: String,
    /// Размер объекта в байтах; 0 — папка или размер неизвестен.
    pub size: u64,
    /// Время объекта, unix-секунды; 0 — неизвестно.
    pub modified: i64,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            listing: Default::default(),
            error: None,
            request: veldsdk::Latest::new(),
        }
    }
}
