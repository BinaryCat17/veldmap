//! Вкладка глобуса: область во весь экран и подпись под ней.
//!
//! Своего содержимого у вкладки почти нет — всё, что видно, рисует чужой
//! модуль. Здесь только место под него и то, что вокруг.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length};
use crate::module::components::{self, format};
use crate::module::state::{GlobeState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};


/// Сколько знаков имени выбранного снимка помещается в подпись. Числом, а не
/// шириной: полоса тянется вместе с окном, и мерить в ней нечего — а имя
/// продукта длиной под семьдесят знаков и без того не показать целиком.
const PICKED_CHARS: usize = 46;

pub fn view(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // Текстуру область получает от нас же: мы её выделили в ответ на
    // предыдущий on_resized. На первом кадре её ещё нет — место занимается
    // пустым, и это нормально: следующим событием оно придёт.
    let mut area = viewport::<Msg>()
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resized(move |size| Msg::In(view, ViewMsg::GlobeResized(size)))
        .on_pointer(move |pointer| Msg::In(view, ViewMsg::GlobePointer(pointer)));
    if let Some(surface) = &globe.surface {
        area = area.texture(surface.handle());
    }

    column![
        container(area).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        caption(state, view, globe),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Полоса под областью: чем управляют слева, что на шаре справа.
///
/// Названный снимок вытесняет размер места: пока не выбрано и не наложено
/// ничего, размер — то единственное, что можно сказать о безымянной области,
/// а как только снимок назван, он и есть ответ на вопрос «на что я смотрю».
/// Рядом с именем — обратный ход: с шара к строке продукта в каталоге, а у
/// наложений «снять» — разом все. По одному их снимают в «На просмотре»; здесь
/// кнопка на случай, когда той вкладки не открыто.
fn caption(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // Что назвать: выбранный контур, а без него — верхний слой на шаре. Обычно
    // это один и тот же продукт; расходятся они, когда щёлкнули по соседнему
    // контуру, и тогда именем отвечает выбор — он свежее.
    //
    // Верхний из тех, что видно: слоёв бывает несколько, назвать одной строкой
    // можно только один, а собирающийся или скрытый назвал бы то, чего на шаре
    // нет.
    let subject: Option<(&str, &str)> = state
        .picked()
        .map(|product| (product.name.as_str(), product.identifier.as_str()))
        .or_else(|| {
            state
                .overlays
                .iter()
                .rev()
                .find(|overlay| overlay.on_globe() && !overlay.hidden)
                .map(|overlay| (overlay.label.as_str(), overlay.identifier.as_str()))
        });

    let mut trailing: Vec<Element<Msg>> = Vec::new();
    match subject {
        Some((name, identifier)) => {
            trailing.push(
                mono::<Msg>(format::ellipsize(name, PICKED_CHARS))
                    .size(theme::TEXT_SMALL)
                    .color(theme::INK_SOFT)
                    .single_line()
                    .into(),
            );
            trailing.push(theme::bar_button("В каталоге").on_press(Msg::In(view, ViewMsg::Enter(components::folder_of(identifier).to_string()))).into());
        }
        None => {
            let label = match &globe.surface {
                Some(surface) => format!("{}×{}", surface.width, surface.height),
                None => "область ещё не размечена".to_string(),
            };
            trailing.push(
                text::<Msg>(label).size(theme::TEXT_LABEL).color(theme::INK_FAINT).single_line().into(),
            );
        }
    }
    // Контуры считаются наравне с растрами: очерченный шар — тоже «что-то на
    // нём лежит», и снимать это надо тем же рычагом.
    if !state.overlays.is_empty() || state.shown.is_some() {
        trailing.push(theme::bar_button("Снять с шара").on_press(Msg::GlobeClear).into());
    }

    let mut line = row![
        text::<Msg>("Тащите — вращает, колесо — приближает, щелчок — выбирает".to_string())
            .size(theme::TEXT_LABEL)
            .color(theme::INK_DIM)
            .single_line(),
        theme::spacer(),
    ];
    for element in trailing {
        line = line.push(element);
    }

    theme::chrome_bar(
        line.spacing(crate::module::view::BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}
