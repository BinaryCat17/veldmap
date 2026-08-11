//! Вкладка глобуса: область во весь экран и подпись под ней.
//!
//! Своего содержимого у вкладки почти нет — всё, что видно, рисует чужой
//! модуль. Здесь только место под него и то, что вокруг.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, text, Alignment, Element, Length, Padding};
use crate::module::state::{GlobeState, State};
use crate::module::{theme, Msg};

/// Высота строки под областью. Фиксирована, как и весь хром: содержимое не
/// вправе её растянуть.
const CAPTION_HEIGHT: f32 = 30.0;

pub fn view(_state: &State, globe: &GlobeState) -> Element<Msg> {
    // Текстуру область получает от нас же: мы её выделили в ответ на
    // предыдущий on_resized. На первом кадре её ещё нет — место занимается
    // пустым, и это нормально: следующим событием оно придёт.
    let mut area = viewport::<Msg>()
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resized(Msg::GlobeResized)
        .on_pointer(Msg::GlobePointer);
    if let Some(surface) = &globe.surface {
        area = area.texture(surface.handle());
    }

    column![
        container(area).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        caption(globe),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn caption(globe: &GlobeState) -> Element<Msg> {
    let label = match &globe.surface {
        Some(surface) => format!("{}×{}", surface.width, surface.height),
        None => "область ещё не размечена".to_string(),
    };

    container(
        row![
            text::<Msg>("Тащите — вращает, колесо — приближает".to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK_DIM)
                .single_line(),
            container(veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(0.0)))
                .width(Length::Fill),
            text::<Msg>(label).size(theme::TEXT_LABEL).color(theme::INK_FAINT).single_line(),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .align_items(Alignment::Center)
        .padding(Padding { top: 0.0, bottom: 0.0, left: 14.0, right: 14.0 }),
    )
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(CAPTION_HEIGHT))
    .into()
}
