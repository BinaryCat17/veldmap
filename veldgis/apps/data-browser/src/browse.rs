use veld_ui::{column, row, text, button, scrollable, container, Element};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, has_prev: bool, has_next: bool, downloading_key: Option<&str>) -> Element<Message> {
    // Подготовка элементов с учетом состояния загрузки
    let mut display_items = items.to_vec();
    for item in &mut display_items {
        if downloading_key == Some(&item.s3_key) {
            item.is_downloading = true;
        }
    }

    let list = crate::common::render_list(&display_items);

    let mut pagination = row![].spacing(10.0);
    if has_prev {
        pagination = pagination.push(crate::styles::apply_primary(button(text("\u{f060} Previous"))).on_press(Message::PrevPage));
    }
    if has_next {
        pagination = pagination.push(crate::styles::apply_primary(button(text("Next \u{f061}"))).on_press(Message::NextPage));
    }

    let header = row![
        crate::styles::apply_primary(button(text("\u{f062}"))).on_press(Message::BrowseUp),
        text(format!("Browsing /{}", path)).size(20.0),
    ].spacing(10.0).align_items(veld_ui::Alignment::Center);

    container(
        column![
            header,
            text(status).size(14.0),
            pagination,
            scrollable(list)
                .width(veld_ui::Length::Fill)
                .height(veld_ui::Length::Fill)
        ].width(veld_ui::Length::Fill).height(veld_ui::Length::Fill).spacing(10.0)
    )
    .width(veld_ui::Length::Fill)
    .height(veld_ui::Length::Fill)
    .into()
}
