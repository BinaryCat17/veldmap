use veld_ui::proto;
use crate::renderer::GpuRenderer;
use iced_widget::{column, row, text, button, container, Space};
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
            let mut col = column(c.children.iter().map(convert_widget))
                .spacing(c.spacing)
                .padding(convert_padding(&c.padding));
            
            if let Some(align) = convert_alignment(c.align_items()) {
                col = col.align_x(align);
            }
            col.width(convert_length(&c.width)).height(convert_length(&c.height)).into()
        }
        Some(proto::widget::Type::Row(r)) => {
            let mut rw = row(r.children.iter().map(convert_widget))
                .spacing(r.spacing)
                .padding(convert_padding(&r.padding));
            
            if let Some(align) = convert_alignment(r.align_items()) {
                rw = rw.align_y(align);
            }
            rw.width(convert_length(&r.width)).height(convert_length(&r.height)).into()
        }
        Some(proto::widget::Type::Text(t)) => {
            text(t.content.clone())
                .size(t.size)
                .color(convert_color(&t.color))
                .into()
        }
        Some(proto::widget::Type::Button(b)) => {
            let mut btn = button(text(b.label.clone()))
                .width(convert_length(&b.width))
                .height(convert_length(&b.height));
            
            if !b.disabled {
                let tag = b.on_press.clone();
                btn = btn.on_press(UiMessage { tag, value: String::new() });
            }
            btn.into()
        }
        Some(proto::widget::Type::Container(c)) => {
            let mut cont = container(if let Some(child) = &c.child { convert_widget(child) } else { Space::with_width(0.0).into() })
                .padding(convert_padding(&c.padding))
                .width(convert_length(&c.width))
                .height(convert_length(&c.height));
            
            if let Some(ax) = convert_alignment(c.align_x()) { cont = cont.align_x(ax); }
            if let Some(ay) = convert_alignment(c.align_y()) { cont = cont.align_y(ay); }
            
            cont.into()
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