use crate::proto::data_provider::DataProduct;

pub struct SearchState {
    pub query: String,
    pub results: Vec<DataProduct>,
    pub is_loading: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            is_loading: false,
        }
    }
}
