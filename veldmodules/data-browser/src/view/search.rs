//! view/search.rs — поиск снимков.
//!
//! Экран тот же, что у каталога и скачанного, и отличается двумя вещами:
//! источником строк и полосой запроса над ними. Полоса нужна только здесь —
//! остальные виды показывают то, что у них уже есть, а этот сначала спрашивает.

use veld_ui_service_wrap::row;
use crate::proto::ui_service::{Alignment, Element, Length, Padding};
use crate::module::components::{controls, format, list_screen, Row, Screen};
use crate::module::state::listing::Menu;
use crate::module::state::{SearchState, State};
use crate::module::{theme, Msg};


pub fn view(state: &State, search: &SearchState) -> Element<Msg> {
    let rows: Vec<Row> = search
        .results
        .iter()
        .map(|product| Row {
            kind: product.product_type.clone(),
            located: !product.footprint.is_empty(),
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
            // Пока ответа нет, «ничего не нашлось» — неправда: ещё ищем.
            empty: match search.request.is_pending() {
                true => "Спрашиваем каталог…",
                false => "Ничего не нашлось — попробуйте другую миссию или часть имени",
            },
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

/// Полоса запроса: часть имени, миссия, окно съёмки и облачность. Отправляет
/// запрос всё, кроме самого ввода: поиск идёт по сети, и делать его на каждую
/// букву нельзя — поле ждёт Enter.
///
/// Облачность спрашивается не всегда: у радара её нет, и условие по ней вернуло
/// бы пусто (см. [`Mission::clouded`]). Рычаг, который может только обнулить
/// выдачу, лучше не показывать вовсе.
fn bar(search: &SearchState) -> Element<Msg> {
    // Ввод сам по себе ничего не запускает: запрос идёт по сети, поле ждёт
    // Enter (`on_submit`).
    let field = controls::search_field(
        "Часть имени снимка; пусто — самое свежее",
        &search.query,
        Msg::SearchQuery,
        Some(Msg::RunSearch),
    );

    let opened = &search.listing.menu;
    let mut controls: Vec<Element<Msg>> = vec![
        field,
        controls::chip("Миссия:", search.mission, Menu::Mission, opened, &[], Msg::SearchMission),
        controls::chip("Съёмка:", search.period, Menu::Period, opened, &[], Msg::SearchPeriod),
    ];
    if search.mission.clouded() {
        controls.push(
            controls::chip("Облачность:", search.cloud, Menu::Cloud, opened, &[], Msg::SearchCloud),
        );
    }

    row(controls)
        .spacing(7.0)
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .padding(Padding { top: 0.0, bottom: 10.0, left: theme::GUTTER, right: theme::GUTTER })
        .into()
}
