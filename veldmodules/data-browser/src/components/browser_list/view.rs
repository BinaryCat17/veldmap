//! components/browser_list/view.rs — рендеринг списка файлов

use veld_ui_service_wrap::{column, row};
use crate::proto::ui::{text, button, Element, Length, Alignment};
use crate::module::components::browser_list::BrowserItem;
use crate::module::components::task_manager::TaskManager;
use crate::module::styles;

/// Какие входные методы модуля вызывают кнопки элемента списка.
/// `None` — действие недоступно на этом экране. Значением события всегда
/// идёт `s3_key` элемента.
#[derive(Clone, Copy, Default)]
pub struct ItemActions<'a> {
    pub browse: Option<&'a str>,
    pub view: Option<&'a str>,
    pub download: Option<&'a str>,
}

pub fn render_item(item: &BrowserItem, is_downloading: bool, actions: ItemActions) -> Element<()> {
    // Иконка и имя — раздельные Text: у иконки свой шрифт (Icons), имя файла
    // рисуется дефолтным — единая строка ломала бы одно из двух.
    let icon_glyph = if item.is_folder { "\u{f07b}" } else { "\u{f016}" };
    let title: Element<()> = row![
        text(icon_glyph).font_family("Icons"),
        text(item.name.clone()),
    ].spacing(6.0).align_items(Alignment::Center).into();

    // Fill-ширина главной кнопки прижимает статус к правому краю строки
    let main_button: Element<()> = if item.is_folder {
        match actions.browse {
            Some(method) => styles::apply_file(button(title))
                .width(Length::Fill)
                .on_press_with(method, item.s3_key.clone())
                .into(),
            None => title,
        }
    } else if item.exists_locally {
        match actions.view {
            Some(method) => styles::apply_file(button(title))
                .width(Length::Fill)
                .on_press_with(method, item.s3_key.clone())
                .into(),
            None => title,
        }
    } else {
        title
    };

    let status: Element<()> = if is_downloading {
        text("\u{f254}").font_family("Icons").into()
    } else if item.exists_locally {
        let mut r = row![text("\u{f00c}").font_family("Icons")];
        if let Some(method) = actions.view {
            r = r.push(styles::apply_icon(button(text("\u{f06e}").font_family("Icons")), styles::COLOR_PRIMARY).on_press_with(method, item.s3_key.clone()));
        }
        if let Some(method) = actions.download {
            r = r.push(styles::apply_icon(button(text("\u{f021}").font_family("Icons")), styles::COLOR_RELOAD).on_press_with(method, item.s3_key.clone()));
        }
        r.spacing(5.0).align_items(Alignment::Center).into()
    } else if !item.is_folder {
        match actions.download {
            Some(method) => styles::apply_icon(button(text("\u{f019}").font_family("Icons")), styles::COLOR_PRIMARY).on_press_with(method, item.s3_key.clone()).into(),
            None => text("").into(),
        }
    } else {
        text("").into()
    };

    row![main_button, status]
        .width(Length::Fill)
        .spacing(10.0)
        .align_items(Alignment::Center)
        .into()
}

pub fn render_list(items: &[BrowserItem], task_manager: &TaskManager, actions: ItemActions) -> Element<()> {
    column(items.iter().map(|item| {
        let is_downloading = task_manager.is_downloading(&item.s3_key);
        render_item(item, is_downloading, actions)
    }))
    .width(Length::Fill)
    .spacing(8.0)
    .into()
}
