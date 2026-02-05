use anyhow::Result;

pub fn is_previewable(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".jpeg") || n.ends_with(".tif") || n.ends_with(".tiff")
}