//! viewport.rs — место, которое рисует владелец разметки (см. `Viewport` в
//! types.proto).
//!
//! Всё, что здесь есть, следует из одного: у показываемой текстуры нет
//! собственного размера — её размер назначает раскладка. Отсюда и обязанности
//! виджета: сказать владельцу, сколько места ему досталось, и передать ему
//! указатель в тех же единицах, в которых сказан размер, — в пикселях его
//! текстуры.
//!
//! Виджет не обобщён по типу сообщения, в отличие от `Popover`: тот — способ
//! разложить чужие элементы и о протоколе не знает, а этот сам изготавливает
//! нагрузку события (`PointerEvent`, `ViewportSize`) и без неё бессмыслен.

use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::{self, Widget};
use iced_core::{Clipboard, Element, Event, Length, Point, Rectangle, Shell, Size};

use crate::module::converter::UiMessage;
use crate::module::pointer::to_index;
use crate::module::renderer::GpuRenderer;
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};

pub struct Viewport {
    /// Region id текстуры; 0 — владелец ещё её не выделил.
    texture_id: u64,
    width: Length,
    height: Length,
    /// Имена методов, которыми владелец назвал свои обработчики. `None` — он
    /// об этом знать не хочет.
    on_resized: Option<String>,
    on_pointer: Option<String>,
    /// Чем владелец назвал саму область. Нагрузку события здесь изготавливает
    /// виджет (указатель, размер), поэтому назвать себя область может только
    /// отдельным полем — см. `UiEventResponse.key`.
    key: String,
}

/// Состояние между кадрами. Всё три поля — про то, о чём владельцу уже
/// сказали: без них он получал бы один и тот же факт на каждом событии.
#[derive(Default)]
struct State {
    /// Размер, о котором владельцу уже сообщили.
    reported: (u32, u32),
    /// Кнопка, нажатая внутри области и ещё не отпущенная. Пока она есть,
    /// движения уходят владельцу и за краем области: иначе перетаскивание
    /// рвалось бы о границу ровно там, где оно нужнее всего.
    grabbed: Option<u32>,
    /// Был ли курсор над областью. Нужен, чтобы сказать «ушёл» один раз, а не
    /// на каждом движении мимо.
    inside: bool,
}

impl Viewport {
    pub fn new(texture_id: u64, width: Length, height: Length) -> Self {
        Self {
            texture_id,
            width,
            height,
            on_resized: None,
            on_pointer: None,
            key: String::new(),
        }
    }

    pub fn on_resized(mut self, method: String) -> Self {
        self.on_resized = Some(method);
        self
    }

    pub fn on_pointer(mut self, method: String) -> Self {
        self.on_pointer = Some(method);
        self
    }

    /// Имя области у владельца. Оба обработчика называют её одинаково —
    /// область одна, — поэтому имя ставится один раз, а не по разу на метод.
    pub fn key(mut self, key: String) -> Self {
        self.key = key;
        self
    }
}

impl Widget<UiMessage, iced_core::Theme, GpuRenderer> for Viewport {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &GpuRenderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(self.width, self.height, Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &GpuRenderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiMessage>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        // Масштаб спрашивается у рендерера, а не хранится здесь: он один на
        // весь кадр, и второй его носитель разошёлся бы с первым при смене DPI.
        let scale = renderer.current_sf;

        if let Some(method) = &self.on_resized {
            let size = (
                (bounds.width * scale).round().max(0.0) as u32,
                (bounds.height * scale).round().max(0.0) as u32,
            );
            // Нулевой размер — это ещё не раскладка, а её отсутствие: текстуру
            // под него не выделить, и говорить о нём нечего.
            if size != state.reported && size.0 > 0 && size.1 > 0 {
                state.reported = size;
                shell.publish(UiMessage::sized(
                    method.clone(),
                    self.key.clone(),
                    ViewportSize { width: size.0, height: size.1 },
                ));
            }
        }

        let Some(method) = &self.on_pointer else { return };
        let key = self.key.clone();

        // Координаты области: начало — её левый верхний угол, единица — пиксель
        // её текстуры. За краем они выходят за границы, и это законно: так
        // выглядит перетаскивание, ушедшее за область.
        let local = |p: Point| ((p.x - bounds.x) * scale, (p.y - bounds.y) * scale);
        // Кнопки нет у движения и прокрутки — там это `None`, и в протоколе
        // оно называется нулём (см. `PointerEvent` в types.proto).
        let mut send = |action: PointerAction, at: Point, button: Option<u32>, scroll: (f32, f32)| {
            let (x, y) = local(at);
            shell.publish(UiMessage::pointer(method.clone(), key.clone(), PointerEvent {
                action: action as i32,
                x,
                y,
                button: button.unwrap_or(0),
                scroll_y: scroll.1,
            }));
        };

        let over = cursor.position_over(bounds);
        let Some(at) = cursor.position() else { return };

        match event {
            // Кнопки, которой нет в протоколе, для области не существует:
            // назвать её владельцу нечем, а «какая-нибудь» кнопка означала бы
            // жест, которого не делали.
            Event::Mouse(mouse::Event::ButtonPressed(button))
                if over.is_some() && to_index(*button).is_some() =>
            {
                state.grabbed = to_index(*button);
                send(PointerAction::PointerPressed, at, to_index(*button), (0.0, 0.0));
                shell.capture_event();
            }
            // Отпускание ловится по своему нажатию, а не по границам: кнопку,
            // нажатую в области, отпускают где угодно, и владелец обязан узнать
            // об этом — иначе его перетаскивание никогда не кончится.
            Event::Mouse(mouse::Event::ButtonReleased(button))
                if state.grabbed.is_some() && state.grabbed == to_index(*button) =>
            {
                state.grabbed = None;
                send(PointerAction::PointerReleased, at, to_index(*button), (0.0, 0.0));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.grabbed.is_some() || over.is_some() {
                    state.inside = over.is_some();
                    send(PointerAction::PointerMoved, at, None, (0.0, 0.0));
                } else if state.inside {
                    state.inside = false;
                    send(PointerAction::PointerLeft, at, None, (0.0, 0.0));
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if over.is_some() => {
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (*x, *y),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                // Наружу — в щелчках колеса, а не в пикселях сглаженного
                // потока: сколько в щелчке пикселей, знает только сглаживание
                // (см. `wheel_notch`), и владельцу это знать неоткуда. Целый
                // щелчок при этом приезжает не разом, а долями, пока гаснет
                // инерция, — их сумма и составит единицу.
                let notch = crate::module::handlers::wheel_notch();
                send(PointerAction::PointerScrolled, at, None, (dx / notch, dy / notch));
                // Иначе прокрутка уехала бы ещё и в список вокруг области.
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut GpuRenderer,
        _theme: &iced_core::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        // Место занято, а рисовать нечем — владелец ещё не выделил текстуру.
        // Пустая область честнее растянутой чужой картинки.
        if self.texture_id != 0 {
            renderer.draw_external_image(layout.bounds(), self.texture_id, true);
        }
    }
}

impl<'a> From<Viewport> for Element<'a, UiMessage, iced_core::Theme, GpuRenderer> {
    fn from(viewport: Viewport) -> Self {
        Element::new(viewport)
    }
}
