use super::proto;
pub use super::proto::*;

use serde::Serialize;
pub use futures_util::task::noop_waker_ref;

pub struct Element<M> {
    pub widget: proto::Widget,
    pub _marker: std::marker::PhantomData<M>,
}

impl<M> Element<M> {
    pub fn key(mut self, k: u64) -> Self {
        self.widget.key = k;
        self
    }
}

impl<M> From<proto::Widget> for Element<M> {
    fn from(widget: proto::Widget) -> Self { Self { widget, _marker: std::marker::PhantomData } }
}

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
    pub fn from_proto(p: proto::Padding) -> Self { Self { top: p.top, right: p.right, bottom: p.bottom, left: p.left } }
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
    fn to_proto(self) -> proto::Length {
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
    fn to_proto(self) -> proto::Color { proto::Color { r: self.r, g: self.g, b: self.b, a: self.a } }
}

// --- Style Definitions ---

#[derive(Clone, Copy, Default)]
pub struct Radius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl Radius {
    pub fn new(r: f32) -> Self { Self { top_left: r, top_right: r, bottom_right: r, bottom_left: r } }
    fn to_proto(self) -> proto::Radius {
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
    fn to_proto(self) -> proto::Border {
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
    fn to_proto(self) -> proto::Shadow {
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
    fn to_proto(self) -> proto::Background {
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
    fn to_proto(self) -> proto::WidgetStyle {
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

// --- Builder Structs ---

pub struct Column<M> {
    widget: proto::Column,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Column<M> {
    pub fn new() -> Self {
        Self { 
            widget: proto::Column {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            }, 
            key: 0,
            _marker: std::marker::PhantomData 
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn push(mut self, child: impl Into<Element<M>>) -> Self {
        let e: Element<M> = child.into();
        self.widget.children.push(e.widget);
        self
    }
    pub fn extend(mut self, children: impl IntoIterator<Item = Element<M>>) -> Self {
        for child in children { self.widget.children.push(child.widget); }
        self
    }
    pub fn spacing(mut self, s: f32) -> Self {
        self.widget.spacing = s;
        self
    }
    pub fn align_items(mut self, align: Alignment) -> Self {
        self.widget.align_items = align as i32;
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
    pub fn max_width(mut self, w: f32) -> Self {
        self.widget.max_width = Some(proto::Length { value: Some(proto::length::Value::Fixed(w)) });
        self
    }
    pub fn max_height(mut self, h: f32) -> Self {
        self.widget.max_height = Some(proto::Length { value: Some(proto::length::Value::Fixed(h)) });
        self
    }
}

impl<M> From<Column<M>> for Element<M> {
    fn from(c: Column<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Column(c.widget)), 
            key: c.key,
            ..Default::default() 
        }.into()
    }
}

pub fn column<M>(children: impl IntoIterator<Item = Element<M>>) -> Column<M> {
    Column::new().extend(children)
}

pub struct Row<M> {
    widget: proto::Row,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Row<M> {
    pub fn new() -> Self {
        Self { 
            widget: proto::Row {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            }, 
            key: 0,
            _marker: std::marker::PhantomData 
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn push(mut self, child: impl Into<Element<M>>) -> Self {
        let e: Element<M> = child.into();
        self.widget.children.push(e.widget);
        self
    }
    pub fn extend(mut self, children: impl IntoIterator<Item = Element<M>>) -> Self {
        for child in children { self.widget.children.push(child.widget); }
        self
    }
    pub fn spacing(mut self, s: f32) -> Self {
        self.widget.spacing = s;
        self
    }
    pub fn align_items(mut self, align: Alignment) -> Self {
        self.widget.align_items = align as i32;
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
    pub fn max_width(mut self, w: f32) -> Self {
        self.widget.max_width = Some(proto::Length { value: Some(proto::length::Value::Fixed(w)) });
        self
    }
    pub fn max_height(mut self, h: f32) -> Self {
        self.widget.max_height = Some(proto::Length { value: Some(proto::length::Value::Fixed(h)) });
        self
    }
}

impl<M> From<Row<M>> for Element<M> {
    fn from(r: Row<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Row(r.widget)), 
            key: r.key,
            ..Default::default() 
        }.into()
    }
}

pub fn row<M>(children: impl IntoIterator<Item = Element<M>>) -> Row<M> {
    Row::new().extend(children)
}

pub struct Text<M> {
    widget: proto::Text,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Text<M> {
    pub fn new(content: impl Into<String>) -> Self {
        Self { 
            widget: proto::Text {
                content: content.into(),
                size: 16.0,
                color: Some(proto::Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }),
                bold: false,
                horizontal_alignment: 0,
                vertical_alignment: 0,
                style: String::new(),
                width: None,
                height: None,
                shaping: 0,
                font_family: String::new(),
            }, 
            key: 0,
            _marker: std::marker::PhantomData 
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.widget.size = size;
        self
    }
    pub fn color(mut self, color: Color) -> Self {
        self.widget.color = Some(color.to_proto());
        self
    }
    pub fn style(mut self, style: impl Into<String>) -> Self {
        self.widget.style = style.into();
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
    pub fn horizontal_alignment(mut self, align: Alignment) -> Self {
        self.widget.horizontal_alignment = align as i32;
        self
    }
    pub fn vertical_alignment(mut self, align: Alignment) -> Self {
        self.widget.vertical_alignment = align as i32;
        self
    }
    pub fn shaping(mut self, advanced: bool) -> Self {
        self.widget.shaping = if advanced { 1 } else { 0 };
        self
    }
}

impl<M> From<Text<M>> for Element<M> {
    fn from(t: Text<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Text(t.widget)), 
            key: t.key,
            ..Default::default() 
        }.into()
    }
}

pub fn text<M>(content: impl Into<String>) -> Text<M> {
    Text::new(content)
}

pub struct Button<M> {
    widget: proto::Button,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Button<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self { widget: proto::Button {
            child: Some(Box::new(content.into().widget)),
            on_press: String::new(),
            disabled: false,
            width: None, height: None,
            style_variant: Some(proto::button::StyleVariant::StyleClass(String::new())),
            padding: None,
        }, key: 0, _marker: std::marker::PhantomData }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn on_press(mut self, msg: M) -> Self where M: Serialize {
        self.widget.on_press = serde_json::to_string(&msg).unwrap_or_default();
        self
    }
    /// Set the message tag for fire-and-forget event handling.
    /// This is a convenience method for string message tags.
    pub fn on_press_tag(mut self, tag: impl Into<String>) -> Self {
        self.widget.on_press = serde_json::to_string(&tag.into()).unwrap_or_default();
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        match style.into() {
            Style::Class(s) => self.widget.style_variant = Some(proto::button::StyleVariant::StyleClass(s)),
            Style::Custom(c) => {
                self.widget.style_variant = Some(proto::button::StyleVariant::StyleCustom(proto::ButtonStyle {
                    active: Some(c.active.to_proto()),
                    hovered: Some(c.hovered.to_proto()),
                    pressed: Some(c.pressed.to_proto()),
                    disabled: Some(c.disabled.to_proto()),
                }));
            }
        }
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
}

impl<M> From<Button<M>> for Element<M> {
    fn from(b: Button<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Button(Box::new(b.widget))), 
            key: b.key,
            ..Default::default() 
        }.into()
    }
}

pub fn button<M>(content: impl Into<Element<M>>) -> Button<M> {
    Button::new(content)
}

pub struct TextInput<M> {
    widget: proto::TextInput,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> TextInput<M> {
    pub fn new(placeholder: &str, value: &str) -> Self {
        Self {
            widget: proto::TextInput {
                placeholder: placeholder.to_string(),
                value: value.to_string(),
                size: 16.0,
                padding: Some(proto::Padding { top: 5.0, bottom: 5.0, left: 10.0, right: 10.0 }),
                ..Default::default()
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn on_input(mut self, f: impl Fn(String) -> M) -> Self where M: Serialize {
        let msg = f(String::new());
        self.widget.on_input = serde_json::to_string(&msg).unwrap_or_default();
        self
    }
    pub fn on_input_tag(mut self, tag: impl Into<String>) -> Self {
        self.widget.on_input = serde_json::to_string(&tag.into()).unwrap_or_default();
        self
    }
    pub fn on_submit(mut self, msg: M) -> Self where M: Serialize {
        self.widget.on_submit = serde_json::to_string(&msg).unwrap_or_default();
        self
    }
    pub fn on_submit_tag(mut self, tag: impl Into<String>) -> Self {
        self.widget.on_submit = serde_json::to_string(&tag.into()).unwrap_or_default();
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.widget.size = size;
        self
    }
    pub fn style(mut self, style: impl Into<String>) -> Self {
        self.widget.style = style.into();
        self
    }
}

impl<M> From<TextInput<M>> for Element<M> {
    fn from(t: TextInput<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::TextInput(t.widget)), 
            key: t.key,
            ..Default::default() 
        }.into()
    }
}

pub fn text_input<M>(placeholder: &str, value: &str) -> TextInput<M> {
    TextInput::new(placeholder, value)
}

pub struct Container<M> {
    widget: proto::Container,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Container<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Container {
                child: Some(Box::new(child.into().widget)),
                ..Default::default()
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
    pub fn max_width(mut self, w: f32) -> Self {
        self.widget.max_width = Some(proto::Length { value: Some(proto::length::Value::Fixed(w)) });
        self
    }
    pub fn max_height(mut self, h: f32) -> Self {
        self.widget.max_height = Some(proto::Length { value: Some(proto::length::Value::Fixed(h)) });
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn style(mut self, style: impl Into<String>) -> Self {
        self.widget.style = style.into();
        self
    }
    pub fn background(mut self, background: impl Into<Background>) -> Self {
        self.widget.background = Some(background.into().to_proto());
        self
    }
    pub fn align_x(mut self, align: Alignment) -> Self {
        self.widget.align_x = align as i32;
        self
    }
    pub fn align_y(mut self, align: Alignment) -> Self {
        self.widget.align_y = align as i32;
        self
    }
    pub fn center_x(mut self) -> Self {
        self.widget.align_x = Alignment::Center as i32;
        self
    }
    pub fn center_y(mut self) -> Self {
        self.widget.align_y = Alignment::Center as i32;
        self
    }
}

impl<M> From<Container<M>> for Element<M> {
    fn from(c: Container<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Container(Box::new(c.widget))), 
            key: c.key,
            ..Default::default() 
        }.into()
    }
}

pub fn container<M>(child: impl Into<Element<M>>) -> Container<M> {
    Container::new(child)
}

pub struct Scrollable<M> {
    widget: proto::Scrollable,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Scrollable<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Scrollable {
                content: Some(Box::new(content.into().widget)),
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
}

impl<M> From<Scrollable<M>> for Element<M> {
    fn from(s: Scrollable<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Scrollable(Box::new(s.widget))), 
            key: s.key,
            ..Default::default() 
        }.into()
    }
}

pub fn scrollable<M>(content: impl Into<Element<M>>) -> Scrollable<M> {
    Scrollable::new(content)
}

pub struct ProgressBar<M> {
    widget: proto::ProgressBar,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> ProgressBar<M> {
    pub fn new(range: std::ops::RangeInclusive<f32>, value: f32) -> Self {
        Self {
            widget: proto::ProgressBar {
                range_start: *range.start(),
                range_end: *range.end(),
                value,
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fixed(12.0)) }),
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
}

impl<M> From<ProgressBar<M>> for Element<M> {
    fn from(p: ProgressBar<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::ProgressBar(p.widget)), 
            key: p.key,
            ..Default::default() 
        }.into()
    }
}

pub fn progress_bar<M>(range: std::ops::RangeInclusive<f32>, value: f32) -> ProgressBar<M> {
    ProgressBar::new(range, value)
}

pub struct Space<M> {
    widget: proto::Space,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Space<M> {
    pub fn new(width: Length, height: Length) -> Self {
        Self {
            widget: proto::Space {
                width: Some(width.to_proto()),
                height: Some(height.to_proto()),
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn with_width(w: f32) -> Self {
        Self::new(Length::Fixed(w), Length::Shrink)
    }
    pub fn with_height(h: f32) -> Self {
        Self::new(Length::Shrink, Length::Fixed(h))
    }
}

impl<M> From<Space<M>> for Element<M> {
    fn from(s: Space<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Space(s.widget)), 
            key: s.key,
            ..Default::default() 
        }.into()
    }
}

pub fn space<M>(width: Length, height: Length) -> Space<M> {
    Space::new(width, height)
}

pub struct Stack<M> {
    widget: proto::Stack,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Stack<M> {
    pub fn new() -> Self {
        Self {
            widget: proto::Stack {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn push(mut self, child: impl Into<Element<M>>) -> Self {
        let e: Element<M> = child.into();
        self.widget.children.push(e.widget);
        self
    }
    pub fn extend(mut self, children: impl IntoIterator<Item = Element<M>>) -> Self {
        for child in children { self.widget.children.push(child.widget); }
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
}

impl<M> From<Stack<M>> for Element<M> {
    fn from(s: Stack<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Stack(s.widget)), 
            key: s.key,
            ..Default::default() 
        }.into()
    }
}

pub fn stack<M>(children: impl IntoIterator<Item = Element<M>>) -> Stack<M> {
    Stack::new().extend(children)
}

pub struct Tooltip<M> {
    widget: proto::Tooltip,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

#[derive(Clone, Copy)]
pub enum TooltipPosition {
    Top = 1,
    Bottom = 2,
    Left = 3,
    Right = 4,
}

impl<M> Tooltip<M> {
    pub fn new(content: impl Into<Element<M>>, label: impl Into<String>, position: TooltipPosition) -> Self {
        Self {
            widget: proto::Tooltip {
                content: Some(Box::new(content.into().widget)),
                tooltip: label.into(),
                position: position as u32,
                gap: 5.0,
                padding: Some(proto::Padding::default()),
                ..Default::default()
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.widget.gap = gap;
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
}

impl<M> From<Tooltip<M>> for Element<M> {
    fn from(t: Tooltip<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Tooltip(Box::new(t.widget))), 
            key: t.key,
            ..Default::default() 
        }.into()
    }
}

pub fn tooltip<M>(content: impl Into<Element<M>>, label: impl Into<String>, position: TooltipPosition) -> Tooltip<M> {
    Tooltip::new(content, label, position)
}

pub struct Image<M> {
    widget: proto::WgpuImage,
    key: u64,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Image<M> {
    pub fn new(handle: veldsdk::rpc::core::ResourceHandle) -> Self {
        Self {
            widget: proto::WgpuImage {
                handle: Some(handle),
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
            },
            key: 0,
            _marker: std::marker::PhantomData,
        }
    }
    pub fn key(mut self, k: u64) -> Self {
        self.key = k;
        self
    }
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn height(mut self, h: Length) -> Self {
        self.widget.height = Some(h.to_proto());
        self
    }
}

impl<M> From<Image<M>> for Element<M> {
    fn from(i: Image<M>) -> Self {
        proto::Widget { 
            r#type: Some(proto::widget::Type::Image(i.widget)), 
            key: i.key,
            ..Default::default() 
        }.into()
    }
}

pub fn image<M>(handle: veldsdk::rpc::core::ResourceHandle) -> Image<M> {
    Image::new(handle)
}

// --- Diffing ---

pub mod diffing {
    use super::proto;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use veldsdk::prost::Message;

    pub fn assign_ids_and_hash(widget: &mut proto::Widget, index: &mut u64) -> u64 {
        let mut hasher = DefaultHasher::new();
        
        // 1. Присваиваем стабильный позиционный ID, смешанный с ключом
        let base_id = *index;
        widget.id = base_id ^ widget.key;
        *index += 1;
        
        // 2. Хешируем тип виджета
        let type_tag = match &widget.r#type {
            Some(proto::widget::Type::Column(_)) => 1,
            Some(proto::widget::Type::Row(_)) => 2,
            Some(proto::widget::Type::Text(_)) => 3,
            Some(proto::widget::Type::Button(_)) => 4,
            Some(proto::widget::Type::TextInput(_)) => 5,
            Some(proto::widget::Type::Container(_)) => 6,
            Some(proto::widget::Type::Scrollable(_)) => 7,
            Some(proto::widget::Type::Stack(_)) => 8,
            Some(proto::widget::Type::Space(_)) => 9,
            Some(proto::widget::Type::ProgressBar(_)) => 10,
            Some(proto::widget::Type::Tooltip(_)) => 11,
            Some(proto::widget::Type::Image(_)) => 12,
            _ => 0,
        };
        type_tag.hash(&mut hasher);

        // 3. Рекурсивно обрабатываем детей
        match &mut widget.r#type {
            Some(proto::widget::Type::Column(c)) => { for child in &mut c.children { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Row(r)) => { for child in &mut r.children { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Stack(s)) => { for child in &mut s.children { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Container(c)) => { if let Some(child) = &mut c.child { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Scrollable(s)) => { if let Some(child) = &mut s.content { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Tooltip(t)) => { if let Some(child) = &mut t.content { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            Some(proto::widget::Type::Button(b)) => { if let Some(child) = &mut b.child { assign_ids_and_hash(child, index).hash(&mut hasher); } }
            _ => {}
        }

        // 4. Хешируем контент самого виджета (без ID)
        let mut widget_copy = widget.clone();
        clear_ids(&mut widget_copy);
        let bytes = widget_copy.encode_to_vec();
        bytes.hash(&mut hasher);

        hasher.finish()
    }

    fn clear_ids(widget: &mut proto::Widget) {
        widget.id = 0;
        match &mut widget.r#type {
            Some(proto::widget::Type::Column(c)) => { for child in &mut c.children { clear_ids(child); } }
            Some(proto::widget::Type::Row(r)) => { for child in &mut r.children { clear_ids(child); } }
            Some(proto::widget::Type::Stack(s)) => { for child in &mut s.children { clear_ids(child); } }
            Some(proto::widget::Type::Container(c)) => { if let Some(child) = &mut c.child { clear_ids(child); } }
            Some(proto::widget::Type::Scrollable(s)) => { if let Some(child) = &mut s.content { clear_ids(child); } }
            Some(proto::widget::Type::Tooltip(t)) => { if let Some(child) = &mut t.content { clear_ids(child); } }
            Some(proto::widget::Type::Button(b)) => { if let Some(child) = &mut b.child { clear_ids(child); } }
            _ => {}
        }
    }

    pub fn diff_layouts(old: &proto::Layout, new: &proto::Layout) -> Option<proto::LayoutPatch> {
        if let (Some(o), Some(n)) = (&old.root, &new.root) {
            if o.id == n.id && old.hash == new.hash && old.width == new.width && old.height == new.height {
                return None;
            }
        }

        let mut updates = Vec::new();
        if let (Some(o), Some(n)) = (&old.root, &new.root) {
            find_updates(o, n, &mut updates);
        } else if let Some(n) = &new.root {
            updates.push(proto::WidgetUpdate { widget_id: 0, new_widget: Some(n.clone()) });
        }

        if updates.is_empty() && old.width == new.width && old.height == new.height {
            return None;
        }

        Some(proto::LayoutPatch { updates, width: new.width, height: new.height, new_hash: new.hash })
    }

    fn find_updates(old: &proto::Widget, new: &proto::Widget, updates: &mut Vec<proto::WidgetUpdate>) {
        if old.id != new.id {
            updates.push(proto::WidgetUpdate { widget_id: old.id, new_widget: Some(new.clone()) });
            return;
        }

        let mut o_copy = old.clone(); o_copy.id = 0;
        let mut n_copy = new.clone(); n_copy.id = 0;
        if o_copy.encode_to_vec() == n_copy.encode_to_vec() { return; }

        let mut structural_change = false;
        match (&old.r#type, &new.r#type) {
            (Some(proto::widget::Type::Column(oc)), Some(proto::widget::Type::Column(nc))) => {
                if oc.children.len() == nc.children.len() {
                    for (oc_child, nc_child) in oc.children.iter().zip(nc.children.iter()) { find_updates(oc_child, nc_child, updates); }
                } else { structural_change = true; }
            }
            (Some(proto::widget::Type::Row(oc)), Some(proto::widget::Type::Row(nc))) => {
                if oc.children.len() == nc.children.len() {
                    for (oc_child, nc_child) in oc.children.iter().zip(nc.children.iter()) { find_updates(oc_child, nc_child, updates); }
                } else { structural_change = true; }
            }
            (Some(proto::widget::Type::Stack(oc)), Some(proto::widget::Type::Stack(nc))) => {
                if oc.children.len() == nc.children.len() {
                    for (oc_child, nc_child) in oc.children.iter().zip(nc.children.iter()) { find_updates(oc_child, nc_child, updates); }
                } else { structural_change = true; }
            }
            (Some(proto::widget::Type::Scrollable(oc)), Some(proto::widget::Type::Scrollable(nc))) => {
                if let (Some(oc_c), Some(nc_c)) = (&oc.content, &nc.content) { find_updates(oc_c, nc_c, updates); }
                else { structural_change = true; }
            }
            (Some(proto::widget::Type::Button(oc)), Some(proto::widget::Type::Button(nc))) => {
                if let (Some(oc_c), Some(nc_c)) = (&oc.child, &nc.child) { find_updates(oc_c, nc_c, updates); }
                else { structural_change = true; }
            }
            _ => structural_change = true,
        }

        if structural_change {
            updates.push(proto::WidgetUpdate { widget_id: old.id, new_widget: Some(new.clone()) });
        } else {
            if !matches!(new.r#type, Some(proto::widget::Type::Column(_)) | Some(proto::widget::Type::Row(_))) {
                updates.push(proto::WidgetUpdate { widget_id: old.id, new_widget: Some(new.clone()) });
            }
        }
    }
}



/// UI Rendering module
pub mod render {
    use super::*;

    /// Единственная точка входа для рендеринга.
    /// Теперь принимает готовый корень и ссылку на хранилище старого лейаута.
    pub fn render(
        plugin_id: &str, 
        root_element: Element<()>, 
        last_layout: &mut Option<super::Layout>,
        width: u32,
        height: u32
    ) {
        use prost::Message;
        let mut root_widget = root_element.widget;
        
        let mut index = 0;
        let hash = super::diffing::assign_ids_and_hash(&mut root_widget, &mut index);
        
        let new_layout = super::Layout {
            root: Some(root_widget),
            width,
            height,
            hash,
        };

        let request = if let Some(ref old_layout) = last_layout {
            if let Some(patch) = super::diffing::diff_layouts(old_layout, &new_layout) {
                Some(super::SetViewRequest {
                    plugin_id: plugin_id.to_string(),
                    update: Some(super::set_view_request::Update::Patch(patch)),
                })
            } else {
                None
            }
        } else {
            Some(super::SetViewRequest {
                plugin_id: plugin_id.to_string(),
                update: Some(super::set_view_request::Update::FullLayout(new_layout.clone())),
            })
        };

        if let Some(req) = request {
            veldsdk::output!("ui-service/set_view", req);
        }
        
        *last_layout = Some(new_layout);
    }
}

pub mod reexports {
    pub use std::task::{Poll, Context};
    pub use futures_util::task::noop_waker_ref;
}

#[macro_export]
macro_rules! column {
    ($($x:expr),* $(,)?) => {
        $crate::Column::new()$(.push($x))*
    };
}

#[macro_export]
macro_rules! row {
    ($($x:expr),* $(,)?) => {
        $crate::Row::new()$(.push($x))*
    };
}

#[macro_export]
macro_rules! stack {
    ($($x:expr),* $(,)?) => {
        $crate::Stack::new()$(.push($x))*
    };
}
