//! popover.rs — панель, всплывающая у своего якоря (см. `Popover` в types.proto).
//!
//! Своего виджета в iced для этого нет, а собрать меню из уже имеющихся не
//! получается: любая панель, положенная в разметку рядом с кнопкой, занимает
//! место у соседей и режется ближайшей прокруткой — а меню строки списка живёт
//! как раз внутри прокрутки. Поэтому панель уезжает в overlay: там она рисуется
//! после всей разметки, ножницами предков не ограничена и в раскладке места не
//! занимает. Тем же механизмом в iced сделана подсказка.
//!
//! Открытость сюда приходит готовой: состояние меню принадлежит клиенту, а
//! сервис только рисует. Обратно уезжает единственное событие — «щёлкнули мимо
//! панели», без которого открытое меню нечем закрыть.

use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::overlay;
use iced_core::renderer;
use iced_core::widget::{self, Widget};
use iced_core::{Clipboard, Element, Event, Point, Rectangle, Shell, Size, Vector};

use crate::module::delegate::delegate_to_child;

/// Каким краем панель равняется на якорь.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Левым к левому — меню под кнопкой слева.
    Start,
    /// Правым к правому — меню под кнопкой справа.
    End,
}

pub struct Popover<'a, Message, Theme, Renderer> {
    anchor: Element<'a, Message, Theme, Renderer>,
    panel: Element<'a, Message, Theme, Renderer>,
    open: bool,
    align: Align,
    gap: f32,
    on_dismiss: Option<Message>,
}

impl<'a, Message, Theme, Renderer> Popover<'a, Message, Theme, Renderer> {
    pub fn new(
        anchor: impl Into<Element<'a, Message, Theme, Renderer>>,
        panel: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Self {
            anchor: anchor.into(),
            panel: panel.into(),
            open: false,
            align: Align::Start,
            gap: 0.0,
            on_dismiss: None,
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Popover<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced_core::Renderer,
{
    // Дети — оба: у панели своё состояние (позиция прокрутки, каретка), и
    // жить оно должно между кадрами, пока меню открыто.
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.anchor), widget::Tree::new(&self.panel)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.anchor.as_widget(), self.panel.as_widget()]);
    }

    // Ниже — всё, что относится к месту в разметке: это место якоря, панели
    // здесь нет вовсе. Её показывает `overlay`, и он единственный знает про
    // второго ребёнка.
    delegate_to_child!(anchor: Message, Theme, Renderer;
        size, size_hint, layout, update, mouse_interaction, draw, operate);

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let mut children = tree.children.iter_mut();
        let anchor_tree = children.next().expect("дети заведены в children()");
        let panel_tree = children.next().expect("дети заведены в children()");

        // Оверлей якоря (у него самого может быть свой) не теряется: закрытое
        // меню не должно отменять чужое открытое.
        let anchor = self.anchor.as_widget_mut().overlay(anchor_tree, layout, renderer, viewport, translation);

        let panel = self.open.then(|| {
            overlay::Element::new(Box::new(Panel {
                panel: &mut self.panel,
                tree: panel_tree,
                anchor: layout.bounds() + translation,
                align: self.align,
                gap: self.gap,
                on_dismiss: self.on_dismiss.clone(),
            }))
        });

        match (anchor, panel) {
            (None, None) => None,
            (anchor, panel) => Some(
                overlay::Group::with_children(anchor.into_iter().chain(panel).collect()).overlay(),
            ),
        }
    }
}

impl<'a, Message, Theme, Renderer> From<Popover<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced_core::Renderer + 'a,
{
    fn from(popover: Popover<'a, Message, Theme, Renderer>) -> Self {
        Element::new(popover)
    }
}

/// Сама панель на время, пока она открыта.
struct Panel<'a, 'b, Message, Theme, Renderer> {
    panel: &'b mut Element<'a, Message, Theme, Renderer>,
    tree: &'b mut widget::Tree,
    /// Место якоря в окне — от него панель и отсчитывается.
    anchor: Rectangle,
    align: Align,
    gap: f32,
    on_dismiss: Option<Message>,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Panel<'_, '_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced_core::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.panel.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let size = node.size();

        let x = match self.align {
            Align::Start => self.anchor.x,
            Align::End => self.anchor.x + self.anchor.width - size.width,
        };
        // Вниз, а если снизу не помещается — вверх: панель у нижней строки
        // списка иначе уехала бы за край окна и стала недоступной.
        let below = self.anchor.y + self.anchor.height + self.gap;
        let y = if below + size.height <= bounds.height {
            below
        } else {
            (self.anchor.y - self.gap - size.height).max(0.0)
        };

        node.move_to(Point::new(
            x.clamp(0.0, (bounds.width - size.width).max(0.0)),
            y,
        ))
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        // Щелчок мимо панели закрывает меню и дальше не идёт: иначе он бы
        // заодно нажал то, что под ним, — а пользователь целился в «закрыть».
        //
        // Сюда приходит и щелчок по самому якорю: панель видит события раньше
        // него. Меню от этого закрывается — то есть повторное нажатие на чип
        // делает ровно то, чего от него ждут, только закрывает его не
        // переключатель на стороне клиента, а вот этот отказ.
        if let (Event::Mouse(mouse::Event::ButtonPressed(_)), Some(dismiss)) = (event, &self.on_dismiss)
            && cursor.position_over(layout.bounds()).is_none()
        {
            shell.publish(dismiss.clone());
            shell.capture_event();
            return;
        }

        self.panel.as_widget_mut().update(
            self.tree, event, layout, cursor, renderer, clipboard, shell,
            &Rectangle::with_size(Size::INFINITE),
        );
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.panel.as_widget().draw(
            self.tree, renderer, theme, style, layout, cursor,
            &Rectangle::with_size(Size::INFINITE),
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.panel.as_widget().mouse_interaction(
            self.tree, layout, cursor, &Rectangle::with_size(Size::INFINITE), renderer,
        )
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        // Своё место панель объявляет прежде, чем пустить обход внутрь: в
        // разметке её нет, и снаружи о ней не знает никто, а нажать под ней
        // нельзя — щелчок мимо панели закрывает её, а не жмёт лежащее под ней.
        operation.custom(None, layout.bounds(), &mut crate::module::locate::Covered);
        self.panel.as_widget_mut().operate(self.tree, layout, renderer, operation);
    }
}
