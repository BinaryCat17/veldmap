use veld_ui::{column, row, text, button, scrollable, container, Element, Padding};
use crate::{AppMessage as Message, common::BrowserItem};

pub fn view(path: &str, items: &[BrowserItem], status: &str, has_prev: bool, has_next: bool, downloading_key: Option<&str>) -> Element<Message> {
    let list = column(items.iter().map(|item| {
        let is_downloading = downloading_key == Some(&item.s3_key);
        
        // Item Main Part (Folder is button, File is panel)
        let main_part: Element<Message> = if item.is_folder {
            crate::styles::apply_file(button(
                row![
                    text("\u{f07b}").color(veld_ui::Color::from_rgb(0.4, 0.6, 1.0)),
                    text(&item.name)
                ].spacing(10.0).align_items(veld_ui::Alignment::Center)
            ))
            .width(veld_ui::Length::Fill)
            .on_press(Message::BrowsePath(item.s3_key.clone()))
            .into()
        } else {
            container(
                row![
                    text("\u{f15b}").color(veld_ui::Color::from_rgb(0.7, 0.7, 0.7)),
                    text(&item.name)
                ].spacing(10.0).align_items(veld_ui::Alignment::Center)
            )
            .padding(5.0)
            .width(veld_ui::Length::Fill)
            .into()
        };

        // Status / Action Part
        let status_element: Element<Message> = if is_downloading {
            container(text("\u{f017}").color(veld_ui::Color::from_rgb(1.0, 0.7, 0.3)))
                .width(veld_ui::Length::Fixed(80.0))
                .align_x(veld_ui::Alignment::Center)
                .into()
        } else if item.exists_locally {
            container(row![
                text("\u{f00c}").color(veld_ui::Color::from_rgb(0.3, 0.8, 0.3)),
                crate::styles::apply_icon(button(text("\u{f021}")), veld_ui::Color::from_rgb(0.5, 0.5, 0.7))
                    .on_press(Message::DownloadFile(item.s3_key.clone()))
            ].spacing(10.0).align_items(veld_ui::Alignment::Center))
            .width(veld_ui::Length::Fixed(80.0))
            .align_x(veld_ui::Alignment::End)
            .into()
        } else if !item.is_folder {
            // File exists only on S3
            container(
                crate::styles::apply_icon(button(text("\u{f019}")), veld_ui::Color::from_rgb(0.3, 0.8, 0.3))
                    .on_press(Message::DownloadFile(item.s3_key.clone()))
            )
            .width(veld_ui::Length::Fixed(80.0))
            .align_x(veld_ui::Alignment::End)
            .into()
        } else {
            container(veld_ui::Space::with_width(80.0))
                .width(veld_ui::Length::Fixed(80.0))
                .into()
        };

        row![
            main_part,
            status_element
        ].width(veld_ui::Length::Fill).spacing(10.0).align_items(veld_ui::Alignment::Center).into()
    }))
    .width(veld_ui::Length::Fill)
    .spacing(8.0)
    .padding(Padding { right: 30.0, ..Default::default() });

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
