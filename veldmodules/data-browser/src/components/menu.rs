//! components/menu.rs — выпадающая панель: список действий или значений.
//!
//! Одна на все меню приложения: «плюс» в полосе вкладок, чипы отбора и меню
//! строки. Различаются они только пунктами, а пункт устроен одинаково —
//! пометка слева, подпись, счётчик справа.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, icon, text, Alignment, Element, Length, Padding,
};
use crate::module::{theme, Msg};

/// Ширина панели, пока подписи в неё влезают. Одна на все меню: разъезжающаяся
/// по ширине панель читается как разные меню, хотя это одно и то же место
/// интерфейса.
const WIDTH: f32 = 214.0;

/// Постоянное в строке пункта: место под пометку, зазоры и поля панели.
/// Считается по тем же числам, которыми строка и собрана ([`line`],
/// [`panel`]) — вторая их запись разошлась бы с первой ровно на ту долю, на
/// которую подпись потом обрезается.
const ITEM_OVERHEAD: f32 = 12.0 + 8.0 * 2.0 + 4.0 * 2.0 + theme::GUTTER;

/// Галочка выбранного значения.

pub struct Item {
    label: String,
    message: Msg,
    /// Пометка слева: галочка у выбранного значения или свой глиф у действия.
    mark: Option<&'static str>,
    /// Счётчик справа — сколько записей под этим значением.
    count: Option<usize>,
    selected: bool,
    danger: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, message: Msg) -> Self {
        Self { label: label.into(), message, mark: None, count: None, selected: false, danger: false }
    }

    /// Выбранное значение: галочка и подсветка.
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

    /// Чем пункт подписан. Спрашивают об этом тесты состава меню: набор
    /// пунктов — это и есть то, что строка умеет.
    pub fn named(&self) -> &str {
        &self.label
    }

    pub fn glyph(mut self, glyph: &'static str) -> Self {
        self.mark = Some(glyph);
        self
    }

    /// Необратимое действие — единственное, что подписано цветом.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// Панель из готовых пунктов.
pub fn panel(items: Vec<Item>) -> Element<Msg> {
    // Обрезанный пункт меню не называет действия, а меню только затем и
    // открывают. Поэтому панель расширяется под самую длинную подпись —
    // ужиматься ей не во что, места она не занимает (рисуется оверлеем).
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
    // Место под пометку и счётчик остаётся, даже когда их нет: без этого
    // подписи соседних пунктов начинались бы в разных местах.
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
        container(text::<Msg>(item.label).size(theme::TEXT_BODY).single_line())
            .width(Length::Fill),
        count,
    ]
    .spacing(8.0)
    .width(Length::Fill)
    .align_items(Alignment::Center);

    theme::menu_item(content, item.selected, item.danger)
        .width(Length::Fill)
        .on_press(item.message)
        .into()
}
