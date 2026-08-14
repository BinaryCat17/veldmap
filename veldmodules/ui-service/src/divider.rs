//! divider.rs — граница между соседями в раскладке (см. `Divider` в types.proto).
//!
//! Всё, что здесь есть, следует из одного: место соседям раздаёт владелец
//! разметки, а не эта полоса. Она умеет ровно два дела — показать, что за неё
//! можно взяться, и, пока её тянут, говорить владельцу, насколько уехал курсор.
//! Что с этим сделать, решает он.
//!
//! Виджет не обобщён по типу сообщения, в отличие от `Popover`: тот — способ
//! разложить чужие элементы и о протоколе не знает, а этот сам изготавливает
//! нагрузку события и без неё бессмыслен.

use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::renderer::{self, Renderer as _};
use iced_core::widget::{self, Widget};
use iced_core::{Clipboard, Color, Element, Event, Length, Point, Rectangle, Shell, Size};

use crate::module::converter::UiMessage;
use crate::module::renderer::GpuRenderer;
use crate::proto::ui_service::DividerAxis;

pub struct Divider {
    axis: DividerAxis,
    thickness: f32,
    line: f32,
    color: Color,
    active: Color,
    /// Имя входа владельца, куда уходит сдвиг. `None` — тянуть нельзя, полоса
    /// остаётся просто линией.
    on_drag: Option<String>,
    /// Что сказать владельцу, когда границу отпустят. Готовым сообщением, а не
    /// именем метода: нагрузку этого события изготавливает не виджет — её
    /// назвала сама разметка.
    on_release: Option<UiMessage>,
    /// Чем владелец назвал саму границу: нагрузку здесь изготавливает виджет,
    /// поэтому назвать себя граница может только отдельным полем — см.
    /// `UiEventResponse.key`.
    key: String,
}

/// Состояние между кадрами: где был курсор, когда о нём сказали в прошлый раз.
/// `None` — за границу не держатся.
///
/// Хранится именно прошлое место курсора, а не место самой границы: разметка
/// пересобирается на каждое событие, и граница успевает переехать — сдвиг,
/// отсчитанный от неё, посчитал бы этот переезд второй раз.
#[derive(Default)]
struct State {
    grabbed: Option<Point>,
}

impl Divider {
    pub fn new(axis: DividerAxis, thickness: f32, line: f32, color: Color, active: Color) -> Self {
        Self {
            axis,
            thickness,
            line,
            color,
            active,
            on_drag: None,
            on_release: None,
            key: String::new(),
        }
    }

    pub fn on_drag(mut self, method: String) -> Self {
        self.on_drag = Some(method);
        self
    }

    pub fn on_release(mut self, message: UiMessage) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn key(mut self, key: String) -> Self {
        self.key = key;
        self
    }

    /// Место всей полосы: поперёк — её ширина, вдоль — сколько дали.
    fn size(&self) -> Size<Length> {
        match self.axis {
            DividerAxis::DividerVertical => {
                Size::new(Length::Fixed(self.thickness), Length::Fill)
            }
            DividerAxis::DividerHorizontal => {
                Size::new(Length::Fill, Length::Fixed(self.thickness))
            }
        }
    }

    /// Сдвиг вдоль оси перетаскивания: поперёк неё граница не двигается вовсе.
    fn along(&self, from: Point, to: Point) -> f32 {
        match self.axis {
            DividerAxis::DividerVertical => to.x - from.x,
            DividerAxis::DividerHorizontal => to.y - from.y,
        }
    }
}

impl Widget<UiMessage, iced_core::Theme, GpuRenderer> for Divider {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        self.size()
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &GpuRenderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = self.size();
        layout::Node::new(limits.resolve(size.width, size.height, Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &GpuRenderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiMessage>,
        _viewport: &Rectangle,
    ) {
        let Some(method) = self.on_drag.clone() else { return };
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let Some(at) = cursor.position() else { return };

        match event {
            // Границу тянут левой кнопкой и только ею: правая и средняя в
            // раскладке не значат ничего, и хватать ими то, что двигает пол-окна,
            // человек не собирался.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.position_over(bounds).is_some() =>
            {
                state.grabbed = Some(at);
                // Иначе нажатие уедет ещё и в то, что под границей.
                shell.capture_event();
            }
            // Отпускание ловится по своему захвату, а не по границам: взявшись
            // за полосу, кнопку отпускают где угодно.
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.grabbed.is_some() =>
            {
                state.grabbed = None;
                if let Some(message) = self.on_release.clone() {
                    shell.publish(message);
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(was) = state.grabbed else { return };
                state.grabbed = Some(at);
                let delta = self.along(was, at);
                // Движение поперёк оси сдвига не даёт: событие о нём было бы
                // просьбой подвинуть границу на ноль.
                if delta != 0.0 {
                    shell.publish(UiMessage::text(method, self.key.clone(), delta.to_string()));
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    /// Полоса под курсором и полоса, за которую держатся, выглядят одинаково:
    /// оба состояния значат «эта граница сейчас ваша».
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut GpuRenderer,
        _theme: &iced_core::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let taken = state.grabbed.is_some() || cursor.position_over(bounds).is_some();
        let color = match taken && self.on_drag.is_some() {
            true => self.active,
            false => self.color,
        };

        // Линия рисуется посередине полосы: браться можно за всю её ширину, а
        // видно ровно ту же волосяную линию, что и между всем прочим.
        let line = self.line.min(match self.axis {
            DividerAxis::DividerVertical => bounds.width,
            DividerAxis::DividerHorizontal => bounds.height,
        });
        let visible = match self.axis {
            DividerAxis::DividerVertical => Rectangle {
                x: bounds.x + (bounds.width - line) / 2.0,
                width: line,
                ..bounds
            },
            DividerAxis::DividerHorizontal => Rectangle {
                y: bounds.y + (bounds.height - line) / 2.0,
                height: line,
                ..bounds
            },
        };
        renderer.fill_quad(renderer::Quad { bounds: visible, ..Default::default() }, color);
    }
}

impl<'a> From<Divider> for Element<'a, UiMessage, iced_core::Theme, GpuRenderer> {
    fn from(divider: Divider) -> Self {
        Element::new(divider)
    }
}
