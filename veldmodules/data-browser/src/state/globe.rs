use veldsdk::surface::Delegated;

use crate::proto::data_provider::Ring;

#[derive(Default)]
pub struct GlobeState {
    pub surface: Option<Delegated>,
    pub dragging: Option<Drag>,
}

pub struct Drag {
    pub last: (f32, f32),
    pub travelled: f32,
}

pub struct Outlined {
    pub key: String,
    pub label: String,
    pub folder: bool,
    pub rings: Vec<Ring>,
}
