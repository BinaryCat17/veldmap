use crate::proto::data_provider::DataProduct;

pub struct SearchState {
    /// Запрос к провайдеру — не то же, что фильтр по имени в `listing`: тот
    /// отбирает уже найденное, этот решает, что искать.
    pub query: String,
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
            results: Vec::new(),
            error: None,
            request: veldsdk::Latest::new(),
            listing: Default::default(),
        }
    }
}
