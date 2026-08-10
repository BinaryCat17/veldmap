//! styles.rs — внешний вид приложения: цвета и готовые виджеты по их роли.
//!
//! Роль, а не оформление: разметка просит «вкладку» или «строку списка», а как
//! они выглядят, знает только этот файл.

use crate::proto::ui_service::{ButtonStyle, WidgetStyle, Color, Border, Background, Button, Padding, Length, button, container, icon};

// --- Colors ---
pub const COLOR_TEXT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
pub const COLOR_TEXT_DIM: Color = Color { r: 0.6, g: 0.6, b: 0.7, a: 1.0 };
pub const COLOR_BG_HOVER: Color = Color { r: 0.25, g: 0.25, b: 0.35, a: 1.0 };
pub const COLOR_PRIMARY: Color = Color { r: 0.1, g: 0.4, b: 0.7, a: 1.0 };
pub const COLOR_PRIMARY_HOVER: Color = Color { r: 0.15, g: 0.5, b: 0.8, a: 1.0 };
pub const COLOR_SUCCESS: Color = Color { r: 0.3, g: 0.8, b: 0.3, a: 1.0 };
pub const COLOR_WARNING: Color = Color { r: 1.0, g: 0.7, b: 0.3, a: 1.0 };
pub const COLOR_RELOAD: Color = Color { r: 0.5, g: 0.5, b: 0.7, a: 1.0 };
pub const COLOR_DANGER: Color = Color { r: 0.85, g: 0.3, b: 0.3, a: 1.0 };
pub const COLOR_BORDER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.12 };
pub const COLOR_BORDER_HOVER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.25 };

// --- Сборка стиля ---
//
// Все кнопки приложения устроены одинаково: фон, цвет текста, рамка — и
// отличаются только цветами. Поэтому состояние собирается одним `face`, а
// сама кнопка — одним `button_style`: иначе на каждую роль приходится своя
// копия одного и того же литерала, и они расходятся по мелочам.

/// Скругление углов — одно на все кнопки приложения.
const RADIUS: f32 = 4.0;

/// Вид виджета в одном состоянии.
fn face(background: Color, text_color: Color, border: Border) -> WidgetStyle {
    WidgetStyle {
        background: Some(Background::Color(background)),
        text_color: Some(text_color),
        border,
        ..Default::default()
    }
}

/// То же без рамки — только фон и текст.
fn plain(background: Color, text_color: Color) -> WidgetStyle {
    face(background, text_color, Border::with_radius(RADIUS))
}

/// Рамка в один пиксель — ей отличается карточка файла.
fn outline(color: Color) -> Border {
    Border { color, width: 1.0, radius: RADIUS }
}

/// Стиль кнопки из её состояний. `pressed` намеренно повторяет `active`:
/// нажатие видно по тому, что оно делает, а мигание фоном под курсором только
/// спорит с hover.
fn button_style(active: WidgetStyle, hovered: WidgetStyle, disabled: WidgetStyle) -> ButtonStyle {
    ButtonStyle { active: active.clone(), hovered, pressed: active, disabled }
}

/// Кнопка, у которой нерабочего состояния не бывает: без `on_press` её просто
/// не рисуют (см. view). Единственное исключение — карточка файла.
fn always_enabled(active: WidgetStyle, hovered: WidgetStyle) -> ButtonStyle {
    button_style(active, hovered, WidgetStyle::default())
}

// --- Роли ---

pub fn apply_nav<M>(btn: Button<M>) -> Button<M> {
    let style = always_enabled(
        plain(Color::from_rgb(0.15, 0.15, 0.18), Color::from_rgb(0.7, 0.7, 0.7)),
        plain(Color::from_rgb(0.25, 0.25, 0.3), Color::WHITE),
    );
    btn.style(style).padding(Padding { top: 8.0, bottom: 8.0, left: 15.0, right: 15.0 })
}

