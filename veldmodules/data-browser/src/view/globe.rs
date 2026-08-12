//! Вкладка глобуса: область во весь экран и подпись под ней.
//!
//! Своего содержимого у вкладки почти нет — всё, что видно, рисует чужой
//! модуль. Здесь только место под него и то, что вокруг.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length, Padding};
use crate::module::components::format;
use crate::module::state::{GlobeState, State};
use crate::module::{theme, Msg};


/// Сколько знаков имени выбранного снимка помещается в подпись. Числом, а не
/// шириной: полоса тянется вместе с окном, и мерить в ней нечего — а имя
/// продукта длиной под семьдесят знаков и без того не показать целиком.
const PICKED_CHARS: usize = 46;

pub fn view(state: &State, globe: &GlobeState) -> Element<Msg> {
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
        caption(state, globe),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Полоса под областью: чем управляют слева, что выбрано справа.
///
/// Выбранное вытесняет размер места: пока не выбрано ничего, размер — то
/// единственное, что можно сказать о безымянной области, а как только снимок
/// назван, он и есть ответ на вопрос «на что я смотрю».
fn caption(state: &State, globe: &GlobeState) -> Element<Msg> {
    let picked: Option<Element<Msg>> = state.picked().map(|product| {
        mono::<Msg>(format::ellipsize(&product.name, PICKED_CHARS))
            .size(theme::TEXT_SMALL)
            .color(theme::INK_SOFT)
            .single_line()
            .into()
    });
    let label = match &globe.surface {
        Some(surface) => format!("{}×{}", surface.width, surface.height),
        None => "область ещё не размечена".to_string(),
    };

    container(
        row![
            text::<Msg>("Тащите — вращает, колесо — приближает, щелчок — выбирает".to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK_DIM)
                .single_line(),
            container(veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(0.0)))
                .width(Length::Fill),
            picked.unwrap_or_else(|| {
                text::<Msg>(label).size(theme::TEXT_LABEL).color(theme::INK_FAINT).single_line().into()
            }),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .align_items(Alignment::Center)
        .padding(Padding { top: 0.0, bottom: 0.0, left: 14.0, right: 14.0 }),
    )
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(theme::BAR_HEIGHT))
    .into()
}
