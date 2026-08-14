//! drag.rs — перенос мышью: что берут и куда кладут (см. `Drag` и `Drop` в
//! types.proto).
//!
//! Весь перенос ведёт сервис, а владельцу разметки уезжает одно событие — то,
//! чем он кончился. Иначе и не выходит: пока вкладку несут, всё, что
//! происходит, — это «курсор над той зоной, а теперь над этой», а где какая
//! зона, знает только тот, кто раскладывал. Владелец её геометрии не видит и
//! видеть не должен.
//!
//! Отсюда и общее состояние на весь сервис: взявший знает нагрузку, а
//! показывает и принимает её другой виджет — тот, над которым курсор. Мышь
//! одна, поэтому и несомое одно; хранить его у кого-то из двоих нельзя — они
//! друг про друга не знают вовсе.

use std::sync::Mutex;

use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::overlay;
use iced_core::renderer::{self, Renderer as _};
use iced_core::widget::{self, Widget};
use iced_core::{
    Clipboard, Color, Element, Event, Length, Point, Rectangle, Shell, Size, Vector,
};

use crate::module::converter::UiMessage;
use crate::module::renderer::GpuRenderer;
use crate::proto::ui_service::{DropEdge, DropEvent};

/// Насколько надо увести курсор с нажатого места, чтобы это считалось
/// переносом. Без порога всякий щелчок начинался бы переносом и подсвечивал
/// зоны под курсором — на глаз это дрожь интерфейса от простого нажатия.
const THRESHOLD: f32 = 4.0;

/// Какая часть зоны у края считается краем. Меньше — в край не попасть, больше
/// — середины не останется.
///
/// Проверяется это глазом, прогоном по сценарию: нативных тестов у сервиса нет
/// — его сборка под хост тянет оконный бэкенд iced, а он собирается только под
/// настоящее окно.
const EDGE: f32 = 0.28;

/// Что сейчас несут. Одно на весь сервис: мышь одна, и двух переносов сразу не
/// бывает (см. заголовок файла).
static CARRIED: Mutex<Option<String>> = Mutex::new(None);

fn carried() -> Option<String> {
    CARRIED.lock().ok().and_then(|carried| carried.clone())
}

fn take_up(payload: String) {
    if let Ok(mut carried) = CARRIED.lock() {
        *carried = Some(payload);
    }
}

/// Перенос кончился. Зовётся не виджетом, а кадровым циклом — после того, как
/// отпускание разошлось по всей разметке: взявший и принявший видят одно и то
/// же событие, и порядок между ними не определён. Сбрось несомое любой из них
/// сам — второму досталось бы «никто ничего не несёт».
pub fn drop_carried() {
    if let Ok(mut carried) = CARRIED.lock() {
        *carried = None;
    }
}

// -- Взятое --

pub struct Draggable {
    content: Element<'static, UiMessage, iced_core::Theme, GpuRenderer>,
    payload: String,
}

/// Нажатое место — от него меряется порог. `None` — за коробку не держатся.
#[derive(Default)]
struct Held {
    origin: Option<Point>,
    carrying: bool,
}

impl Draggable {
    pub fn new(
        content: Element<'static, UiMessage, iced_core::Theme, GpuRenderer>,
        payload: String,
    ) -> Self {
        Self { content, payload }
    }
}

impl Widget<UiMessage, iced_core::Theme, GpuRenderer> for Draggable {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Held>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(Held::default())
    }

    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &GpuRenderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &GpuRenderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiMessage>,
        viewport: &Rectangle,
    ) {
        // Содержимое видит событие целиком: взятая вкладка остаётся кнопкой, и
        // щелчок по ней по-прежнему её выбирает. Перенос отличает от щелчка
        // только порог — до него не происходит ничего.
        self.content.as_widget_mut().update(
            &mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport,
        );

        if self.payload.is_empty() {
            return;
        }
        let held = tree.state.downcast_mut::<Held>();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(at) = cursor.position_over(layout.bounds()) else { return };
                *held = Held { origin: Some(at), carrying: false };
            }
            // Место курсора берётся из самого события, а не у `cursor`:
            // взятое обычно лежит в прокручиваемой полосе, а она объявляет
            // курсор недоступным своим детям, едва он уйдёт за её край, — то
            // есть ровно тогда, когда перенос и начинается.
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(origin) = held.origin else { return };
                if !held.carrying && position.distance(origin) >= THRESHOLD {
                    held.carrying = true;
                    take_up(self.payload.clone());
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                *held = Held::default();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &GpuRenderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0], layout, cursor, viewport, renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut GpuRenderer,
        theme: &iced_core::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0], renderer, theme, style, layout, cursor, viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &GpuRenderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &GpuRenderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, UiMessage, iced_core::Theme, GpuRenderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0], layout, renderer, viewport, translation,
        )
    }
}

