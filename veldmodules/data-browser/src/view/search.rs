//! view/search.rs — поиск снимков.
//!
//! Экран тот же, что у каталога и скачанного, и отличается двумя вещами:
//! источником строк и полосой запроса над ними. Полоса нужна только здесь —
//! остальные виды показывают то, что у них уже есть, а этот сначала спрашивает.

use crate::proto::ui_service::Element;
use crate::module::components::{controls, format, list_screen, rows, Row, Screen};
use crate::module::state::listing::Menu;
use crate::module::state::search::Period;
use crate::module::state::{SearchState, State, ViewId};
use crate::module::{Msg, ViewMsg};


pub fn view(state: &State, view: ViewId, search: &SearchState) -> Element<Msg> {
    // В выдаче поиска снимок — каждая строка: каталог отвечает продуктами, а не
    // файлами (сборка строк — в components::rows).
    let rows: Vec<Row> = rows::search(state, search);

    list_screen::view(
        view,
        Screen {
            title: "Поиск снимков",
            picked: state.picked_key(),
            outlined: state.outlined_in(&search.listing),
            subtitle: subtitle(search, rows.len()),
            path: None,
            // Пока ответа нет, «ничего не нашлось» — неправда: ещё ищем.
            empty: match search.request.is_pending() {
                true => "Спрашиваем каталог…",
                false => "Ничего не нашлось — попробуйте другую миссию или часть имени",
            },
            controls: Some(bar(view, search, state.pane_width(view))),
            rows,
        },
        &search.listing,
        state.pane_width(view),
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
fn bar(view: ViewId, search: &SearchState, width: f32) -> Element<Msg> {
    // Ввод сам по себе ничего не запускает: запрос идёт по сети, поле ждёт
    // Enter (`on_submit`).
    let field = controls::search_field(
        "Часть имени снимка; пусто — самое свежее",
        &search.query,
        move |query| Msg::In(view, ViewMsg::SearchQuery(query)),
        Some(Msg::In(view, ViewMsg::RunSearch)),
    );

    let opened = &search.listing.menu;
    let mut controls = vec![
        controls::chip(view, "Миссия:", search.mission, Menu::Mission, opened, &[], move |choice| {
            Msg::In(view, ViewMsg::SearchMission(choice))
        }),
        controls::chip(view, "Съёмка:", search.period, Menu::Period, opened, &[], move |choice| {
            Msg::In(view, ViewMsg::SearchPeriod(choice))
        }),
    ];
    // Поля дат — только у своего интервала: у пресета краёв не спрашивают, и
    // пустые поля рядом с «за неделю» обещали бы выбор, которого нет.
    if search.period == Period::Custom {
        controls.push(controls::date_field("с", &search.from, move |value| {
            Msg::In(view, ViewMsg::SearchFrom(value))
        }, Msg::In(view, ViewMsg::RunSearch)));
        controls.push(controls::date_field("по", &search.to, move |value| {
            Msg::In(view, ViewMsg::SearchTo(value))
        }, Msg::In(view, ViewMsg::RunSearch)));
    }
    if search.mission.clouded() {
        controls.push(
            controls::chip(view, "Облачность:", search.cloud, Menu::Cloud, opened, &[], move |choice| {
                Msg::In(view, ViewMsg::SearchCloud(choice))
            }),
        );
    }

    controls::bar(width, field, controls)
}
