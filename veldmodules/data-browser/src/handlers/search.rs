//! Поиск по каталогу: запрос, приём найденного и его контуры на глобусе.
//!
//! Здесь же проходит граница данных и представления. Провайдер отдаёт снимок с
//! его геометрией и ничего не знает о том, что её кто-то рисует; глобус
//! принимает ломаные и ничего не знает о снимках. Свести одно с другим может
//! только тот, у кого на экране и список, и шар, — то есть мы.

use crate::module::state::{State, ViewId, ViewKind};
use crate::proto::data_provider::{SearchRequest, SearchResponse};
use crate::proto::globe::{GeoPoint, Outline, Outlines};

/// Набранное в поле запроса. Сам запрос отсюда не уходит: искать на каждую
/// букву значит слать в сеть десяток запросов на одно слово.
pub fn on_query(state: &mut State, query: String) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.query = query;
}

pub fn on_mission(state: &mut State, mission: String) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.mission = mission;
    run(state);
}

/// Спросить каталог.
pub fn run(state: &mut State) {
    let Some((view, search)) = state.active_search_mut() else { return };
    let request = SearchRequest {
        mission: search.mission.clone(),
        name: search.query.clone(),
        // Область, время и облачность контракт принимает, но задать их пока
        // нечем: под них нужна карта с рамкой и выбор дат, а их в разметке нет.
        // Без них каталог отдаёт самое свежее — см. `$orderby` у провайдера.
        area: None,
        from: 0,
        to: 0,
        max_cloud: None,
        limit: 0,
    };
    search.error = None;
    let correlation_id = search.request.begin();
    state.searches.insert(correlation_id.clone(), view);

    crate::calls::data_provider::on_search(&request, &correlation_id);
}

/// Результат поиска. Чей он — знает таблица маршрутов; свой устаревший
/// (запрос успели сменить) отбрасываем так же, как чужой.
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.searches.take(&correlation_id) else { return };
    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };

    if search.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        search.error = Some(response.error);
        search.results = Vec::new();
    } else {
        search.error = None;
        search.results = response.products;
    }

    show_on_globe(state, view);
}

/// Отправляет глобусу контуры найденного.
///
/// Целиком, а не добавкой: набор у глобуса заменяется полностью (см. `Outlines`
/// в его types.proto), и пустой список — это «ничего не нашлось», то есть тоже
/// осмысленный ответ, а не повод промолчать.
///
/// Открыта ли вкладка глобуса, здесь не проверяется, и знать этого не нужно:
/// контуры — свойство найденного, а не экрана. Глобус их запомнит и покажет,
/// когда ему дадут место.
fn show_on_globe(state: &mut State, view: ViewId) {
    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };

    let outlines = search
        .results
        .iter()
        .flat_map(|product| &product.footprint)
        .map(|ring| Outline {
            points: ring
                .points
                .iter()
                .map(|point| GeoPoint { lat: point.lat, lon: point.lon })
                .collect(),
        })
        .collect();

    crate::calls::globe::on_outlines(&Outlines { outlines });
}
