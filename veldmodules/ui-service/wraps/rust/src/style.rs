//! Стили и геометрия виджетов: удобные Rust-типы поверх proto-сообщений.
//! Конвертация в proto — pub(crate) to_proto, используется билдерами widgets.

use crate::proto;

/// Логические имена шрифтов ui-service — строковый контракт между ним и
/// клиентами разметки. Регистрация файлов шрифтов — в ui-service (state.rs),
/// здесь — единственное место имён для клиентов; литералы не разносить.
pub const FONT_DEFAULT: &str = "JetBrains Mono";
pub const FONT_ICONS: &str = "Icons";

#[derive(Clone, Copy, Default)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Padding {
    pub const ZERO: Padding = Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };
    pub fn new(p: f32) -> Self { Self { top: p, right: p, bottom: p, left: p } }
    pub fn to_proto(self) -> proto::Padding { proto::Padding { top: self.top, right: self.right, bottom: self.bottom, left: self.left } }
}

impl From<f32> for Padding {
    fn from(p: f32) -> Self { Padding::new(p) }
}

#[derive(Clone, Copy)]
pub enum Length {
    Fill,
    Shrink,
    Fixed(f32),
    FillPortion(u16),
}

impl Length {
    pub(crate) fn to_proto(self) -> proto::Length {
        match self {
            Length::Fill => proto::Length { value: Some(proto::length::Value::Fill(true)) },
            Length::Shrink => proto::Length { value: Some(proto::length::Value::Shrink(true)) },
            Length::Fixed(f) => proto::Length { value: Some(proto::length::Value::Fixed(f)) },
            Length::FillPortion(p) => proto::Length { value: Some(proto::length::Value::Portion(p as f32)) },
        }
    }
}

#[derive(Clone, Copy)]
pub enum Alignment {
    Start = 0,
    Center = 1,
    End = 2,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Default)]
pub struct Color {
    pub r: f32, pub g: f32, pub b: f32, pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self { Self { r, g, b, a: 1.0 } }
    pub fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self { Self { r, g, b, a } }
    pub(crate) fn to_proto(self) -> proto::Color { proto::Color { r: self.r, g: self.g, b: self.b, a: self.a } }
}

#[derive(Clone, Copy, Default)]
pub struct Border {
    pub color: Color,
    pub width: f32,
    /// Скругление одно на все четыре угла — см. `Border` в types.proto.
    pub radius: f32,
}

impl Border {
    pub(crate) fn to_proto(self) -> proto::Border {
        proto::Border {
            color: Some(self.color.to_proto()),
            width: self.width,
            radius: self.radius,
        }
    }
    /// Рамка без самой рамки: одно скругление. Так задаётся форма виджета,
    /// у которого есть фон, но нет обводки.
    pub fn with_radius(radius: f32) -> Self {
        Self { radius, ..Default::default() }
    }
}

#[derive(Clone, Copy)]
pub enum Background {
    Color(Color),
}

impl Background {
    pub(crate) fn to_proto(self) -> proto::Background {
        match self {
            Background::Color(c) => proto::Background {
                r#type: Some(proto::background::Type::Color(c.to_proto())),
            }
        }
    }
}

impl From<Color> for Background {
    fn from(c: Color) -> Self { Background::Color(c) }
}

#[derive(Clone, Default)]
pub struct WidgetStyle {
    pub background: Option<Background>,
    /// `None` — цвет текста берётся у темы (см. `WidgetStyle` в types.proto).
    pub text_color: Option<Color>,
    pub border: Border,
}

impl WidgetStyle {
    pub(crate) fn to_proto(self) -> proto::WidgetStyle {
        proto::WidgetStyle {
            background: self.background.map(|b| b.to_proto()),
            text_color: self.text_color.map(|c| c.to_proto()),
            border: Some(self.border.to_proto()),
        }
    }
}

/// Кнопка во всех своих состояниях. `disabled` — та, у которой нет `on_press`:
/// hover и нажатие до неё не доходят (см. `Button` в types.proto).
#[derive(Clone, Default)]
pub struct ButtonStyle {
    pub active: WidgetStyle,
    pub hovered: WidgetStyle,
    pub pressed: WidgetStyle,
    pub disabled: WidgetStyle,
}

impl ButtonStyle {
    pub(crate) fn to_proto(self) -> proto::ButtonStyle {
        proto::ButtonStyle {
            active: Some(self.active.to_proto()),
            hovered: Some(self.hovered.to_proto()),
            pressed: Some(self.pressed.to_proto()),
            disabled: Some(self.disabled.to_proto()),
        }
    }
}