/// Вкладка. Активная светлее фона окна и с полным контрастом текста —
/// показанное ниже принадлежит именно ей; остальные приглушены. Разница
/// именно в фоне, а не в рамке: рамка на одной вкладке из ряда читается как
/// выделение чего угодно, а поднятый фон — как «этот слой сейчас сверху».
pub fn apply_tab<M>(btn: Button<M>, active: bool) -> Button<M> {
    let (background, text_color) = if active {
        (Color::from_rgb(0.16, 0.16, 0.20), COLOR_TEXT)
    } else {
        (Color::from_rgb(0.09, 0.09, 0.11), COLOR_TEXT_DIM)
    };
    let style = always_enabled(
        plain(background, text_color),
        plain(COLOR_BG_HOVER, COLOR_TEXT),
    );
    btn.style(style).padding(Padding { top: 6.0, bottom: 6.0, left: 10.0, right: 10.0 })
}

pub fn apply_primary<M>(btn: Button<M>) -> Button<M> {
    let style = always_enabled(
        plain(COLOR_PRIMARY, Color::WHITE),
        plain(COLOR_PRIMARY_HOVER, Color::WHITE),
    );
    btn.style(style).padding(Padding { top: 6.0, bottom: 6.0, left: 12.0, right: 12.0 })
}

/// Строка списка файлов: одна и та же кнопка с лёгкой рамкой и для кликабельных
/// (папка, готовый файл — есть `on_press`), и для некликабельных (недокачанный
/// или ещё не скачанный — карточка без действия). Один виджет на оба случая
/// держит левую границу общей.
///
/// `disabled` — тот же бордер, приглушённый и без реакции на hover: без
/// `on_press` хост отдаёт только его, hover/pressed не приходят.
pub fn apply_file<M>(btn: Button<M>) -> Button<M> {
    let card = Color::from_rgba(1.0, 1.0, 1.0, 0.02);
    let style = button_style(
        face(card, Color::WHITE, outline(COLOR_BORDER)),
        face(COLOR_BG_HOVER, Color::WHITE, outline(COLOR_BORDER_HOVER)),
        face(card, COLOR_TEXT_DIM, outline(COLOR_BORDER)),
    );
    btn.style(style).padding(5.0)
}

pub fn apply_search_input<M>(input: crate::proto::ui_service::TextInput<M>) -> crate::proto::ui_service::TextInput<M> {
    input.padding(10.0).size(16.0)
}

/// Кнопка-иконка: фон — тот же цвет, что и глиф, но почти прозрачный.
pub fn apply_icon<M>(btn: Button<M>, color: Color) -> Button<M> {
    let tint = |alpha| Color::from_rgba(color.r, color.g, color.b, alpha);
    let style = always_enabled(plain(tint(0.1), color), plain(tint(0.2), color));
    btn.style(style).padding(6.0)
}

/// Сторона квадратной кнопки-иконки (play/pause/reload/eye/trash) в списке
/// файлов — одна и та же для всех, независимо от конкретного глифа.
const ICON_BUTTON_SIZE: f32 = 32.0;

/// Кнопка-иконка фиксированного размера с глифом по центру. Без фиксации
/// размер кнопки — это размер её текстового содержимого плюс паддинг, а у
/// разных глифов Font Awesome разная метрика ширины (например, "play"
/// заметно уже "pause" или "trash") — визуально кнопки в одной строке
/// получались бы разного размера. Глиф оборачивается в `container` с
/// `center_x/center_y`, а не кладётся в кнопку как есть — иначе при фикс.
/// размере кнопки, большем чем сам глиф, он прижимался бы к левому краю
/// паддинга вместо центра.
pub fn icon_button<M>(glyph: impl Into<String>, color: Color) -> Button<M> {
    let centered = container(icon(glyph))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y();
    apply_icon(button(centered), color)
        .width(Length::Fixed(ICON_BUTTON_SIZE))
        .height(Length::Fixed(ICON_BUTTON_SIZE))
}
