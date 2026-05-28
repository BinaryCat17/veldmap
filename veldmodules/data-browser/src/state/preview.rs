pub struct PreviewState {
    pub current_image: Option<u64>,
    pub current_path: String,
    pub is_loading: bool,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            current_image: None,
            current_path: String::new(),
            is_loading: false,
        }
    }
}
