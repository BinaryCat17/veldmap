//! Поиск по каталогу: запрос, приём найденного и его контуры на глобусе.
//!
//! Здесь же проходит граница данных и представления. Провайдер отдаёт снимок с
//! его геометрией и ничего не знает о том, что её кто-то рисует; глобус
//! принимает ломаные и ничего не знает о снимках. Свести одно с другим может
//! только тот, у кого на экране и список, и шар, — то есть мы.

use crate::module::footprint;
use crate::module::state::globe::Shown;
use crate::module::state::search::{Cloud, Mission, Period, SearchState};
use crate::module::state::{State, ViewId, ViewKind};
use crate::proto::data_provider::{DataProduct, SearchRequest, SearchResponse};
use crate::proto::globe::{
    camera_command::Command, CameraCommand, Focus, GeoPoint, Outline, Outlines,
};

/// Набранное в поле запроса. Сам запрос отсюда не уходит: искать на каждую
/// букву значит слать в сеть десяток запросов на одно слово.
pub fn on_query(state: &mut State, query: String) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.query = query;
}

pub fn on_mission(state: &mut State, mission: Mission) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.mission = mission;
    // Радар про облачность не спрашивают, и его чипа на полосе нет — а
    // оставшийся от прошлой миссии потолок обнулил бы выдачу молча, потому что
    // условие по облачности отбирает снимки, у которых атрибут есть.
    if !mission.clouded() {
        search.cloud = Cloud::default();
    }
    search.listing.refine();
    run(state);
}

pub fn on_period(state: &mut State, period: Period) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.period = period;
    search.listing.refine();
    run(state);
}

pub fn on_cloud(state: &mut State, cloud: Cloud) {
    let Some((_, search)) = state.active_search_mut() else { return };
    search.cloud = cloud;
    search.listing.refine();
    run(state);
}

/// Спросить каталог.
pub fn run(state: &mut State) {
    let now = crate::module::components::format::now();
    let Some((view, search)) = state.active_search_mut() else { return };
    let request = SearchRequest {
        mission: search.mission.collection().to_string(),
        name: search.query.clone(),
        // Область контракт принимает, но задать её пока нечем: под неё нужна
        // рамка на шаре, а её в интерфейсе нет.
        area: None,
        from: search.period.since(now),
        // Верхнего края у окна нет: ищут «за последнее время», а не «в
        // промежутке» — второй край понадобится вместе с выбором дат.
        to: 0,
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
    // Выбранное переживает новый поиск, только если само в нём осталось: контур
    // ушёл с шара — значит, выбрано больше ничего. `shown` до успеха не
    // трогаем: ранний выход не должен разводить его с нарисованным.
    let selected = state
        .shown
        .as_ref()
        .filter(|shown| shown.view == view)
        .and_then(|shown| shown.selected.clone());

    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };
    let selected = selected
        .filter(|id| search.results.iter().any(|product| &product.identifier == id));

    crate::calls::globe::on_outlines(&Outlines {
        outlines: outlines_of(search, selected.as_deref()),
    });
    state.shown = Some(Shown { view, selected });

    // Наложение — тем же правилом, что выделение: его продукт ушёл из
    // выдачи — снимку больше не с чего быть на шаре. Правило действует на
    // наложения из этого же вида: показанному из каталога чужая выдача не указ.
    let alive: std::collections::HashSet<String> = {
        let Some(ViewKind::Search(search)) = state.get(view) else { return };
        search.results.iter().map(|product| product.identifier.clone()).collect()
    };
    super::overlay::keep_only(state, view, |identifier| alive.contains(identifier));
}

/// Контуры результатов; у выбранного — признак выделения.
fn outlines_of(search: &SearchState, selected: Option<&str>) -> Vec<Outline> {
    search
        .results
        .iter()
        .flat_map(|product| {
            let picked = selected == Some(product.identifier.as_str());
            product.footprint.iter().map(move |ring| Outline {
                points: ring
                    .points
                    .iter()
                    .map(|point| GeoPoint { lat: point.lat, lon: point.lon })
                    .collect(),
                selected: picked,
            })
        })
        .collect()
}

