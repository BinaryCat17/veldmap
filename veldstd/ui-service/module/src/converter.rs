use veld_ui::proto;
use crate::renderer::GpuRenderer;
use iced_widget::{column, row, text, button, container, scrollable, progress_bar, stack, tooltip, Space};
use iced_core::{Element, Theme, Length, Color, alignment, Size};

#[derive(Clone, Debug)]
pub struct UiMessage {
    pub tag: String,
    pub value: String,
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
            // log::info!("Converting Column: {} children, width: {:?}", c.children.len(), c.width);
            let mut col = column(c.children.iter().map(convert_widget))
                .spacing(c.spacing)
                .padding(convert_padding(&c.padding));
            
            if let Some(align) = convert_alignment(c.align_items()) {
                col = col.align_x(align);
            }
            col.width(convert_length(&c.width)).height(convert_length(&c.height)).into()
        }
        Some(proto::widget::Type::Row(r)) => {
             // log::info!("Converting Row: {} children", r.children.len());
            let mut rw = row(r.children.iter().map(convert_widget))
                .spacing(r.spacing)
                .padding(convert_padding(&r.padding));
            
            if let Some(align) = convert_alignment(r.align_items()) {
                rw = rw.align_y(align);
            }
            rw.width(convert_length(&r.width)).height(convert_length(&r.height)).into()
        }
        Some(proto::widget::Type::Text(t)) => {
            let txt = text(t.content.clone())
                .size(t.size)
                .color(convert_color(&t.color))
                .width(convert_length(&t.width))
                .height(convert_length(&t.height))
                .align_x(convert_horizontal_alignment(t.horizontal_alignment()))
                .align_y(convert_vertical_alignment(t.vertical_alignment()));
            
            // Здесь можно добавить маппинг стилей текста если нужно
            
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
                .height(convert_length(&b.height));
            
            if !b.disabled {
                let tag = b.on_press.clone();
                btn = btn.on_press(UiMessage { tag, value: String::new() });
            }

            // Маппинг стилей
            match &b.style_variant {
                Some(proto::button::StyleVariant::StyleClass(name)) => {
                    match name.as_str() {
                        "text" | "sync_button" => { 
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
                    // Клонируем данные, чтобы переместить их в замыкание
                    let active = convert_appearance(&custom.active);
                    let hovered = convert_appearance(&custom.hovered);
                    let pressed = convert_appearance(&custom.pressed);
                    let disabled = convert_appearance(&custom.disabled);

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
            
            // Если паддинг задан явно, он переопределяет паддинг стиля
            if b.padding.is_some() {
                 btn = btn.padding(convert_padding(&b.padding));
            }

            btn.into()
        }
        Some(proto::widget::Type::TextInput(t)) => {
            let mut input = iced_widget::text_input(&t.placeholder, &t.value)
                .width(convert_length(&t.width))
                .padding(convert_padding(&t.padding))
                .size(t.size);
            
            if !t.on_input.is_empty() {
                let tag = t.on_input.clone();
                input = input.on_input(move |v| UiMessage { tag: tag.clone(), value: v });
            }
            
            if !t.on_submit.is_empty() {
                let tag = t.on_submit.clone();
                input = input.on_submit(UiMessage { tag, value: String::new() });
            }

            input.into()
        }
        Some(proto::widget::Type::Container(c)) => {
            let mut cont = container(if let Some(child) = &c.child { convert_widget(child) } else { iced_widget::Space::with_width(0.0).into() })
                .padding(convert_padding(&c.padding))
                .width(convert_length(&c.width))
                .height(convert_length(&c.height));
            
            if let Some(ax) = convert_alignment(c.align_x()) { cont = cont.align_x(ax); }
            if let Some(ay) = convert_alignment(c.align_y()) { cont = cont.align_y(ay); }

            // Стилизация контейнера
            if !c.style.is_empty() {
                // Здесь можно добавить маппинг стилей контейнера
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
            
            // Note: iced 0.13 tooltip position is an enum
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
            WgpuImageWidget {
                handle: img.handle.clone().unwrap_or_default(),
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

fn convert_appearance(app: &Option<proto::Appearance>) -> iced_widget::button::Style {
    if let Some(a) = app {
        iced_widget::button::Style {
            background: a.background.as_ref().map(|c| iced_core::Background::Color(convert_color(&Some(c.clone())))),
            text_color: convert_color(&a.text_color),
            border: convert_border(&a.border),
            shadow: convert_shadow(&a.shadow),
        }
    } else {
        iced_widget::button::Style::default()
    }
}

fn convert_border(b: &Option<proto::Border>) -> iced_core::Border {
    if let Some(b) = b {
        iced_core::Border {
            color: convert_color(&b.color),
            width: b.width,
            radius: b.radius.into(),
        }
    } else {
        iced_core::Border::default()
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
    handle: veldsdk::rpc::core::ResourceHandle,
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