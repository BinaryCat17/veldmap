//! Element и билдеры виджетов: типобезопасная сборка дерева proto::Widget.
//! Стили и геометрия — в style.rs.

use crate::proto;
use super::style::{Alignment, Background, Length, Padding, Style};

pub struct Element<M> {
    pub widget: proto::Widget,
    pub _marker: std::marker::PhantomData<M>,
}

impl<M> From<proto::Widget> for Element<M> {
    fn from(widget: proto::Widget) -> Self { Self { widget, _marker: std::marker::PhantomData } }
}

// --- Builder Structs ---

pub struct Column<M> {
    widget: proto::Column,
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
            _marker: std::marker::PhantomData
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn column<M>(children: impl IntoIterator<Item = Element<M>>) -> Column<M> {
    Column::new().extend(children)
}

pub struct Row<M> {
    widget: proto::Row,
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
            _marker: std::marker::PhantomData
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn row<M>(children: impl IntoIterator<Item = Element<M>>) -> Row<M> {
    Row::new().extend(children)
}

pub struct Text<M> {
    widget: proto::Text,
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
                width: None,
                height: None,
                shaping: 0,
                font_family: String::new(),
            },
            _marker: std::marker::PhantomData
        }
    }
    pub fn size(mut self, size: f32) -> Self {
        self.widget.size = size;
        self
    }
    pub fn color(mut self, color: super::style::Color) -> Self {
        self.widget.color = Some(color.to_proto());
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
    /// Логическое имя шрифта, как оно было зарегистрировано в `GpuRenderer::new`
    /// на стороне ui-service (например "Icons"). Пусто — используется дефолтный шрифт.
    pub fn font_family(mut self, name: impl Into<String>) -> Self {
        self.widget.font_family = name.into();
        self
    }
}

impl<M> From<Text<M>> for Element<M> {
    fn from(t: Text<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Text(t.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn text<M>(content: impl Into<String>) -> Text<M> {
    Text::new(content)
}

pub struct Button<M> {
    widget: proto::Button,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Button<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self { widget: proto::Button {
            child: Some(Box::new(content.into().widget)),
            on_press: None,
            disabled: false,
            width: None, height: None,
            style_variant: Some(proto::button::StyleVariant::StyleClass(String::new())),
            padding: None,
        }, _marker: std::marker::PhantomData }
    }
    /// Dispatch a press to the named input method of the owning module.
    pub fn on_press(mut self, method: impl Into<String>) -> Self {
        self.widget.on_press = Some(proto::Handler { method: method.into(), value: String::new() });
        self
    }
    /// Like `on_press`, with a payload delivered in `UiEventResponse.value`.
    pub fn on_press_with(mut self, method: impl Into<String>, value: impl Into<String>) -> Self {
        self.widget.on_press = Some(proto::Handler { method: method.into(), value: value.into() });
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
            ..Default::default()
        }.into()
    }
}

pub fn button<M>(content: impl Into<Element<M>>) -> Button<M> {
    Button::new(content)
}

pub struct TextInput<M> {
    widget: proto::TextInput,
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
            _marker: std::marker::PhantomData,
        }
    }
    /// Dispatch typed text to the named input method (the renderer fills value).
    pub fn on_input(mut self, method: impl Into<String>) -> Self {
        self.widget.on_input = Some(proto::Handler { method: method.into(), value: String::new() });
        self
    }
    /// Dispatch Enter/submit to the named input method.
    pub fn on_submit(mut self, method: impl Into<String>) -> Self {
        self.widget.on_submit = Some(proto::Handler { method: method.into(), value: String::new() });
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
}

impl<M> From<TextInput<M>> for Element<M> {
    fn from(t: TextInput<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::TextInput(t.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn text_input<M>(placeholder: &str, value: &str) -> TextInput<M> {
    TextInput::new(placeholder, value)
}

pub struct Container<M> {
    widget: proto::Container,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Container<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Container {
                child: Some(Box::new(child.into().widget)),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn container<M>(child: impl Into<Element<M>>) -> Container<M> {
    Container::new(child)
}

pub struct Scrollable<M> {
    widget: proto::Scrollable,
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
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn scrollable<M>(content: impl Into<Element<M>>) -> Scrollable<M> {
    Scrollable::new(content)
}

pub struct ProgressBar<M> {
    widget: proto::ProgressBar,
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
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn progress_bar<M>(range: std::ops::RangeInclusive<f32>, value: f32) -> ProgressBar<M> {
    ProgressBar::new(range, value)
}

pub struct Space<M> {
    widget: proto::Space,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Space<M> {
    pub fn new(width: Length, height: Length) -> Self {
        Self {
            widget: proto::Space {
                width: Some(width.to_proto()),
                height: Some(height.to_proto()),
            },
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn space<M>(width: Length, height: Length) -> Space<M> {
    Space::new(width, height)
}

pub struct Stack<M> {
    widget: proto::Stack,
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
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn stack<M>(children: impl IntoIterator<Item = Element<M>>) -> Stack<M> {
    Stack::new().extend(children)
}

pub struct Tooltip<M> {
    widget: proto::Tooltip,
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
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn tooltip<M>(content: impl Into<Element<M>>, label: impl Into<String>, position: TooltipPosition) -> Tooltip<M> {
    Tooltip::new(content, label, position)
}

pub struct Image<M> {
    widget: proto::WgpuImage,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Image<M> {
    pub fn new(handle: veldsdk::proto::core::ResourceHandle) -> Self {
        Self {
            widget: proto::WgpuImage {
                handle: Some(handle),
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
            },
            _marker: std::marker::PhantomData,
        }
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
            ..Default::default()
        }.into()
    }
}

pub fn image<M>(handle: veldsdk::proto::core::ResourceHandle) -> Image<M> {
    Image::new(handle)
}