/// Вкладка-источник закрывается. Контуры остаются на шаре — они свойство
/// найденного, а не вкладки, — но выбирать среди них больше нечего: выделение
/// гаснет, а `Shown` снимается, и щелчки по шару перестают что-либо значить.
/// Иначе подсветка горела бы вечно: снять её мог только выбор в этом же виде.
/// Наложение из выдачи этого вида уходит вместе с ним: продукта, из которого
/// оно взялось, больше нет ни в одном списке (см. overlay::source_closed).
pub fn on_source_closed(state: &mut State, id: ViewId, search: &SearchState) {
    super::overlay::source_closed(state, id);
    if state.shown.as_ref().is_none_or(|shown| shown.view != id) {
        return;
    }
    state.shown = None;
    crate::calls::globe::on_outlines(&Outlines { outlines: outlines_of(search, None) });
}

/// Погасить выделение контура, не трогая сами контуры и выбор по щелчку.
/// Нужно показу по ключу (см. overlay::on_locate_result): на шар лёг другой
/// снимок, и подсвеченный контур прежнего рядом с ним — ложь о том, на что
/// смотрят.
pub fn deselect(state: &mut State) {
    let Some(shown) = &state.shown else { return };
    if shown.selected.is_none() {
        return;
    }
    let view = shown.view;
    state.shown = Some(Shown { view, selected: None });
    show_on_globe(state, view);
}

/// Выбрать снимок, накрывающий точку. `None` — щелчок пришёлся мимо Земли.
///
/// Из накрывших берём самый мелкий: одну съёмку каталог отдаёт несколькими
/// продуктами с почти одинаковым контуром, а полоса радара накрывает собой
/// целую плитку, — и «то, что помельче» единственное отвечает на вопрос «куда
/// я ткнул».
pub fn pick(state: &mut State, at: Option<(f64, f64)>) {
    let Some(view) = state.shown.as_ref().map(|shown| shown.view) else { return };

    let selected = at.and_then(|(lat, lon)| {
        let ViewKind::Search(search) = state.get(view)? else { return None };
        search
            .results
            .iter()
            .filter(|product| footprint::covers(&product.footprint, lat, lon))
            .min_by(|left, right| extent(left).total_cmp(&extent(right)))
            .map(|product| product.identifier.clone())
    });

    // Ничего не изменилось — не тревожим глобус: набор поедет тот же самый.
    if state.shown.as_ref().is_some_and(|shown| shown.selected == selected) {
        return;
    }
    state.shown = Some(Shown { view, selected });
    show_on_globe(state, view);
}

/// Показать снимок из выдачи активного поиска: выбрать его, навести на него
/// камеру, наложить его растры и открыть вкладку с шаром. `false` — активный
/// вид не поиск или продукта в выдаче нет; тогда его восстанавливает по ключу
/// провайдер (см. overlay::on_show_pressed).
pub fn show(state: &mut State, identifier: &str) -> bool {
    let Some((view, search)) = state.active_search_mut() else { return false };
    let Some(product) = search
        .results
        .iter()
        .find(|product| product.identifier == identifier)
        .cloned()
    else {
        return false;
    };

    // Наводка — при живом контуре: продукт без геометрии (вспомогательные
    // данные) камере не указ, но наложение его растров всё равно пробуем —
    // честный ответ «нет растров» придёт от провайдера.
    if let Some(frame) = footprint::frame(&product.footprint) {
        crate::calls::globe::on_camera(&CameraCommand {
            command: Some(Command::Focus(Focus {
                at: Some(GeoPoint { lat: frame.lat, lon: frame.lon }),
                radius_deg: frame.radius_deg,
            })),
        });
    }

    state.shown = Some(Shown { view, selected: Some(identifier.to_string()) });
    show_on_globe(state, view);
    super::overlay::show(state, &product, Some(view));
    super::nav::on_new_globe(state);
    true
}

/// Насколько снимок велик — угловой радиус его контура. Без геометрии он
/// бесконечен: такой не выиграет ни у одного настоящего.
fn extent(product: &DataProduct) -> f64 {
    footprint::frame(&product.footprint).map_or(f64::INFINITY, |frame| frame.radius_deg)
}
