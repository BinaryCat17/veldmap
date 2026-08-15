//! Поиск по каталогу: запрос и приём найденного.
//!
//! Контуров здесь нет: рисует шар не то, что нашлось, а то, что отметили, — и
//! сводит это в набор `handlers::outline`. Выдача с ним связана одним
//! правилом: ушедший из неё продукт уносит с собой и отметку, и наложение —
//! отмечать и показывать нечего то, чего в списке больше нет.

use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{State, ViewId, ViewKind};
use crate::proto::data_provider::{SearchRequest, SearchResponse};

/// Набранное в поле запроса. Сам запрос отсюда не уходит: искать на каждую
/// букву значит слать в сеть десяток запросов на одно слово.
pub fn on_query(state: &mut State, view: ViewId, query: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.query = query;
}

pub fn on_mission(state: &mut State, view: ViewId, mission: Mission) {
    let Some(search) = state.search_mut(view) else { return };
    search.mission = mission;
    // Радар про облачность не спрашивают, и его чипа на полосе нет — а
    // оставшийся от прошлой миссии потолок обнулил бы выдачу молча, потому что
    // условие по облачности отбирает снимки, у которых атрибут есть.
    if !mission.clouded() {
        search.cloud = Cloud::default();
    }
    state.refine(view);
    run(state, view);
}

/// Край своего интервала. Запрос отсюда не уходит: дату набирают по знаку, и
/// спрашивать каталог на каждый было бы то же, что искать на каждую букву.
pub fn on_from(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.from = value;
}

pub fn on_to(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.to = value;
}

pub fn on_period(state: &mut State, view: ViewId, period: Period) {
    let Some(search) = state.search_mut(view) else { return };
    search.period = period;
    state.refine(view);
    run(state, view);
}

pub fn on_cloud(state: &mut State, view: ViewId, cloud: Cloud) {
    let Some(search) = state.search_mut(view) else { return };
    search.cloud = cloud;
    state.refine(view);
    run(state, view);
}

/// Спросить каталог.
///
/// Страница сбрасывается здесь, а не только у чипов отбора: выдача меняется
/// целиком, и «страница 3» относилась бы к прошлой. Иначе Enter в поле имени
/// или в поле даты открывает новый ответ на старой странице — а это чужая
/// страница чужого списка (см. `ListingState::page`).
pub fn run(state: &mut State, view: ViewId) {
    state.refine(view);
    let now = crate::module::components::format::now();
    let Some(search) = state.search_mut(view) else { return };
    let (from, to) = search.window(now);
    let request = SearchRequest {
        mission: search.mission.collection().to_string(),
        name: search.query.clone(),
        // Область контракт принимает, но задать её пока нечем: под неё нужна
        // рамка на шаре, а её в интерфейсе нет.
        area: None,
        from,
        to,
        max_cloud: search.cloud.max(),
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
    // Раскрытое принадлежало прошлой выдаче: продукты в ней другие, и файлы
    // под их строками — тоже. Свернуть надо и сами строки: раскрытая с пустым
    // содержимым выглядит как снимок без файлов.
    search.children.clear();
    search.listing.expanded.clear();

    survivors(state, view);
}

/// Что пережило новую выдачу.
///
/// Правило одно на отметку и на наложение: ушедший из выдачи продукт уносит с
/// собой и контур, и снимок с шара — быть им там больше не с чего. Действует
/// оно только на своё: показанному из каталога чужая выдача не указ.
///
/// И только когда выдача есть. Отказ каталога — это отсутствие ответа, а не
/// пустой ответ: продукт от сетевой ошибки никуда не делся, и снимать из-за неё
/// то, что человек положил на шар руками, значит терять его работу.
fn survivors(state: &mut State, view: ViewId) {
    let alive: std::collections::HashSet<String> = {
        let Some(ViewKind::Search(search)) = state.get(view) else { return };
        if search.error.is_some() {
            return;
        }
        search.results.iter().map(|product| product.identifier.clone()).collect()
    };

    if let Some(listing) = state.listing_mut(view) {
        listing.selected.retain(|key| alive.contains(key));
    }
    super::overlay::keep_only(state, view, |identifier| alive.contains(identifier));
    super::outline::refresh(state);
}

/// Вкладка-источник закрывается: её отметки уходят вместе с ней — очерчивал
/// снимок этот список, и другого носителя у отметки нет. Наложение из её выдачи
/// уходит по тому же правилу (см. overlay::source_closed).
pub fn on_source_closed(state: &mut State, id: ViewId) {
    super::overlay::source_closed(state, id);
    super::outline::refresh(state);
}

/// Наложить растры снимка из выдачи названного поиска. `false` — этот вид не
/// поиск или продукта в его выдаче нет; тогда его восстанавливает по ключу
/// провайдер (см. overlay::on_show_pressed).
///
/// Именно названного, а не активного: строку могли нажать в той половине
/// экрана, что не под рукой, и продукт искать надо в её выдаче.
///
/// Наводка камеры и выбор снимка живут не здесь: они одни на все четыре места,
/// откуда снимок кладут на шар, и делает их `outline::focus` (см.
/// `overlay::on_show_pressed`).
pub fn show(state: &mut State, view: ViewId, identifier: &str) -> bool {
    let Some(search) = state.search_mut(view) else { return false };
    let Some(product) = search
        .results
        .iter()
        .find(|product| product.identifier == identifier)
        .cloned()
    else {
        return false;
    };

    // Наложение растров пробуем и без геометрии: честный ответ «нет растров»
    // придёт от провайдера, а наводка при пустом контуре просто не случится.
    super::overlay::show(state, &product, Some(view));
    true
}
