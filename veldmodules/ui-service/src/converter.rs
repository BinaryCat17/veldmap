use crate::proto::ui_service as proto;
use crate::module::renderer::GpuRenderer;
use iced_widget::{column, row, text, button, container, scrollable, progress_bar, stack, tooltip, Space};
use iced_core::{Element, Theme, Length, Color, alignment, Size, Font, font::Family};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct UiMessage {
    pub method: String,
    pub value: String,
}

/// `iced_core::Font::family` требует `&'static str`, а логическое имя шрифта
/// приходит рантаймовой строкой из proto (через wasm ABI). Каждое различное
/// имя один раз "утекает" в статическую память и переиспользуется — набор
/// имён на практике фиксирован (задан в коде плагинов), так что утечка ограничена.
fn intern_font_family(name: &str) -> &'static str {
    static INTERNED: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);
    let mut guard = INTERNED.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(existing) = map.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    map.insert(name.to_string(), leaked);
    leaked
}

pub fn convert_layout(layout: &proto::Layout) -> Element<'static, UiMessage, Theme, GpuRenderer> {
    if let Some(root) = &layout.root {
        convert_widget(root)
    } else {
        column([]).into()
    }
}

fn convert_widget(widget: &proto::Widget) -> Element<'static, UiMessage, Theme, GpuRenderer> {
    match &widget.r#type {
        Some(proto::widget::Type::Column(c)) => {
            let mut col = column(c.children.iter().map(convert_widget))
                .spacing(c.spacing)
                .padding(convert_padding(&c.padding));
            
            if let Some(align) = convert_alignment(c.align_items()) {
                col = col.align_x(align);
            }
            col.width(convert_length(&c.width))
               .height(convert_length(&c.height))
               .into()
        }
        Some(proto::widget::Type::Row(r)) => {
            let mut rw = row(r.children.iter().map(convert_widget))
                .spacing(r.spacing)
                .padding(convert_padding(&r.padding));
            
            if let Some(align) = convert_alignment(r.align_items()) {
                rw = rw.align_y(align);
            }
            rw.width(convert_length(&r.width))
              .height(convert_length(&r.height))
              .into()
        }
        Some(proto::widget::Type::Text(t)) => {
            let mut size = t.size;
            if size <= 0.0 {
                veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "Text widget has invalid size: {}. Resetting to 16.0", size);
                size = 16.0;
            }
            let mut txt = text(t.content.clone())
                .size(size)
                .color(convert_color(&t.color))
                .width(convert_length(&t.width))
                .height(convert_length(&t.height))
                .align_x(convert_horizontal_alignment(t.horizontal_alignment()))
                .align_y(convert_vertical_alignment(t.vertical_alignment()))
                .shaping(match t.shaping {
                    1 => iced_core::text::Shaping::Advanced,
                    _ => iced_core::text::Shaping::Basic,
                });

            if !t.font_family.is_empty() {
                let family = intern_font_family(&t.font_family);
                txt = txt.font(Font { family: Family::Name(family), ..Font::DEFAULT });
            }

            txt.into()
        }
        Some(proto::widget::Type::Button(b)) => {
            let content = if let Some(child) = &b.child {
                convert_widget(child)
            } else {
                iced_widget::Space::with_width(0.0).into()
            };

            let mut btn = button(content)
                .width(convert_length(&b.width))
                .height(convert_length(&b.height))
                .padding(convert_padding(&b.padding));
            
            if !b.disabled {
                if let Some(h) = &b.on_press {
                    if !h.method.is_empty() {
                        btn = btn.on_press(UiMessage { method: h.method.clone(), value: h.value.clone() });
                    }
                }
            }

            match &b.style_variant {
                Some(proto::button::StyleVariant::StyleClass(name)) => {
                    match name.as_str() {
                        "text" | "sync_button" | "download_button" => { 
                            btn = btn.style(iced_widget::button::text); 
                        }
                        "primary" => { btn = btn.style(iced_widget::button::primary); }
                        "secondary" => { btn = btn.style(iced_widget::button::secondary); }
                        "success" => { btn = btn.style(iced_widget::button::success); }
                        "danger" => { btn = btn.style(iced_widget::button::danger); }
                        _ => {
                            btn = btn.style(iced_widget::button::primary);
                        }
                    }
                }
                Some(proto::button::StyleVariant::StyleCustom(custom)) => {
                    let active = convert_widget_style(&Some(custom.active.clone().unwrap_or_default()));
                    let hovered = convert_widget_style(&Some(custom.hovered.clone().unwrap_or_default()));
                    let pressed = convert_widget_style(&Some(custom.pressed.clone().unwrap_or_default()));
                    let disabled = convert_widget_style(&Some(custom.disabled.clone().unwrap_or_default()));

                    btn = btn.style(move |_theme: &Theme, status| {
                         match status {
                             iced_widget::button::Status::Active => active,
                             iced_widget::button::Status::Hovered => hovered,
                             iced_widget::button::Status::Pressed => pressed,
                             iced_widget::button::Status::Disabled => disabled,
                         }
                    });
                }
                None => {
                    btn = btn.style(iced_widget::button::primary);
                }
            }
            
            btn.into()
        }
        Some(proto::widget::Type::TextInput(t)) => {
            let mut size = t.size;
            if size <= 0.0 {
                veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "TextInput widget has invalid size: {}. Resetting to 16.0", size);
                size = 16.0;
            }
            let mut input = iced_widget::text_input(&t.placeholder, &t.value)
                .width(convert_length(&t.width))
                .padding(convert_padding(&t.padding))
                .size(size);
            
            if let Some(h) = &t.on_input {
                if !h.method.is_empty() {
                    let method = h.method.clone();
                    // The typed text replaces the handler's value at runtime.
                    input = input.on_input(move |v| UiMessage { method: method.clone(), value: v });
                }
            }

            if let Some(h) = &t.on_submit {
                if !h.method.is_empty() {
                    input = input.on_submit(UiMessage { method: h.method.clone(), value: h.value.clone() });
                }
            }

            input.into()
        }
        Some(proto::widget::Type::Container(c)) => {
            let mut cont = container(if let Some(child) = &c.child { convert_widget(child) } else { iced_widget::Space::with_width(0.0).into() })
                .padding(convert_padding(&c.padding))
                .width(convert_length(&c.width))
                .height(convert_length(&c.height))
                .max_width(convert_length_val(&c.max_width))
                .max_height(convert_length_val(&c.max_height));
            
            if let Some(ax) = convert_alignment(c.align_x()) { cont = cont.align_x(ax); }
            if let Some(ay) = convert_alignment(c.align_y()) { cont = cont.align_y(ay); }

            if let Some(bg) = &c.background {
                let background = convert_background(bg);
                cont = cont.style(move |_theme: &Theme| {
                    iced_widget::container::Style {
                        background: Some(background),
                        ..Default::default()
                    }
                });
            }
            
            cont.into()
        }
        Some(proto::widget::Type::Scrollable(s)) => {
            let content = if let Some(child) = &s.content { convert_widget(child) } else { Space::with_width(0.0).into() };
            scrollable(content)
                .width(convert_length(&s.width))
                .height(convert_length(&s.height))
                .into()
        }
        Some(proto::widget::Type::ProgressBar(p)) => {
            progress_bar(p.range_start..=p.range_end, p.value)
                .width(convert_length(&p.width))
                .height(convert_length(&p.height))
                .into()
        }
        Some(proto::widget::Type::Stack(s)) => {
            stack(s.children.iter().map(convert_widget))
                .width(convert_length(&s.width))
                .height(convert_length(&s.height))
                .into()
        }
        Some(proto::widget::Type::Tooltip(t)) => {
            let content = if let Some(c) = &t.content {
                convert_widget(c)
            } else {
                Space::with_width(0.0).into()
            };
            
            let position = match t.position {
                1 => iced_widget::tooltip::Position::Top,
                2 => iced_widget::tooltip::Position::Bottom,
                3 => iced_widget::tooltip::Position::Left,
                4 => iced_widget::tooltip::Position::Right,
                _ => iced_widget::tooltip::Position::Top,
            };

            tooltip(content, text(t.tooltip.clone()), position)
                .gap(t.gap)
                .padding(t.padding.as_ref().map(|p| p.top).unwrap_or(5.0))
                .into()
        }
        Some(proto::widget::Type::Image(img)) => {
            let handle = img.handle.clone().unwrap_or_default();
            veldsdk::vdebug!(veldsdk::FLAG_UI_HANDLERS, "WgpuImage: handle.id = {}, handle.size = {}", handle.id, handle.size);
            WgpuImageWidget {
                handle,
                width: convert_length(&img.width),
                height: convert_length(&img.height),
            }.into()
        }
        _ => column([]).into(),
    }
}

