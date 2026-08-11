use crate::proto::data_provider::DataProduct;

/// Миссии, по которым сужается запрос. Пустая строка — не сужать; имена
/// буквальные, как их знает каталог, и уходят в запрос без перевода.
pub const MISSIONS: [(&str, &str); 4] = [
    ("", "Все"),
    ("SENTINEL-1", "Sentinel-1"),
    ("SENTINEL-2", "Sentinel-2"),
    ("SENTINEL-3", "Sentinel-3"),
];

pub struct SearchState {
    /// Запрос к провайдеру — не то же, что фильтр по имени в `listing`: тот
    /// отбирает уже найденное, этот решает, что искать.
    pub query: String,
    /// Миссия из [`MISSIONS`]; пусто — искать по всем.
    pub mission: String,
    pub results: Vec<DataProduct>,
    /// Последняя ошибка search от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
    /// Ожидание ответа на data-provider/on_search: актуален только последний —
    /// результат по прошлому запросу под нынешним запросом ввёл бы в заблуждение.
    pub request: veldsdk::Latest,
    /// Как показывать найденное — отбор, порядок, страница.
    pub listing: super::listing::ListingState,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            mission: String::new(),
            results: Vec::new(),
            error: None,
            request: veldsdk::Latest::new(),
            listing: Default::default(),
        }
    }
}
