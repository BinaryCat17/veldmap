use crate::proto::image_view::ViewState;

pub struct Drag {
    pub last: (f32, f32),
}

#[derive(Default)]
pub struct PreviewState {
    pub label: String,
    pub entry: Option<String>,
    pub surface: Option<veldsdk::surface::Delegated>,
    pub request: veldsdk::Latest,
    pub error: Option<String>,
    pub view_state: Option<ViewState>,
    pub variable: String,
    pub variables: Vec<crate::proto::image_view::Variable>,
    pub replaced: bool,
    pub dragging: Option<Drag>,
}

impl PreviewState {
    pub fn failure(&self) -> Option<&str> {
        if let Some(error) = &self.error {
            return Some(error);
        }
        self.view_state
            .as_ref()
            .map(|state| state.error.as_str())
            .filter(|error| !error.is_empty())
    }

    pub fn begin(&mut self) -> String {
        self.request.begin()
    }

    pub fn confirmed_variable(&self) -> String {
        let shown = self.view_state.as_ref().and_then(|state| state.variable.as_ref());
        match shown {
            Some(shown) if shown.path == self.variable => self.variable.clone(),
            _ => String::new(),
        }
    }
}
