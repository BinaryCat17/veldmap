//! delegate.rs — проброс `Widget` единственному ребёнку обёртки.

/// Методы `iced_core::Widget`, которые обёртка отдаёт своему ребёнку как есть.
///
/// Обёртке — взятому, зоне, поповеру — принадлежит не место в разметке, а
/// поведение: ребёнок у неё один, своей раскладки нет, места она занимает
/// ровно столько же, сколько он, и всё, ради чего её завели, — это реакция на
/// события и то, что дорисовано поверх. Остальные методы трейта у таких
/// обёрток поэтому дословно совпадают, а написанные по разу на виджет заводят
/// места, которые расходятся молча: забытый `operate` сборку не ломает, он
/// просто перестаёт находить поле ввода внутри.
///
/// Макрос, а не общий тип-обёртка: своё у каждой обёртки — целые методы
/// трейта, и подставить их в чужую реализацию нечем.
///
/// Пробрасывается только перечисленное — что у виджета своё, тот пишет рядом
/// руками, и молча общего не получает. Ребёнок в дереве состояния всегда
/// первый: `children` отсюда кладёт его единственным, а кто заводит детей сам,
/// ставит пробрасываемого первым.
///
/// Типы сообщения, темы и рендерера называются так, как они названы в
/// impl-блоке: у одних обёрток они конкретные, у других — параметры.
macro_rules! delegate_to_child {
    ($child:ident: $message:ty, $theme:ty, $renderer:ty; $($method:tt),+ $(,)?) => {
        $( delegate_to_child!(@method $method, $child: $message, $theme, $renderer); )+
    };

    (@method children, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn children(&self) -> Vec<iced_core::widget::Tree> {
            vec![iced_core::widget::Tree::new(&self.$child)]
        }
    };

    (@method diff, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn diff(&self, tree: &mut iced_core::widget::Tree) {
            tree.diff_children(&[self.$child.as_widget()]);
        }
    };

    (@method size, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn size(&self) -> iced_core::Size<iced_core::Length> {
            self.$child.as_widget().size()
        }
    };

    (@method size_hint, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn size_hint(&self) -> iced_core::Size<iced_core::Length> {
            self.$child.as_widget().size_hint()
        }
    };

    (@method layout, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn layout(
            &mut self,
            tree: &mut iced_core::widget::Tree,
            renderer: &$renderer,
            limits: &iced_core::layout::Limits,
        ) -> iced_core::layout::Node {
            self.$child.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
        }
    };

    (@method update, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn update(
            &mut self,
            tree: &mut iced_core::widget::Tree,
            event: &iced_core::Event,
            layout: iced_core::layout::Layout<'_>,
            cursor: iced_core::mouse::Cursor,
            renderer: &$renderer,
            clipboard: &mut dyn iced_core::Clipboard,
            shell: &mut iced_core::Shell<'_, $message>,
            viewport: &iced_core::Rectangle,
        ) {
            self.$child.as_widget_mut().update(
                &mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    };

    (@method mouse_interaction, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn mouse_interaction(
            &self,
            tree: &iced_core::widget::Tree,
            layout: iced_core::layout::Layout<'_>,
            cursor: iced_core::mouse::Cursor,
            viewport: &iced_core::Rectangle,
            renderer: &$renderer,
        ) -> iced_core::mouse::Interaction {
            self.$child.as_widget().mouse_interaction(
                &tree.children[0], layout, cursor, viewport, renderer,
            )
        }
    };

    (@method draw, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn draw(
            &self,
            tree: &iced_core::widget::Tree,
            renderer: &mut $renderer,
            theme: &$theme,
            style: &iced_core::renderer::Style,
            layout: iced_core::layout::Layout<'_>,
            cursor: iced_core::mouse::Cursor,
            viewport: &iced_core::Rectangle,
        ) {
            self.$child.as_widget().draw(
                &tree.children[0], renderer, theme, style, layout, cursor, viewport,
            );
        }
    };

    (@method operate, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn operate(
            &mut self,
            tree: &mut iced_core::widget::Tree,
            layout: iced_core::layout::Layout<'_>,
            renderer: &$renderer,
            operation: &mut dyn iced_core::widget::Operation,
        ) {
            self.$child.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
        }
    };

    (@method overlay, $child:ident: $message:ty, $theme:ty, $renderer:ty) => {
        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut iced_core::widget::Tree,
            layout: iced_core::layout::Layout<'b>,
            renderer: &$renderer,
            viewport: &iced_core::Rectangle,
            translation: iced_core::Vector,
        ) -> Option<iced_core::overlay::Element<'b, $message, $theme, $renderer>> {
            self.$child.as_widget_mut().overlay(
                &mut tree.children[0], layout, renderer, viewport, translation,
            )
        }
    };
}

pub(crate) use delegate_to_child;
