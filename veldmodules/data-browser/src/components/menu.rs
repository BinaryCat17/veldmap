//! components/menu.rs — drop-down panel: a list of actions or values.
//!
//! One panel for all application menus: the "plus" in the tab bar, the filter
//! chips, and the row menu. They differ only in their items, and an item is
//! built the same way — mark on the left, label, count on the right.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, icon, text, Alignment, Element, Length, Padding,
};
use crate::module::{theme, Msg};

/// Panel width while the labels fit into it. One for all menus: a panel that
/// stretches with its width reads as different menus, even though it is the
/// same place in the interface.
const WIDTH: f32 = 214.0;

/// Constant part of an item row: room for the mark, the gaps, and the panel
/// padding. Computed from the same numbers the row itself is built from
/// ([`line`], [`panel`]) — a second record of them would diverge from the
/// first by exactly the amount the label is later truncated to.
const ITEM_OVERHEAD: f32 = 12.0 + 8.0 * 2.0 + 4.0 * 2.0 + theme::GUTTER;

pub struct Item {
    label: String,
    /// What the item sends on click; empty — a note item, it is not clicked.
    message: Option<Msg>,
    /// Mark on the left: a tick by the selected value or its own glyph by an action.
    mark: Option<&'static str>,
    /// Count on the right — how many records carry this value.
    count: Option<usize>,
    selected: bool,
    danger: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, message: Msg) -> Self {
        Self { label: label.into(), message: Some(message), mark: None, count: None, selected: false, danger: false }
    }

    /// A note in the panel — "and 14 more": neither an action nor a value,
    /// there is nothing to click, and it must not look like an item.
    pub fn note(label: impl Into<String>) -> Self {
        Self { label: label.into(), message: None, mark: None, count: None, selected: false, danger: false }
    }

    /// The selected value: a tick and a highlight.
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

    /// The label the item is signed with. The set of items is exactly what a
    /// row can do.
    pub fn named(&self) -> &str {
        &self.label
    }

    /// Whether the item is marked as the selected value.
    pub fn marked(&self) -> bool {
        self.selected
    }

    pub fn glyph(mut self, glyph: &'static str) -> Self {
        self.mark = Some(glyph);
        self
    }

    /// An irreversible action — the only thing signed with color.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// A panel of ready items.
pub fn panel(items: Vec<Item>) -> Element<Msg> {
    // A truncated menu item does not name the action, and menus are opened
    // precisely to name one. So the panel widens to the longest label —
    // it has nothing to shrink to, and it takes no space (drawn as an overlay).
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
    // Room for the mark and the count stays even when they are absent:
    // otherwise neighboring item labels would start at different positions.
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
        // With the same padding as an item — a row in a line; without
        // highlight or click — not an item.
        None => container(content)
            .padding(Padding { top: 6.0, bottom: 6.0, left: 9.0, right: 9.0 })
            .width(Length::Fill)
            .into(),
    }
}
