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

/// Полоса под областью: чем управляют слева, что на шаре справа.
///
/// Названный снимок вытесняет размер места: пока не выбрано и не наложено
/// ничего, размер — то единственное, что можно сказать о безымянной области,
/// а как только снимок назван, он и есть ответ на вопрос «на что я смотрю».
/// Рядом с именем — обратный ход: с шара к строке продукта в каталоге, а у
/// наложения ещё и «снять» — единственное место, где показанное из каталога
/// можно убрать (показанное из поиска уходит и со своей выдачей).
fn caption(state: &State, globe: &GlobeState) -> Element<Msg> {
    // Что назвать: выбранный контур, а без него — наложенный снимок. Обычно
    // это один и тот же продукт; расходятся они, когда щёлкнули по соседнему
    // контуру, и тогда именем отвечает выбор — он свежее.
    let subject: Option<(&str, &str)> = state
        .picked()
        .map(|product| (product.name.as_str(), product.identifier.as_str()))
        .or_else(|| {
            state
                .overlay
                .as_ref()
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
            trailing.push(bar_button("В каталоге", Msg::Enter(folder_of(identifier))));
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
    if state.overlay.is_some() {
        trailing.push(bar_button("Снять с шара", Msg::GlobeClear));
    }

    let mut line = row![
        text::<Msg>("Тащите — вращает, колесо — приближает, щелчок — выбирает".to_string())
            .size(theme::TEXT_LABEL)
            .color(theme::INK_DIM)
            .single_line(),
        container(veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(0.0)))
            .width(Length::Fill),
    ];
    for element in trailing {
        line = line.push(element);
    }

    container(
        line.spacing(10.0)
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

/// Кнопка в полосе: подпись и действие, по росту полосы.
fn bar_button(label: &str, message: Msg) -> Element<Msg> {
    theme::surface_button(
        text::<Msg>(label.to_string()).size(theme::TEXT_SMALL).single_line(),
        false,
    )
    .padding(Padding { top: 3.0, bottom: 3.0, left: 8.0, right: 8.0 })
    .on_press(message)
    .into()
}

/// Папка, в которой лежит продукт, — туда ведёт «В каталоге»: там его строка.
/// То же правило, что у меню строки (см. components::row::Row::folder).
fn folder_of(identifier: &str) -> String {
    let path = identifier.trim_end_matches('/');
    match path.rfind('/') {
        Some(cut) => path[..cut].to_string(),
        None => String::new(),
    }
}
