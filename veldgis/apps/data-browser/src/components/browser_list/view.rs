//! components/browser_list/view.rs — рендеринг списка файлов

use veld_ui::{column, row, text, button, Element, Length, Alignment};
use crate::common::BrowserItem;
use crate::components::task_manager::TaskManager;

pub fn render_item<M: Clone + serde::Serialize>(
    item: &BrowserItem,
    is_downloading: bool,
    on_browse: impl Fn(String) -> M,
    on_view: impl Fn(String) -> M,
    on_download: impl Fn(String) -> M,
) -> Element<M> {
    let icon = if item.is_folder { "📁" } else { "📄" };
    let title = format!("{} {}", icon, item.name);
    
    let main_button: Element<M> = if item.is_folder {
        button(text(title))
            .on_press(on_browse(item.s3_key.clone()))
            .into()
    } else if item.exists_locally {
        button(text(title))
            .on_press(on_view(item.s3_key.clone()))
            .into()
    } else {
        text(title).into()
    };
    
    let status: Element<M> = if is_downloading {
        text("⏳").into()
    } else if item.exists_locally {
        row![
            text("✓"),
            button(text("👁")).on_press(on_view(item.s3_key.clone())),
            button(text("🔄")).on_press(on_download(item.s3_key.clone()))
        ]
        .spacing(5.0)
        .into()
    } else if !item.is_folder {
        button(text("⬇"))
            .on_press(on_download(item.s3_key.clone()))
            .into()
    } else {
        text("").into()
    };
    
    row![main_button, status]
        .width(Length::Fill)
        .spacing(10.0)
        .align_items(Alignment::Center)
        .into()
}

pub fn render_list<M: Clone + serde::Serialize>(
    items: &[BrowserItem],
    task_manager: &TaskManager,
    _path_prefix: &str,
    on_browse: impl Fn(String) -> M + Clone,
    on_view: impl Fn(String) -> M + Clone,
    on_download: impl Fn(String) -> M + Clone,
) -> Element<M> {
    column(items.iter().map(|item| {
        let is_downloading = task_manager.is_downloading(&item.s3_key);
        render_item(
            item,
            is_downloading,
            on_browse.clone(),
            on_view.clone(),
            on_download.clone(),
        )
    }))
    .width(Length::Fill)
    .spacing(8.0)
    .into()
}