fn convert_length(len: &Option<proto::Length>) -> Length {
    match len {
        Some(l) => match &l.value {
            Some(proto::length::Value::Fixed(f)) => Length::Fixed(*f),
            Some(proto::length::Value::Fill(_)) => Length::Fill,
            Some(proto::length::Value::Shrink(_)) => Length::Shrink,
            Some(proto::length::Value::Portion(p)) => Length::FillPortion(*p as u16),
            None => Length::Shrink,
        },
        None => Length::Shrink,
    }
}

fn convert_length_val(len: &Option<proto::Length>) -> f32 {
    match len {
        Some(l) => match &l.value {
            Some(proto::length::Value::Fixed(f)) => *f,
            _ => f32::INFINITY,
        },
        None => f32::INFINITY,
    }
}

fn convert_padding(p: &Option<proto::Padding>) -> iced_core::Padding {
    match p {
        Some(p) => iced_core::Padding {
            top: p.top,
            right: p.right,
            bottom: p.bottom,
            left: p.left,
        },
        None => iced_core::Padding::ZERO,
    }
}

fn convert_horizontal_alignment(a: proto::Alignment) -> iced_core::alignment::Horizontal {
    match a {
        proto::Alignment::Start => iced_core::alignment::Horizontal::Left,
        proto::Alignment::Center => iced_core::alignment::Horizontal::Center,
        proto::Alignment::End => iced_core::alignment::Horizontal::Right,
    }
}

