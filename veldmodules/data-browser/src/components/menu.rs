use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, icon, text, Alignment, Element, Length, Padding,
};
use crate::module::{theme, Msg};

const WIDTH: f32 = 214.0;

const ITEM_OVERHEAD: f32 = 12.0 + 8.0 * 2.0 + 4.0 * 2.0 + theme::GUTTER;

pub struct Item {
    label: String,
    message: Option<Msg>,
    mark: Option<&'static str>,
    count: Option<usize>,
    selected: bool,
    danger: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, message: Msg) -> Self {
        Self { label: label.into(), message: Some(message), mark: None, count: None, selected: false, danger: false }
    }

    pub fn note(label: impl Into<String>) -> Self {
        Self { label: label.into(), message: None, mark: None, count: None, selected: false, danger: false }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        if selected {
            self.mark = Some(theme::glyph::TICK);
        }
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn named(&self) -> &str {
        &self.label
    }

    pub fn marked(&self) -> bool {
        self.selected
    }

    pub fn glyph(mut self, glyph: &'static str) -> Self {
        self.mark = Some(glyph);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

pub fn panel(items: Vec<Item>) -> Element<Msg> {
    let widest = items
        .iter()
        .map(|item| super::format::text_width(&item.label, theme::TEXT_BODY) + ITEM_OVERHEAD)
        .fold(WIDTH, f32::max);
    container(
        column(items.into_iter().map(line))
            .width(Length::Fill)
            .spacing(1.0),
    )
    .style(theme::panel())
    .width(Length::Fixed(widest))
    .padding(Padding::new(4.0))
    .into()
}

fn line(item: Item) -> Element<Msg> {
    let mark: Element<Msg> = match item.mark {
        Some(glyph) => icon::<Msg>(glyph).size(9.0).color(theme::ACCENT).into(),
        None => theme::nothing(),
    };
    let count: Element<Msg> = match item.count {
        Some(count) => text::<Msg>(count.to_string()).size(theme::TEXT_SMALL).color(theme::INK_DIM).single_line().into(),
        None => theme::nothing(),
    };

    let content = row![
        container(mark).width(Length::Fixed(12.0)),
        container(
            text::<Msg>(item.label)
                .size(theme::TEXT_BODY)
                .color(if item.message.is_some() { theme::INK } else { theme::INK_DIM })
                .single_line()
        )
        .width(Length::Fill),
        count,
    ]
    .spacing(8.0)
    .width(Length::Fill)
    .align_items(Alignment::Center);

    match item.message {
        Some(message) => theme::menu_item(content, item.selected, item.danger)
            .width(Length::Fill)
            .on_press(message)
            .into(),
        None => container(content)
            .padding(Padding { top: 6.0, bottom: 6.0, left: 9.0, right: 9.0 })
            .width(Length::Fill)
            .into(),
    }
}
