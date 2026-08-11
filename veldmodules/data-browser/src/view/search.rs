//! view/search.rs — поиск снимков.
//!
//! Экран тот же, что у каталога и скачанного, и отличается двумя вещами:
//! источником строк и полосой запроса над ними. Полоса нужна только здесь —
//! остальные виды показывают то, что у них уже есть, а этот сначала спрашивает.

use veld_ui_service_wrap::row;
use crate::proto::ui_service::{
    container, icon, text, text_input, Alignment, Element, FontWeight, Length, Padding,
};
use crate::module::components::{format, list_screen, Row, Screen};
use crate::module::state::search::MISSIONS;
use crate::module::state::{SearchState, State};
use crate::module::{theme, Msg};

const GLYPH_SEARCH: &str = "\u{f002}";

pub fn view(state: &State, search: &SearchState) -> Element<Msg> {
    let rows: Vec<Row> = search
        .results
        .iter()
        .map(|product| Row {
            kind: product.product_type.clone(),
            ..Row::remote(
                &state.library,
                product.identifier.clone(),
                product.name.clone(),
                product.size,
                product.acquired,
            )
        })
        .collect();

    list_screen::view(
        Screen {
            title: "Поиск снимков",
            subtitle: subtitle(search, rows.len()),
            path: None,
            empty: "Ничего не нашлось — попробуйте другую миссию или часть имени",
            controls: Some(bar(search)),
            rows,
        },
        &search.listing,
        state.logical_width(),
    )
}

/// Строка под заголовком: сколько нашлось и насколько облачно. Облачность
/// диапазоном, потому что своей колонки у неё нет, а разброс по выдаче — то
/// единственное, что о ней стоит сказать одной строкой.
fn subtitle(search: &SearchState, found: usize) -> String {
    if let Some(error) = &search.error {
        return error.clone();
    }
    if search.request.is_pending() {
        return "идёт поиск…".to_string();
    }

    let count = format!("{} {}", found, format::plural(found, ["снимок", "снимка", "снимков"]));
    let clouds: Vec<f64> = search.results.iter().filter_map(|product| product.cloud_cover).collect();
    match (clouds.iter().cloned().fold(f64::MAX, f64::min), clouds.iter().cloned().fold(0.0, f64::max)) {
        (low, high) if !clouds.is_empty() => format!("{}, облачность {:.0}–{:.0}%", count, low, high),
        _ => count,
    }
}

/// Полоса запроса: часть имени и миссия. Отправляют её и ввод, и выбор миссии —
/// поиск идёт по сети, и делать его на каждую букву нельзя.
fn bar(search: &SearchState) -> Element<Msg> {
    let field = container(
        row![
            icon::<Msg>(GLYPH_SEARCH).size(11.0).color(theme::INK_FAINT),
            text_input::<Msg>("Часть имени снимка; пусто — самое свежее", &search.query)
                .style(theme::field())
                .font_family(veld_ui_service_wrap::style::FONT_UI)
                .size(theme::TEXT_BODY)
                .padding(Padding::ZERO)
                .width(Length::Fill)
                .on_input(Msg::SearchQuery)
                .on_submit(Msg::RunSearch),
        ]
        .spacing(8.0)
        .width(Length::Fill)
        .align_items(Alignment::Center),
    )
    .style(theme::control_box())
    .width(Length::Fill)
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 });

    let mut controls: Vec<Element<Msg>> = vec![field.into()];
    for (value, label) in MISSIONS {
        let chosen = search.mission == value;
        controls.push(
            theme::surface_button(
                text::<Msg>(label.to_string())
                    .size(theme::TEXT_LABEL)
                    .weight(if chosen { FontWeight::WeightBold } else { FontWeight::WeightNormal })
                    .single_line(),
                chosen,
            )
            .height(Length::Fixed(theme::CONTROL_HEIGHT))
            .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
            .on_press(Msg::SearchMission(value.to_string()))
            .into(),
        );
    }

    row(controls)
        .spacing(7.0)
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .padding(Padding { top: 0.0, bottom: 10.0, left: theme::GUTTER, right: theme::GUTTER })
        .into()
}
