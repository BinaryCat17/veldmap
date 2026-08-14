//! Стили и геометрия виджетов: удобные Rust-типы поверх proto-сообщений.
//! Конвертация в proto — pub(crate) to_proto, используется билдерами widgets.

use crate::proto;

/// Логические имена шрифтов ui-service — строковый контракт между ним и
/// клиентами разметки. Регистрация файлов шрифтов — в ui-service (state.rs),
/// здесь — единственное место имён для клиентов; литералы не разносить.
///
/// `UI` — шрифт подписей и заголовков, он же дефолтный; `MONO` — там, где
/// важна одинаковая ширина знака: имена файлов, размеры, пути; `ICONS` — глифы.
pub const FONT_UI: &str = "UI";
pub const FONT_MONO: &str = "Mono";
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
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self { Self { r, g, b, a: 1.0 } }
    pub fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self { Self { r, g, b, a } }
    /// Цвет так, как его называют в макете: `#2E2A22` → `rgb8(0x2E, 0x2A, 0x22)`.
    /// Числа остаются в sRGB — в линейное их переводит рендерер (см. `Vertex`).
    pub const fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Self { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }
    /// Тот же цвет полупрозрачным — для теней и подсветок.
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
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

/// Тень под виджетом. Нулевая — её нет.
#[derive(Clone, Copy, Default)]
pub struct Shadow {
    pub color: Color,
    pub offset: (f32, f32),
    pub blur: f32,
}

impl Shadow {
    pub(crate) fn to_proto(self) -> proto::Shadow {
        proto::Shadow {
            color: Some(self.color.to_proto()),
            offset_x: self.offset.0,
            offset_y: self.offset.1,
            blur: self.blur,
        }
    }
}

#[derive(Clone, Default)]
pub struct WidgetStyle {
    pub background: Option<Background>,
    /// `None` — цвет текста берётся у темы (см. `WidgetStyle` в types.proto).
    pub text_color: Option<Color>,
    pub border: Border,
    pub shadow: Option<Shadow>,
}

impl WidgetStyle {
    pub(crate) fn to_proto(self) -> proto::WidgetStyle {
        proto::WidgetStyle {
            background: self.background.map(|b| b.to_proto()),
            text_color: self.text_color.map(|c| c.to_proto()),
            border: Some(self.border.to_proto()),
            shadow: self.shadow.map(|s| s.to_proto()),
        }
    }
}

/// Полоса прогресса: дорожка, заполненная часть, общая рамка.
#[derive(Clone, Copy, Default)]
pub struct ProgressBarStyle {
    pub track: Option<Background>,
    pub bar: Option<Background>,
    pub border: Border,
}

impl ProgressBarStyle {
    pub(crate) fn to_proto(self) -> proto::ProgressBarStyle {
        proto::ProgressBarStyle {
            track: self.track.map(|b| b.to_proto()),
            bar: self.bar.map(|b| b.to_proto()),
            border: Some(self.border.to_proto()),
        }
    }
}

/// Ползунок: дорожка до ручки, дорожка после и сама ручка — см. `SliderStyle`
/// в types.proto.
#[derive(Clone, Copy, Default)]
pub struct SliderStyle {
    pub filled: Option<Background>,
    pub rest: Option<Background>,
    pub rail_width: f32,
    pub rail_border: Border,
    pub handle: Option<Background>,
    pub handle_radius: f32,
    pub handle_border_width: f32,
    pub handle_border_color: Color,
}

impl SliderStyle {
    pub(crate) fn to_proto(self) -> proto::SliderStyle {
        proto::SliderStyle {
            filled: self.filled.map(|b| b.to_proto()),
            rest: self.rest.map(|b| b.to_proto()),
            rail_width: self.rail_width,
            rail_border: Some(self.rail_border.to_proto()),
            handle: self.handle.map(|b| b.to_proto()),
            handle_radius: self.handle_radius,
            handle_border_width: self.handle_border_width,
            handle_border_color: Some(self.handle_border_color.to_proto()),
        }
    }
}

/// Поле ввода в трёх состояниях — см. `TextInputStyle` в types.proto.
#[derive(Clone, Default)]
pub struct TextInputStyle {
    pub active: WidgetStyle,
    pub hovered: WidgetStyle,
    pub focused: WidgetStyle,
    pub placeholder: Color,
    pub selection: Color,
}

impl TextInputStyle {
    pub(crate) fn to_proto(self) -> proto::TextInputStyle {
        proto::TextInputStyle {
            active: Some(self.active.to_proto()),
            hovered: Some(self.hovered.to_proto()),
            focused: Some(self.focused.to_proto()),
            placeholder: Some(self.placeholder.to_proto()),
            selection: Some(self.selection.to_proto()),
        }
    }
}

/// Полоса прокрутки: геометрия и цвета.
#[derive(Clone, Copy, Default)]
pub struct Scrollbar {
    pub width: f32,
    pub scroller_width: f32,
    pub margin: f32,
    pub rail: Option<Background>,
    pub scroller: Option<Background>,
    pub radius: f32,
}

impl Scrollbar {
    pub(crate) fn to_proto(self) -> proto::Scrollbar {
        proto::Scrollbar {
            width: self.width,
            scroller_width: self.scroller_width,
            margin: self.margin,
            rail: self.rail.map(|b| b.to_proto()),
            scroller: self.scroller.map(|b| b.to_proto()),
            radius: self.radius,
        }
    }
}

// Отдельного типа под «стиль кнопки» здесь нет: состояния коробки называются
// по одному (`Container::hovered`/`pressed`/`disabled`), и незваное вырождается
// к покою. Набор из четырёх лиц, заполняемый целиком, заставлял бы повторять
// вид кнопки четырежды — и три копии из четырёх были бы одинаковыми.
