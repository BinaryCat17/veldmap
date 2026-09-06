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

pub struct Item {
    label: String,
    /// Что пункт шлёт нажатием; пусто — пункт-примечание, его не нажимают.
    message: Option<Msg>,
    /// Пометка слева: галочка у выбранного значения или свой глиф у действия.
    mark: Option<&'static str>,
    /// Счётчик справа — сколько записей под этим значением.
    count: Option<usize>,
    selected: bool,
    danger: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, message: Msg) -> Self {
        Self { label: label.into(), message: Some(message), mark: None, count: None, selected: false, danger: false }
    }

    /// Примечание в панели — «и ещё 14»: не действие и не значение, нажимать
    /// нечего, и выглядеть пунктом оно не должно.
    pub fn note(label: impl Into<String>) -> Self {
        Self { label: label.into(), message: None, mark: None, count: None, selected: false, danger: false }
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

    /// Отмечен ли пункт как выбранное значение — для тех же тестов.
    pub fn marked(&self) -> bool {
        self.selected
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
        // Теми же отступами, что у пункта, — строка в ряду; без подсветки и
        // нажатия — не пункт.
        None => container(content)
            .padding(Padding { top: 6.0, bottom: 6.0, left: 9.0, right: 9.0 })
            .width(Length::Fill)
            .into(),
    }
}
