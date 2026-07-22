//! Отправка layout модуля в ui-service (Elm-цикл view).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use veldsdk::prost::Message;

use super::widgets::Element;

/// Ship the module's current view to the renderer.
/// The layout is sent whole; unchanged layouts are skipped by content hash.
/// Change detection beyond that (what to redraw, when) is the renderer's job.
pub fn render(plugin_id: &str, root: Element<()>, last_hash: &mut u64) {
    let layout = crate::proto::Layout { root: Some(root.widget) };

    let encoded = layout.encode_to_vec();
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    let hash = hasher.finish();

    if hash == *last_hash {
        return;
    }
    *last_hash = hash;

    let request = crate::proto::SetViewRequest {
        plugin_id: plugin_id.to_string(),
        layout: Some(layout),
    };
    crate::inputs::set_view(&request);
}