impl From<Draggable> for Element<'static, UiMessage, iced_core::Theme, GpuRenderer> {
    fn from(draggable: Draggable) -> Self {
        Element::new(draggable)
    }
}

// -- Место, куда кладут --

pub struct DropZone {
    content: Element<'static, UiMessage, iced_core::Theme, GpuRenderer>,
    method: String,
    key: String,
    edges: bool,
    highlight: Color,
}

impl DropZone {
    pub fn new(
        content: Element<'static, UiMessage, iced_core::Theme, GpuRenderer>,
        method: String,
        key: String,
        edges: bool,
        highlight: Color,
    ) -> Self {
        Self { content, method, key, edges, highlight }
    }

    /// В какую часть зоны попал курсор.
    fn edge(&self, bounds: Rectangle, at: Point) -> DropEdge {
        if !self.edges || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return DropEdge::DropCenter;
        }
        let left = (at.x - bounds.x) / bounds.width;
        let above = (at.y - bounds.y) / bounds.height;
        let (right, below) = (1.0 - left, 1.0 - above);

        // Ближайший край — если курсор вообще в его полосе; иначе середина.
        let nearest = left.min(right).min(above).min(below);
        if nearest > EDGE {
            return DropEdge::DropCenter;
        }
        if nearest == left {
            DropEdge::DropLeft
        } else if nearest == right {
            DropEdge::DropRight
        } else if nearest == above {
            DropEdge::DropAbove
        } else {
            DropEdge::DropBelow
        }
    }

    /// Место, которое займёт брошенное: середина — вся зона, край — её
    /// половина с этой стороны. Ровно то, что человек увидит после броска,
    /// поэтому подсветка и есть обещание.
    fn target(bounds: Rectangle, edge: DropEdge) -> Rectangle {
        let (half_w, half_h) = (bounds.width / 2.0, bounds.height / 2.0);
        match edge {
            DropEdge::DropCenter => bounds,
            DropEdge::DropLeft => Rectangle { width: half_w, ..bounds },
            DropEdge::DropRight => {
                Rectangle { x: bounds.x + half_w, width: half_w, ..bounds }
            }
            DropEdge::DropAbove => Rectangle { height: half_h, ..bounds },
            DropEdge::DropBelow => {
                Rectangle { y: bounds.y + half_h, height: half_h, ..bounds }
            }
        }
    }
}

impl Widget<UiMessage, iced_core::Theme, GpuRenderer> for DropZone {
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &GpuRenderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &GpuRenderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiMessage>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport,
        );

        if !matches!(event, Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))) {
            return;
        }
        let Some(payload) = carried() else { return };
        let Some(at) = cursor.position_over(layout.bounds()) else { return };

        shell.publish(UiMessage::dropped(
            self.method.clone(),
            self.key.clone(),
            DropEvent {
                payload,
                edge: self.edge(layout.bounds(), at) as i32,
            },
        ));
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &GpuRenderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0], layout, cursor, viewport, renderer,
        )
    }

    /// Поверх содержимого — обещание: место, которое займёт брошенное.
    /// Рисуется, только пока что-то несут: в покое зона ничем не отличается от
    /// обычной коробки, и знать о ней заранее незачем.
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut GpuRenderer,
        theme: &iced_core::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0], renderer, theme, style, layout, cursor, viewport,
        );

        if carried().is_none() {
            return;
        }
        let Some(at) = cursor.position_over(layout.bounds()) else { return };
        let target = Self::target(layout.bounds(), self.edge(layout.bounds(), at));
        renderer.fill_quad(renderer::Quad { bounds: target, ..Default::default() }, self.highlight);
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &GpuRenderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &GpuRenderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, UiMessage, iced_core::Theme, GpuRenderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0], layout, renderer, viewport, translation,
        )
    }
}

impl From<DropZone> for Element<'static, UiMessage, iced_core::Theme, GpuRenderer> {
    fn from(zone: DropZone) -> Self {
        Element::new(zone)
    }
}