fn convert_vertical_alignment(a: proto::Alignment) -> iced_core::alignment::Vertical {
    match a {
        proto::Alignment::Start => iced_core::alignment::Vertical::Top,
        proto::Alignment::Center => iced_core::alignment::Vertical::Center,
        proto::Alignment::End => iced_core::alignment::Vertical::Bottom,
    }
}

fn convert_alignment(a: proto::Alignment) -> Option<alignment::Alignment> {
    match a {
        proto::Alignment::Start => Some(alignment::Alignment::Start),
        proto::Alignment::Center => Some(alignment::Alignment::Center),
        proto::Alignment::End => Some(alignment::Alignment::End),
    }
}

fn convert_color(c: &Option<proto::Color>) -> Color {
    match c {
        Some(c) => Color::from_rgba(c.r, c.g, c.b, c.a),
        None => Color::BLACK,
    }
}

fn convert_widget_style(style: &Option<proto::WidgetStyle>) -> iced_widget::button::Style {
    if let Some(s) = style {
        iced_widget::button::Style {
            background: s.background.as_ref().map(convert_background),
            text_color: s.text_color.as_ref().map(|c| convert_color(&Some(c.clone()))).unwrap_or(Color::BLACK),
            border: convert_border(&s.border),
            shadow: convert_shadow(&s.shadow),
        }
    } else {
        iced_widget::button::Style::default()
    }
}

fn convert_background(bg: &proto::Background) -> iced_core::Background {
    match &bg.r#type {
        Some(proto::background::Type::Color(c)) => iced_core::Background::Color(convert_color(&Some(c.clone()))),
        None => iced_core::Background::Color(Color::TRANSPARENT),
    }
}

fn convert_border(b: &Option<proto::Border>) -> iced_core::Border {
    if let Some(b) = b {
        iced_core::Border {
            color: convert_color(&b.color),
            width: b.width,
            radius: convert_radius(&b.radius).into(),
        }
    } else {
        iced_core::Border::default()
    }
}

fn convert_radius(r: &Option<proto::Radius>) -> iced_core::border::Radius {
    if let Some(r) = r {
        iced_core::border::Radius {
            top_left: r.top_left,
            top_right: r.top_right,
            bottom_right: r.bottom_right,
            bottom_left: r.bottom_left,
        }
    } else {
        iced_core::border::Radius::from(0.0)
    }
}

fn convert_shadow(s: &Option<proto::Shadow>) -> iced_core::Shadow {
    if let Some(s) = s {
        iced_core::Shadow {
            color: convert_color(&s.color),
            offset: iced_core::Vector::new(s.offset_x, s.offset_y),
            blur_radius: s.blur_radius,
        }
    } else {
        iced_core::Shadow::default()
    }
}

struct WgpuImageWidget {
    handle: veldsdk::proto::core::ResourceHandle,
    width: Length,
    height: Length,
}

impl<'a, Message, Theme> iced_widget::core::Widget<Message, Theme, GpuRenderer> for WgpuImageWidget {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &self,
        _tree: &mut iced_widget::core::widget::Tree,
        _renderer: &GpuRenderer,
        limits: &iced_widget::core::layout::Limits,
    ) -> iced_widget::core::layout::Node {
        iced_widget::core::layout::Node::new(limits.resolve(self.width, self.height, Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &iced_widget::core::widget::Tree,
        renderer: &mut GpuRenderer,
        _theme: &Theme,
        _style: &iced_widget::core::renderer::Style,
        layout: iced_widget::core::layout::Layout<'_>,
        _cursor: iced_widget::core::mouse::Cursor,
        _viewport: &iced_widget::core::Rectangle,
    ) {
        renderer.draw_wgpu_image(layout.bounds(), self.handle.id);
    }
}

impl<'a, Message, Theme> From<WgpuImageWidget> for Element<'a, Message, Theme, GpuRenderer> {
    fn from(widget: WgpuImageWidget) -> Self {
        Self::new(widget)
    }
}
