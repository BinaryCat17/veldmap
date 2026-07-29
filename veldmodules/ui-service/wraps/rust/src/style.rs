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
pub struct Radius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl Radius {
    pub fn new(r: f32) -> Self { Self { top_left: r, top_right: r, bottom_right: r, bottom_left: r } }
    pub(crate) fn to_proto(self) -> proto::Radius {
        proto::Radius {
            top_left: self.top_left,
            top_right: self.top_right,
            bottom_right: self.bottom_right,
            bottom_left: self.bottom_left,
        }
    }
}

impl From<f32> for Radius {
    fn from(r: f32) -> Self { Radius::new(r) }
}

#[derive(Clone, Copy, Default)]
pub struct Border {
    pub color: Color,
    pub width: f32,
    pub radius: Radius,
}

impl Border {
    pub(crate) fn to_proto(self) -> proto::Border {
        proto::Border {
            color: Some(self.color.to_proto()),
            width: self.width,
            radius: Some(self.radius.to_proto()),
        }
    }
    pub fn with_radius(radius: impl Into<Radius>) -> Self {
        Self { radius: radius.into(), ..Default::default() }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Shadow {
    pub color: Color,
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
}

impl Shadow {
    pub(crate) fn to_proto(self) -> proto::Shadow {
        proto::Shadow {
            color: Some(self.color.to_proto()),
            offset_x: self.offset_x,
            offset_y: self.offset_y,
            blur_radius: self.blur_radius,
        }
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
    pub text_color: Option<Color>,
    pub border: Border,
    pub shadow: Shadow,
}

impl WidgetStyle {
    pub(crate) fn to_proto(self) -> proto::WidgetStyle {
        proto::WidgetStyle {
            background: self.background.map(|b| b.to_proto()),
            text_color: self.text_color.map(|c| c.to_proto()),
            border: Some(self.border.to_proto()),
            shadow: Some(self.shadow.to_proto()),
        }
    }
}

pub struct ButtonStyle {
    pub active: WidgetStyle,
    pub hovered: WidgetStyle,
    pub pressed: WidgetStyle,
    pub disabled: WidgetStyle,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            active: WidgetStyle::default(),
            hovered: WidgetStyle::default(),
            pressed: WidgetStyle::default(),
            disabled: WidgetStyle::default(),
        }
    }
}

pub enum Style {
    Class(String),
    Custom(Box<ButtonStyle>),
}

impl From<&str> for Style {
    fn from(s: &str) -> Self { Style::Class(s.to_string()) }
}

impl From<ButtonStyle> for Style {
    fn from(s: ButtonStyle) -> Self { Style::Custom(Box::new(s)) }
}
