// Реэкспортируем системные протоколы, чтобы сгенерированный ui.rs мог их найти
pub use veldsdk::rpc::core;
pub use veldsdk::rpc::app;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
}

use serde::Serialize;
pub use futures_util::task::noop_waker_ref;

// Генерируем транспорт для UI-сервиса. 
veldsdk::rpc_proxy! {
    service: "ui-service",
    set_view: proto::SetViewRequest => proto::SetViewResponse,
    handle_ui_event: proto::HandleUiEventRequest => proto::HandleUiEventResponse,
}

pub struct Element<M> {
    pub widget: proto::Widget,
    pub _marker: std::marker::PhantomData<M>,
}

impl<M> From<proto::Widget> for Element<M> {
    fn from(widget: proto::Widget) -> Self { Self { widget, _marker: std::marker::PhantomData } }
}

pub struct Column<M> {
    widget: proto::Column,
    _marker: std::marker::PhantomData<M>,
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
        self.widget.children.push(child.into().widget);
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
        if let Some(width) = self.widget.width.as_mut() {
             width.value = Some(proto::length::Value::Fixed(w));
        } else {
             self.widget.width = Some(proto::Length { value: Some(proto::length::Value::Fixed(w)) });
        }
        self
    }
}

impl<M> From<Column<M>> for Element<M> {
    fn from(c: Column<M>) -> Self {
        proto::Widget { r#type: Some(proto::widget::Type::Column(c.widget)) }.into()
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
        self.widget.children.push(child.into().widget);
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
}

impl<M> From<Row<M>> for Element<M> {
    fn from(r: Row<M>) -> Self {
        proto::Widget { r#type: Some(proto::widget::Type::Row(r.widget)) }.into()
    }
}

pub fn row<M>(children: impl IntoIterator<Item = Element<M>>) -> Row<M> {
    let mut r = Row::new();
    for child in children { r.widget.children.push(child.widget); }
    r
}

pub struct Text<M> {
    widget: proto::Text,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Text<M> {
    pub fn new(content: impl Into<String>) -> Self {
        Self { widget: proto::Text {
            content: content.into(),
            size: 16.0,
            color: Some(proto::Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }),
            bold: false,
            horizontal_alignment: 0,
            vertical_alignment: 0,
            style: String::new(),
            width: None,
            height: None,
        }, _marker: std::marker::PhantomData }
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
}

impl<M> From<Text<M>> for Element<M> {
    fn from(t: Text<M>) -> Self {
        proto::Widget { r#type: Some(proto::widget::Type::Text(t.widget)) }.into()
    }
}

pub fn text<M>(content: impl Into<String>) -> Text<M> {
    Text::new(content)
}

pub struct Button<M> {
    widget: proto::Button,
    _marker: std::marker::PhantomData<M>,
}

// --- Style Definitions ---

#[derive(Clone, Default)]
pub struct Appearance {
    pub background: Option<Color>,
    pub text_color: Option<Color>,
    pub border: Border,
    pub shadow: Shadow,
}

impl Appearance {
    fn to_proto(self) -> proto::Appearance {
        proto::Appearance {
            background: self.background.map(|c| c.to_proto()),
            text_color: self.text_color.map(|c| c.to_proto()),
            border: Some(self.border.to_proto()),
            shadow: Some(self.shadow.to_proto()),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Border {
    pub color: Color,
    pub width: f32,
    pub radius: f32,
}

impl Border {
    fn to_proto(self) -> proto::Border {
        proto::Border {
            color: Some(self.color.to_proto()),
            width: self.width,
            radius: self.radius,
        }
    }
    pub fn with_radius(radius: f32) -> Self {
        Self { radius, ..Default::default() }
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

pub struct ButtonStyle {
    pub active: Appearance,
    pub hovered: Appearance,
    pub pressed: Appearance,
    pub disabled: Appearance,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            active: Appearance::default(),
            hovered: Appearance::default(),
            pressed: Appearance::default(),
            disabled: Appearance::default(),
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

// --- End Style Definitions ---

impl<M> Button<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self { widget: proto::Button {
            child: Some(Box::new(content.into().widget)),
            on_press: String::new(),
            disabled: false,
            width: None, height: None,
            align_x: None, align_y: None,
            style_variant: Some(proto::button::StyleVariant::StyleClass(String::new())),
            padding: None,
        }, _marker: std::marker::PhantomData }
    }
    pub fn on_press(mut self, msg: M) -> Self where M: Serialize {
        self.widget.on_press = serde_json::to_string(&msg).unwrap_or_default();
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
    pub fn align_x(mut self, align: Alignment) -> Self {
        self.widget.align_x = Some(align as i32);
        self
    }
    pub fn align_y(mut self, align: Alignment) -> Self {
        self.widget.align_y = Some(align as i32);
        self
    }
}

impl<M> From<Button<M>> for Element<M> {
    fn from(b: Button<M>) -> Self {
        proto::Widget { r#type: Some(proto::widget::Type::Button(Box::new(b.widget))) }.into()
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
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }
    pub fn on_input(mut self, f: impl Fn(String) -> M) -> Self where M: Serialize {
        let msg = f(String::new());
        self.widget.on_input = serde_json::to_string(&msg).unwrap_or_default();
        self
    }
    pub fn on_submit(mut self, msg: M) -> Self where M: Serialize {
        self.widget.on_submit = serde_json::to_string(&msg).unwrap_or_default();
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
        proto::Widget { r#type: Some(proto::widget::Type::TextInput(t.widget)) }.into()
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
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn style(mut self, style: impl Into<String>) -> Self {
        self.widget.style = style.into();
        self
    }
    pub fn background(mut self, color: Color) -> Self {
        self.widget.background = Some(color.to_proto());
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
}

impl<M> From<Container<M>> for Element<M> {
    fn from(c: Container<M>) -> Self {
        proto::Widget { r#type: Some(proto::widget::Type::Container(Box::new(c.widget))) }.into()
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
        proto::Widget { r#type: Some(proto::widget::Type::Scrollable(Box::new(s.widget))) }.into()
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
        proto::Widget { r#type: Some(proto::widget::Type::ProgressBar(p.widget)) }.into()
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
        proto::Widget { r#type: Some(proto::widget::Type::Space(s.widget)) }.into()
    }
}

pub fn space<M>(width: Length, height: Length) -> Space<M> {
    Space::new(width, height)
}

#[derive(Clone, Copy)]
pub enum Length {
    Fill,
    Shrink,
    Fixed(f32),
}

impl Length {
    fn to_proto(self) -> proto::Length {
        match self {
            Length::Fill => proto::Length { value: Some(proto::length::Value::Fill(true)) },
            Length::Shrink => proto::Length { value: Some(proto::length::Value::Shrink(true)) },
            Length::Fixed(f) => proto::Length { value: Some(proto::length::Value::Fixed(f)) },
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

pub struct ModuleState<S, M> {
    pub state: S,
    pub tasks: Vec<veldsdk::core::BoxedStream<M>>,
    pub plugin_name: String,
    pub width: u32,
    pub height: u32,
}

#[macro_export]
macro_rules! define_remote_ui_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        message: $message_type:ty,
        init: $init_func:path,
        view: $view_func:path,
        handlers: {
            $($msg_variant:ident $( ( $($arg:ident),* ) )? => $handler_func:path);* $(;)?
        }
    ) => {
        #[no_mangle]
        pub extern "C" fn init() -> i32 {
            let _ = veldsdk::core::init();
            let config_json = match veldsdk::rpc::host::get_config("config") {
                Some(c) => c,
                None => return 1,
            };
            let config: $config_type = match veldsdk::serde_json::from_str(&config_json) {
                Ok(c) => c,
                Err(_) => return 2,
            };
            let plugin_name = veldsdk::rpc::host::get_config("plugin_name").unwrap_or_else(|| "unknown".to_string());
            let (state, _) = match $init_func(config) {
                Ok(s) => s,
                Err(_) => return 3,
            };
            let module_state = $crate::ModuleState::<$state_type, $message_type> {
                state,
                tasks: Vec::new(),
                plugin_name,
                width: 1024,
                height: 768,
            };
            if veldsdk::rpc::MODULE_STATE.set(Ok(std::sync::Arc::new(std::sync::Mutex::new(Box::new(module_state))))).is_err() { return 4; }
            0
        }

        #[no_mangle]
        pub extern "C" fn poll_tasks() -> i32 {
            let state_arc = match veldsdk::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                _ => return 0,
            };
            let mut state_lock = match state_arc.try_lock() {
                Ok(l) => l,
                Err(_) => return 0,
            };
            let module = state_lock.downcast_mut::<$crate::ModuleState<$state_type, $message_type>>().unwrap();

            let waker = $crate::reexports::noop_waker_ref();
            let mut cx = $crate::reexports::Context::from_waker(waker);
            let mut new_messages = Vec::new();
            
            use veldsdk::futures_util::stream::StreamExt;
            
            module.tasks.retain_mut(|task| {
                loop {
                    match task.poll_next_unpin(&mut cx) {
                        $crate::reexports::Poll::Ready(Some(msg)) => {
                            new_messages.push(msg);
                        },
                        $crate::reexports::Poll::Ready(None) => return false,
                        $crate::reexports::Poll::Pending => return true,
                    }
                }
            });

            for msg in new_messages {
                let cmd = internal_update(&mut module.state, msg);
                module.tasks.extend(cmd.0);
            }
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            use veldsdk::prost::Message;
            use veldsdk::rpc::core::{RpcRequest, RpcResponse};

            let input = veldsdk::rpc::host::load_input();
            let request = match RpcRequest::decode(&input[..]) {
                Ok(r) => r,
                Err(_) => return 1,
            };
            
            let state_arc = match veldsdk::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                _ => return 2,
            };
            let mut state_lock = state_arc.lock().unwrap();
            let module = state_lock.downcast_mut::<$crate::ModuleState<$state_type, $message_type>>().unwrap();

            match request.method.as_str() {
                "handle_ui_event" => {
                    let event = match veldsdk::rpc::app::UiEvent::decode(&request.payload[..]) {
                        Ok(e) => e,
                        Err(_) => return 3,
                    };
                    
                    if let Some(ref ev_type) = event.event {
                        match ev_type {
                            veldsdk::rpc::app::ui_event::Event::Resize(r) => {
                                module.width = r.width;
                                module.height = r.height;
                            }
                            veldsdk::rpc::app::ui_event::Event::Frame(_f) => {
                                let waker = $crate::reexports::noop_waker_ref();
                                let mut cx = $crate::reexports::Context::from_waker(waker);
                                let mut new_messages = Vec::new();
                                
                                use veldsdk::futures_util::stream::StreamExt;
                                
                                module.tasks.retain_mut(|task| {
                                    loop {
                                        match task.poll_next_unpin(&mut cx) {
                                            $crate::reexports::Poll::Ready(Some(msg)) => {
                                                new_messages.push(msg);
                                            },
                                            $crate::reexports::Poll::Ready(None) => return false,
                                            $crate::reexports::Poll::Pending => return true,
                                        }
                                    }
                                });

                                for msg in new_messages {
                                    let cmd = internal_update(&mut module.state, msg);
                                    module.tasks.extend(cmd.0);
                                }

                                let element = $view_func(&module.state);
                                let layout = $crate::proto::Layout {
                                    root: Some(element.widget),
                                    width: module.width, height: module.height,
                                };
                                let _ = $crate::raw::set_view(&$crate::proto::SetViewRequest {
                                    plugin_id: module.plugin_name.clone(),
                                    layout: Some(layout),
                                });
                            }
                            _ => {}
                        }
                    }

                    if let Ok(ui_res) = $crate::raw::handle_ui_event(&$crate::proto::HandleUiEventRequest {
                        plugin_id: module.plugin_name.clone(),
                        event: Some(event),
                    }) {
                        for msg_res in ui_res.messages {
                            let message: $message_type = match veldsdk::serde_json::from_str(&msg_res.message_tag) {
                                Ok(m) => m,
                                Err(_) => continue,
                            };
                            let cmd = internal_update(&mut module.state, message);
                            module.tasks.extend(cmd.0);
                        }
                    }
                    
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    veldsdk::rpc::host::store_output(response.encode_to_vec());
                    0
                }
                _ => 6,
            }
        }

        fn internal_update(state: &mut $state_type, message: $message_type) -> veldsdk::core::Command<$message_type> {
            #[allow(unused_imports)]
            use $message_type::*;
            match message {
                $(
                    $msg_variant $( ( $($arg),* ) )? => {
                        $handler_func(state, $($($arg),*)?)
                    }
                )*
            }
        }
    };
}

pub mod reexports {
    pub use std::task::{Poll, Context};
    pub use futures_util::task::noop_waker_ref;
}