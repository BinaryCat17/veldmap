//! Выбор строк, контуры на шаре и то, чем они друг другу не приходятся.
//!
//! Здесь проходит граница данных и представления. Провайдер отдаёт снимок с
//! его геометрией и ничего не знает о том, что её кто-то рисует; глобус
//! принимает ломаные и ничего не знает о снимках. Свести одно с другим может
//! только тот, у кого на экране и список, и шар, — то есть мы.
//!
//! Состояний у строки три, и они независимы: **выбор** коробочкой — набор для
//! пакетных действий; **контур** — очерчен ли снимок на шаре; **показ** —
//! лежит ли он там растром (см. handlers::overlay). Сведи любые два в одно —
//! и одно начнёт отвечать за другое: снятый контур уносил бы набранное к
//! удалению, а показ клал бы снимок в состав пакетного действия, которого
//! никто не просил.
//!
//! Контуры одни на приложение, а не на вкладку: шар один, а списков сколько
//! угодно, и «контуры этой вкладки» на вопрос «что на шаре» не отвечает.
//! Выбор, наоборот, свой у каждого списка — им и действуют над списком.
//!
//! Геометрия при этом под рукой не у всех: у найденного продукт с контуром уже
//! есть, а у строки каталога или файла на диске — один ключ, и продукт по нему
//! восстанавливает провайдер (`on_locate`). Ответы кэшируются: ключ строки не
//! меняется, а ход к каталогу сетевой.

use crate::module::components::{arrange, rows};
use crate::module::footprint;
use crate::module::state::globe::Outlined;
use crate::module::components::Row;
use crate::module::state::listing::Chosen;
use crate::module::state::overlay::Pace;
use crate::module::state::{Highlight, Locate, Located, State, ViewId, ViewKind};
use crate::proto::data_provider::{DataProduct, LocateRequest, LocateResponse};
use crate::proto::globe::{GeoPoint, Outline, OutlineStyle, Outlines};

/// Выбрать строку в списке или снять выбор.
///
/// Шара это не касается вовсе: очерчивает [`toggle_outline`], кладёт растром
/// показ, а выбор — набор для пакетных действий над списком.
pub fn toggle(state: &mut State, view: ViewId, key: String) {
    let what = chosen_by_key(state, view, &key);
    let Some(listing) = state.listing_mut(view) else { return };
    listing.select(key, what);
}

/// Очертить снимок на шаре или убрать контур.
///
/// Ключ здесь всегда ключ снимка: контур бывает у него одного, а геометрию под
/// этим ключом и спрашивают у каталога.
pub fn toggle_outline(state: &mut State, key: String) {
    // Не спросившееся переспрашиваем, а не снимаем: просьба уже записана, и
    // жмут её именно затем, чтобы попробовать ещё раз. Снять её можно тем же
    // значком, когда ответ придёт, или «Снять с шара».
    if state.outlines.contains(&key) && matches!(state.located.get(&key), Some(Located::Failed)) {
        state.located.remove(&key);
        refresh(state);
        return;
    }
    if !state.outlines.remove(&key) {
        retry(state, &key);
        state.outlines.insert(key);
    }
    refresh(state);
}

/// Что стои́т за строкой — род её выбора.
///
/// Ключи файла снимаются со строки здесь, а не в момент действия: выбор
/// переживает переход в другую папку, и к тому времени строки на экране может
/// не быть вовсе.
fn chosen(row: &Row) -> Chosen {
    match row.is_snapshot() {
        true => Chosen::Snapshot,
        false => Chosen::File {
            identifier: row.snapshot_key().to_string(),
            product: row.product.clone(),
            name: row.name.clone(),
        },
    }
}

/// То же по ключу: строку под ним ищем среди показанных — там же, где её и
/// нажали.
///
/// Не нашлась — считаем снимком: род по одному ключу не восстановить. Такое
/// бывает, когда список пересобрала рассылка библиотеки, пока ехало нажатие, —
/// она приходит по десятку раз в секунду, пока что-нибудь качается.
fn chosen_by_key(state: &State, view: ViewId, key: &str) -> Chosen {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else {
        return Chosen::Snapshot;
    };
    match arrange::arrange(&rows, listing).marks().find(|row| row.choice_key() == key) {
        Some(row) => chosen(row),
        None => Chosen::Snapshot,
    }
}

/// Попросить заново — это и переспросить: сорвавшийся ход к каталогу просьбу
/// не переживает (см. [`Located::Failed`]). Ответ каталога — «нет такого» —
/// переживает: он не устаревает.
fn retry(state: &mut State, key: &str) {
    if matches!(state.located.get(key), Some(Located::Failed)) {
        state.located.remove(key);
    }
}

/// Отметить или снять отметку разом со всего, что показано, — коробочка в
/// шапке.
///
/// Набор берётся тот же, о котором она и говорит (`Arranged::marks`): страница
/// со всем раскрытым на ней. Второго определения «что видно» здесь нет
/// намеренно — разойдясь с первым, коробочка обещала бы одно, а делала другое.
pub fn mark_shown(state: &mut State, view: ViewId, on: bool) {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else { return };
    let picked: Vec<(String, Chosen)> = arrange::arrange(&rows, listing)
        .marks()
        .map(|row| (row.choice_key().to_string(), chosen(row)))
        .collect();

    let Some(listing) = state.listing_mut(view) else { return };
    for (key, what) in picked {
        match on {
            true => {
                listing.selected.insert(key, what);
            }
            false => {
                listing.selected.remove(&key);
            }
        }
    }
}

/// Снять весь выбор этого списка — кнопка в заголовке.
///
/// Весь, а не показанный: выбор переживает переход в другую папку, и «снять
/// видимое» оставило бы набранным то, чего на экране уже нет, — а считаться в
/// заголовке оно продолжало бы.
pub fn unmark_all(state: &mut State, view: ViewId) {
    let Some(listing) = state.listing_mut(view) else { return };
    if listing.selected.is_empty() {
        return;
    }
    listing.selected.clear();
}

/// Убрать один контур — из списка «На просмотре», где он стоит своей строкой.
///
/// Выбор в списках это не трогает: коробочка про другое.
pub fn drop_one(state: &mut State, key: &str) {
    if !state.outlines.remove(key) {
        return;
    }
    refresh(state);
}

/// Навести шар на снимок и выбрать его: обвести лентой, назвать полосой под
/// шаром и перевести взгляд туда.
///
/// Один обработчик на контур и на слой: вопрос у них один — «покажи мне, где
/// это», — и два ответа на него разошлись бы тем, что у слоя выбор не
/// зажигался бы. Именно выбирает, а не гасит выбор: сюда идут за местом, и
/// названо должно быть то, к чему привели.
///
/// Куда смотреть, спрашивается у того, у кого это точнее: у контура —
/// нарисованная геометрия, у слоя — рамка, посчитанная в момент показа. Ход к
/// каталогу за наводкой не делается: это сетевой запрос за тем, что уже
/// известно.
pub fn focus(state: &mut State, key: &str) {
    let frame = state
        .outlined
        .iter()
        .find(|outlined| outlined.key == key)
        .and_then(|outlined| footprint::frame(&outlined.rings))
        .or_else(|| {
            state
                .overlays
                .iter()
                .find(|overlay| overlay.identifier == key)
                .and_then(|overlay| overlay.focus.clone())
        });
    super::globe::focus_on(frame);
    select(state, Some(key.to_string()));
    super::nav::on_new_globe(state);
}

/// Выбрать контур — или снять выбор. Единственное место, где выбор меняется по
/// воле человека, и потому же здесь гаснет подсветка перехода: подсвечена на
/// экране одна строка, и выбор на шаре — свежий ответ на тот же вопрос
/// (см. [`Highlight`]).
///
/// Глобусу при этом уезжает весь набор контуров: о выборе ему говорит он же —
/// а неизменившийся набор до шины не доходит.
fn select(state: &mut State, key: Option<String>) {
    if state.picked_key() == key.as_deref().unwrap_or_default() {
        return;
    }
    match key {
        // Щелчок мимо контуров гасит ленту — и только её. Отметку перехода
        // ставил не он, живёт она до ухода из папки, и снимать её заодно
        // значило бы убрать со экрана строку, к которой только что привели
        // (`State::deselect` для того и написан).
        None => {
            state.deselect();
        }
        Some(key) => state.highlight = Some(Highlight { key, view: None, on_globe: true }),
    }
    send(state);
}

/// Снять контуры с шара. Выбора в списках это не касается: коробочка про
/// другое, и набранное к удалению кнопка про шар выбрасывать не должна.
pub fn clear(state: &mut State) {
    // Следов у очерченного три — просьба, набор контуров и лента на шаре, — и
    // снимаются они вместе. Уйти по первому же «контуров не было» нельзя:
    // лента переживает контур, снятый руками, и строка осталась бы подсвеченной
    // как лежащая на шаре, которого на ней уже нет.
    let unmarked = !state.outlines.is_empty();
    state.outlines.clear();
    let unpicked = state.deselect();
    let outlined = !state.outlined.is_empty();
    state.outlined.clear();
    if unmarked || unpicked || outlined {
        send(state);
    }
}

/// Погасить выбор контура, не трогая сами контуры. Нужно показу снимка на шаре
/// (см. overlay): на шар лёг другой снимок, и подсвеченный контур прежнего
/// рядом с ним — ложь о том, на что смотрят.
pub fn deselect(state: &mut State) {
    if !state.deselect() {
        return;
    }
    send(state);
}

/// Выбрать снимок, накрывающий точку. `None` — щелчок пришёлся мимо Земли.
///
/// Из накрывших берём самый мелкий: одну съёмку каталог отдаёт несколькими
/// продуктами с почти одинаковым контуром, а полоса радара накрывает собой
/// целую плитку, — и «то, что помельче» единственное отвечает на вопрос «куда
/// я ткнул».
pub fn pick(state: &mut State, at: Option<(f64, f64)>) {
    let chosen = at.and_then(|(lat, lon)| {
        state
            .outlined
            .iter()
            .filter(|outlined| footprint::covers(&outlined.rings, lat, lon))
            .min_by(|left, right| extent(left).total_cmp(&extent(right)))
            .map(|outlined| outlined.key.clone())
    });
    select(state, chosen);
}

/// Продукт по ключу приехал — или не приехал.
///
/// Три ответа, и запоминаются они по-разному: продукт живёт до конца запуска,
/// «такого нет» тоже (оно не устареет), а «не спросилось» — только до
/// следующей отметки той же строки. Свалив последнее в «такого нет», мы
/// хоронили бы снимок из-за одной сетевой заминки.
pub fn located(state: &mut State, key: String, response: LocateResponse) {
    let answer = match (response.product, response.answered) {
        (Some(product), _) => Located::Found(product),
        (None, true) => Located::Missing,
        (None, false) => {
            veldsdk::log::warn!(target: "handlers", "контур '{}': {}", key, response.error);
            state.notice = Some(format!("Контур не спросился: {}", response.error));
            Located::Failed
        }
    };
    state.located.insert(key, answer);
    refresh(state);
}

/// Пересобрать набор контуров из [`State::outlines`] и отправить его глобусу.
///
/// Зовётся всякий раз, когда меняется просьба или то, из чего берётся
/// геометрия: набор у глобуса заменяется целиком (см. `Outlines` в его
/// types.proto), и другого способа сказать ему про контуры нет. Спрашивать
/// перед отправкой «а изменился ли он» не нужно — топик объявлен снимком, и
/// неизменившийся набор до шины не доходит.
pub fn refresh(state: &mut State) {
    let mut wanted: Vec<Outlined> = Vec::new();
    let mut ask: Vec<String> = Vec::new();
    let mut cache: Vec<(String, DataProduct)> = Vec::new();
    for key in &state.outlines {
        match product(state, key) {
            Known::Have(found) => {
                // Продукт из выдачи поиска кладём в кэш: выдача сменится, а
                // контур переживёт её — и без кэша он моргнул бы, уехав в
                // каталог за геометрией, которая только что была в руках.
                //
                // Пустую геометрию кэшируем тоже: выдача и `on_locate` идут к
                // одному источнику, и второй ход принёс бы ту же пустоту —
                // только секундой позже и по сети.
                if !state.located.contains_key(key) {
                    cache.push((key.clone(), found.clone()));
                }
                if !found.footprint.is_empty() {
                    wanted.push(Outlined {
                        key: key.clone(),
                        label: found.name.clone(),
                        folder: found.folder,
                        rings: found.footprint.clone(),
                    });
                }
            }
            Known::Ask => ask.push(key.clone()),
            // Ответ либо ещё в пути, либо был и рисовать по нему нечего.
            Known::Nothing => {}
        }
    }
    for (key, found) in cache {
        state.located.insert(key, Located::Found(found));
    }
    // Порядок множества случаен, а набор уезжает целиком: без этого один и тот
    // же набор выглядел бы новым на каждую пересборку.
    wanted.sort_by(|left, right| left.key.cmp(&right.key));

    for key in ask {
        // Отметка «спрашиваем» ставится здесь же: со следующей пересборки этот
        // ключ уже известен, и второй раз в каталог не поедет (см. [`Located`]).
        state.located.insert(key.clone(), Located::Asking);
        let correlation = state.locates.begin(Locate::Outline(key.clone()));
        crate::calls::data_provider::on_locate(&LocateRequest { identifier: key }, &correlation);
    }

    state.outlined = wanted;
    forget_gone(state);
    send(state);
}

/// Погасить ленту, если снимка, который она обводит, на шаре больше нет.
///
/// Выбранным остаётся всё, что на шаре хоть как-то есть: контур — им лента и
/// держится, слой — он и без ленты назван полосой под шаром, и гасить выбор
/// снимка, который на шаре виден, значило бы отвечать «ни на что не смотрим»,
/// глядя прямо на него. Пропало и то, и другое — выбирать больше нечего.
///
/// Зовётся отовсюду, откуда снимок с шара уходит: снятая отметка, снятый слой,
/// сменившаяся выдача. Второе такое правило по месту разошлось бы с этим, и
/// строка осталась бы подсвеченной как лежащая на шаре, которого на ней нет.
pub fn forget_gone(state: &mut State) {
    let picked = state.picked_key();
    if picked.is_empty() {
        return;
    }
    let outlined = state.outlined.iter().any(|outlined| outlined.key == picked);
    let laid = state.overlays.iter().any(|overlay| overlay.identifier == picked);
    if !outlined && !laid {
        state.deselect();
    }
}

/// Что известно про геометрию очерчиваемого снимка.
enum Known<'a> {
    Have(&'a DataProduct),
    /// Продукта под рукой нет — надо спросить провайдера.
    Ask,
    /// Рисовать нечего и спрашивать нечего: ответ либо ещё в пути, либо был и
    /// оказался пуст, либо не вышло спросить. Различать их здесь не по чему —
    /// набор контуров от этого не меняется.
    Nothing,
}

/// Продукт, из которого берётся контур: в выдаче поиска он под рукой, у
/// прочих списков — только в ответах провайдера.
///
/// Ищется по ключу во всех выдачах сразу, а не в той вкладке, где ключ
/// отмечен: один и тот же снимок отмечают и в каталоге, и в выдаче, а продукт
/// с геометрией лежит только во второй. Спрашивай мы по вкладке — вторая
/// отметка гасила бы уже нарисованный контур и слала бы в каталог запрос за
/// тем, что лежит в соседнем виде.
fn product<'a>(state: &'a State, key: &str) -> Known<'a> {
    let found = state.views().iter().find_map(|view| match &view.kind {
        ViewKind::Search(search) => {
            search.results.iter().find(|product| product.identifier == key)
        }
        _ => None,
    });
    if let Some(found) = found {
        return Known::Have(found);
    }
    match state.located.get(key) {
        Some(Located::Found(found)) => Known::Have(found),
        Some(Located::Asking | Located::Missing | Located::Failed) => Known::Nothing,
        None => Known::Ask,
    }
}

/// Отправить глобусу весь набор. Единственный способ сказать ему про контуры —
/// отсюда и одна точка вызова на каждое изменение, своё и чужое: набор
/// наложений меняет этот тоже (см. `overlay::send_set`).
pub fn send(state: &State) {
    crate::calls::globe::on_outlines(&Outlines { outlines: drawn(state) });
}

/// Из чего складывается набор: очерченное по просьбе и место того, что на шар
/// ещё едет. Отдельно от отправки, потому что это единственное, что здесь
/// можно проверить, не заводя шины.
fn drawn(state: &State) -> Vec<Outline> {
    let picked = state.picked_key();
    let mut outlines: Vec<Outline> = Vec::new();
    for outlined in &state.outlined {
        let style = match picked == outlined.key {
            true => OutlineStyle::OutlineSelected,
            false => OutlineStyle::OutlinePlain,
        };
        rings(&mut outlines, &outlined.rings, style);
    }
    for key in under_way(state) {
        // Очерченный по просьбе второй раз не рисуется: контур у снимка один,
        // и две ломаные по одному месту сложились бы в одну лишнюю линию.
        // Просьба при этом старше: она пережила бы приезд снимка, а штрих —
        // нет.
        if state.outlined.iter().any(|outlined| outlined.key == key) {
            continue;
        }
        let Known::Have(found) = product(state, &key) else { continue };
        rings(&mut outlines, &found.footprint, OutlineStyle::OutlinePending);
    }
    outlines
}

/// Кольца одной геометрии — контурами названного вида.
fn rings(outlines: &mut Vec<Outline>, geometry: &[crate::proto::data_provider::Ring], style: OutlineStyle) {
    outlines.extend(geometry.iter().map(|ring| Outline {
        points: ring.points.iter().map(|point| GeoPoint { lat: point.lat, lon: point.lon }).collect(),
        style: style as i32,
    }));
}

/// Снимки, которые на шар только едут: показ попросили, а картинки ещё нет.
///
/// Их место и заштриховано на шаре. Без этого нажатие на значок глобуса
/// не отвечает ничем: между просьбой и первой картинкой лежат десятки секунд —
/// ход к каталогу, открытие растров, описание по сети, — и всё это время на
/// шаре не появляется ровно ничего.
///
/// Двумя источниками, потому что просьба живёт в двух положениях: пока
/// восстанавливают продукт по ключу ([`State::showing`]) и пока наложение уже
/// заведено. А кончается штрих не с ними, а с первой картинкой, и спрашивается
/// это одним общим правилом ([`rows::onto_globe`]) — тем же, которым полоса под
/// строкой выбирает между долей и «идёт, а доли нет». Вопрос у них один:
/// видно ли уже хоть что-нибудь. Своего ответа здесь быть не может — он
/// разошёлся бы с полосой, и снимок стоял бы очерченным поверх собственной
/// картинки.
fn under_way(state: &State) -> Vec<String> {
    let laid = |key: &String| state.overlays.iter().any(|overlay| &overlay.identifier == key);
    state
        .overlays
        .iter()
        .map(|overlay| overlay.identifier.clone())
        // Просьба под ключом заведённого слоя не бывает — её снимает тот же
        // ответ, что слой заводит, — но держится это правилом в соседнем
        // обработчике, а два одинаковых кольца по одному месту сложились бы в
        // лишнюю линию.
        .chain(state.showing.iter().filter(|key| !laid(key)).cloned())
        .filter(|key| matches!(rows::onto_globe(state, key).pace(), Pace::Unknown))
        .collect()
}

/// Насколько снимок велик — угловой радиус его контура. Без геометрии он
/// бесконечен: такой не выиграет ни у одного настоящего.
fn extent(outlined: &Outlined) -> f64 {
    footprint::frame(&outlined.rings).map_or(f64::INFINITY, |frame| frame.radius_deg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::handlers::browse;
    use crate::module::state::BrowseState;
    use crate::proto::data_provider::{GeoPoint as ProductPoint, Ring};

    fn state() -> State {
        State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние")
    }

    /// Снимок с квадратным контуром 10..20 по обеим осям.
    fn square(key: &str) -> Outlined {
        Outlined {
            key: key.to_string(),
            label: key.to_string(),
            folder: false,
            rings: vec![Ring {
                points: [(10.0, 10.0), (10.0, 20.0), (20.0, 20.0), (20.0, 10.0)]
                    .into_iter()
                    .map(|(lat, lon)| ProductPoint { lat, lon })
                    .collect(),
            }],
        }
    }

    /// Снимок, который на шар только едет, очерчен штрихом — и перестаёт быть
    /// очерченным, как только доехал.
    ///
    /// Без этого нажатие на значок глобуса не отвечает ничем: между просьбой и
    /// первой картинкой лежат десятки секунд, и всё это время на шаре не видно
    /// даже места, куда снимок ляжет.
    #[test]
    fn a_snapshot_on_its_way_is_outlined_with_a_hatch() {
        use crate::proto::data_provider::DataProduct;
        let key = "eodata/store/A.SAFE";
        let product = DataProduct {
            identifier: key.to_string(),
            footprint: square(key).rings,
            ..Default::default()
        };

        let mut state = state();
        state.located.insert(key.to_string(), Located::Found(product.clone()));
        assert!(drawn(&state).is_empty(), "неспрошенное на шаре не рисуется");

        // Продукт восстанавливают по ключу — картинки нет, а место уже видно.
        state.showing.insert(key.to_string());
        let styles: Vec<i32> = drawn(&state).iter().map(|outline| outline.style).collect();
        assert_eq!(styles, vec![OutlineStyle::OutlinePending as i32]);

        // Наложение завелось, растров ещё нет — штрих тот же.
        state.showing.remove(key);
        crate::module::handlers::overlay::show(&mut state, &product, None);
        let styles: Vec<i32> = drawn(&state).iter().map(|outline| outline.style).collect();
        assert_eq!(styles, vec![OutlineStyle::OutlinePending as i32]);

        // Растры уехали глобусу, но он их ещё описывает — видно по-прежнему
        // ничего, и штрих на месте. Это и есть самая долгая часть пути: у
        // растра по сети она длится десятки секунд.
        if let Some(overlay) = state.overlays.first_mut() {
            overlay.rasters = vec![Default::default()];
            overlay.progress = crate::module::state::overlay::Progress {
                working: true,
                blank: true,
                ..Default::default()
            };
        }
        let styles: Vec<i32> = drawn(&state).iter().map(|outline| outline.style).collect();
        assert_eq!(styles, vec![OutlineStyle::OutlinePending as i32], "штрих снят раньше картинки");

        // Растр описан — по нему уже есть что нарисовать, и место держать
        // больше нечем: снимок виден сам, хотя добыча ещё идёт.
        if let Some(overlay) = state.overlays.first_mut() {
            overlay.progress = crate::module::state::overlay::Progress {
                working: true,
                blank: false,
                ..Default::default()
            };
        }
        assert!(drawn(&state).is_empty(), "штрих пережил приезд картинки");
    }

    /// Очерченный по просьбе рисуется один раз, даже когда он же едет на шар:
    /// контур у снимка один, и вторая ломаная по тому же месту — лишняя линия.
    #[test]
    fn an_asked_outline_is_not_doubled_by_the_hatch() {
        let key = "eodata/store/A.SAFE";
        let mut state = state();
        state.outlined = vec![square(key)];
        state.showing.insert(key.to_string());
        state.located.insert(
            key.to_string(),
            Located::Found(crate::proto::data_provider::DataProduct {
                identifier: key.to_string(),
                footprint: square(key).rings,
                ..Default::default()
            }),
        );

        let styles: Vec<i32> = drawn(&state).iter().map(|outline| outline.style).collect();
        assert_eq!(styles, vec![OutlineStyle::OutlinePlain as i32], "контур нарисован дважды");

        // Выбранный — лентой, и штрих её не подменяет.
        state.highlight =
            Some(Highlight { key: key.to_string(), view: None, on_globe: true });
        let styles: Vec<i32> = drawn(&state).iter().map(|outline| outline.style).collect();
        assert_eq!(styles, vec![OutlineStyle::OutlineSelected as i32]);
    }

    /// Подсвечена на экране одна строка, и владелец у подсветки один: щелчок по
    /// шару гасит подсветку перехода, а переход к другой строке — ленту на
    /// шаре. Без этого правила подсвеченными остаются обе — та, к которой
    /// привели, и та, которую выбрали после.
    #[test]
    fn the_highlight_has_one_owner() {
        let mut state = state();
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
        state.outlined = vec![square("eodata/store/A.SAFE")];
        let elsewhere = "eodata/store/B.SAFE".to_string();

        browse::reveal(&mut state, view, elsewhere.clone());
        assert_eq!(state.target_in(view), elsewhere);

        pick(&mut state, Some((15.0, 15.0)));
        assert_eq!(state.picked_key(), "eodata/store/A.SAFE");
        assert_eq!(state.target_in(view), "", "подсветка перехода пережила выбор на шаре");

        browse::reveal(&mut state, view, elsewhere);
        assert_eq!(state.picked_key(), "", "лента на шаре пережила переход к другой строке");
    }

    /// Род выбора снимается со строки, а не назначается умолчанием.
    ///
    /// Снимок и файл сам по себе выбираются одной коробочкой, а пакетное
    /// действие делает с ними разное: снимок разворачивают в его файлы, файл
    /// адресуют собственными ключами. Записанный не тем родом, он получит
    /// чужие пакетные кнопки.
    #[test]
    fn the_checkbox_remembers_what_the_row_was() {
        use crate::proto::data_library::{LibraryEntry, LibraryStatus};

        let lib = |name: &str, product: &str, identifier: &str| LibraryEntry {
            name: name.to_string(),
            identifier: identifier.to_string(),
            product: product.to_string(),
            done: 1,
            total: 1,
            status: LibraryStatus::LibComplete as i32,
            modified: 0,
            siblings: 1,
            trouble: String::new(),
        };

        let mut state = state();
        state.library.entries = vec![
            lib("B1.TIF", "eodata/store/S2B_X.SAFE", "eodata/store/S2B_X.SAFE/B1.TIF"),
            lib("dem.tif", "", "eodata/dem/dem.tif"),
        ];
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Downloaded(Default::default()));

        toggle(&mut state, view, "eodata/store/S2B_X.SAFE".to_string());
        toggle(&mut state, view, "eodata/dem/dem.tif".to_string());

        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert!(
            matches!(listing.selected.get("eodata/store/S2B_X.SAFE"), Some(Chosen::Snapshot)),
            "снимок записан не снимком"
        );
        match listing.selected.get("eodata/dem/dem.tif") {
            Some(Chosen::File { identifier, name, .. }) => {
                assert_eq!(identifier, "eodata/dem/dem.tif", "ключ файла нужен закачке");
                assert_eq!(name, "dem.tif", "имя записи нужно удалению");
            }
            _ => panic!("файл записан не файлом"),
        }

        // Второе нажатие той же коробочкой снимает выбор.
        toggle(&mut state, view, "eodata/dem/dem.tif".to_string());
        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert!(!listing.selected.contains_key("eodata/dem/dem.tif"));
    }

    /// Коробочка в шапке берёт показанные строки — и только их, и только в
    /// выбор: шара она не касается так же, как и коробочка строки.
    #[test]
    fn the_header_box_takes_the_shown_rows_into_the_choice_only() {
        let mut state = state();
        state.library.entries = vec![crate::proto::data_library::LibraryEntry {
            name: "dem.tif".to_string(),
            identifier: "eodata/dem/dem.tif".to_string(),
            product: String::new(),
            done: 1,
            total: 1,
            status: crate::proto::data_library::LibraryStatus::LibComplete as i32,
            modified: 0,
            siblings: 1,
            trouble: String::new(),
        }];
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Downloaded(Default::default()));

        mark_shown(&mut state, view, true);
        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert_eq!(listing.selected.len(), 1, "показанная строка не выбрана");
        assert!(state.outlines.is_empty(), "коробочка шапки полезла на шар");

        mark_shown(&mut state, view, false);
        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert!(listing.selected.is_empty(), "второе нажатие не сняло выбор");
    }

    /// Граница выбора и шара — в обе стороны.
    ///
    /// «Снять с шара» не должно выбрасывать набранное к удалению, а «снять
    /// выбор» — уносить контуры: это два независимых состояния, и кнопка
    /// каждого отвечает только за своё.
    #[test]
    fn the_globe_and_the_choice_do_not_clear_each_other() {
        let mut state = state();
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));

        state.outlines.insert("eodata/store/A.SAFE".to_string());
        state.outlined = vec![square("eodata/store/A.SAFE")];
        toggle(&mut state, view, "eodata/store/dem.tif".to_string());

        clear(&mut state);
        assert!(state.outlines.is_empty() && state.outlined.is_empty(), "контуры не сняты");
        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert_eq!(listing.selected.len(), 1, "«снять с шара» выбросило набранное");

        state.outlines.insert("eodata/store/A.SAFE".to_string());
        unmark_all(&mut state, view);
        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert!(listing.selected.is_empty(), "выбор не снят");
        assert_eq!(state.outlines.len(), 1, "«снять выбор» унесло контур");
    }

    /// Не спросившийся контур нажатием переспрашивают, а не снимают: просьба
    /// уже записана, и жмут её именно затем, чтобы попробовать ещё раз.
    #[test]
    fn a_failed_ask_is_retried_not_dropped() {
        let mut state = state();
        let key = "eodata/store/A.SAFE".to_string();
        state.outlines.insert(key.clone());
        state.located.insert(key.clone(), Located::Failed);

        toggle_outline(&mut state, key.clone());
        assert!(state.outlines.contains(&key), "просьба снята вместо переспроса");
        assert!(matches!(state.located.get(&key), Some(Located::Asking)), "не переспросили");

        // А не спросившийся, которого никто не просил, ставится заново — и тем
        // же движением переспрашивается.
        let other = "eodata/store/B.SAFE".to_string();
        state.located.insert(other.clone(), Located::Failed);
        toggle_outline(&mut state, other.clone());
        assert!(state.outlines.contains(&other));
        assert!(matches!(state.located.get(&other), Some(Located::Asking)));
    }

    /// Геометрия из выдачи поиска переживает саму выдачу.
    ///
    /// Без кэша контур моргнул бы на смене выдачи и уехал в каталог за тем,
    /// что секунду назад было под рукой.
    #[test]
    fn geometry_from_the_search_survives_it() {
        use crate::module::state::SearchState;

        let mut state = state();
        let key = "eodata/store/A.SAFE".to_string();
        let pane = state.focused();
        let found = DataProduct {
            identifier: key.clone(),
            name: "A.SAFE".to_string(),
            footprint: vec![Ring {
                points: [(10.0, 10.0), (10.0, 20.0), (20.0, 20.0), (20.0, 10.0)]
                    .into_iter()
                    .map(|(lat, lon)| ProductPoint { lat, lon })
                    .collect(),
            }],
            ..Default::default()
        };
        let view = state.open_in(
            pane,
            ViewKind::Search(SearchState { results: vec![found], ..Default::default() }),
        );

        state.outlines.insert(key.clone());
        refresh(&mut state);
        assert_eq!(state.outlined.len(), 1, "контур из выдачи не нарисован");
        assert!(matches!(state.located.get(&key), Some(Located::Found(_))), "геометрия не в кэше");

        // Выдача сменилась — контур остался, и в каталог за ним не поехали.
        if let Some(ViewKind::Search(search)) = state.get_mut(view) {
            search.results.clear();
        }
        refresh(&mut state);
        assert_eq!(state.outlined.len(), 1, "контур моргнул вместе с выдачей");
        assert!(matches!(state.located.get(&key), Some(Located::Found(_))), "поехали спрашивать");
    }

    /// Коробочка контура не рисует и в каталог не ходит: очерчивает свой
    /// значок, и это разные состояния строки.
    ///
    /// Ход по сети на каждую выбранную строку был бы особенно обиден там, ради
    /// чего выбор и заведён: пакетом их набирают десятками, чтобы удалить.
    #[test]
    fn the_checkbox_neither_draws_nor_asks() {
        let mut state = state();
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
        let key = "eodata/store/A.SAFE".to_string();

        toggle(&mut state, view, key.clone());
        refresh(&mut state);
        assert!(state.outlined.is_empty(), "у выбранного нашёлся контур");
        assert!(state.located.is_empty(), "за выбранным сходили в каталог");

        // А значок контура — ходит: геометрию знает каталог, и другого места
        // взять её нет.
        toggle_outline(&mut state, key.clone());
        assert!(
            matches!(state.located.get(&key), Some(Located::Asking)),
            "за контуром в каталог не пошли"
        );

        // Второе нажатие убирает просьбу.
        toggle_outline(&mut state, key.clone());
        assert!(!state.outlines.contains(&key), "контур не снялся");
    }

    /// Слой без контура остаётся выбранным. Контур у снимка бывает не всегда —
    /// геометрию каталог знает не про всё, а отметку с него могли снять, — но
    /// на шаре он при этом лежит, и полоса под шаром называет его. Гасить такой
    /// выбор на первой же пересборке значило бы отвечать «ни на что не смотрим»,
    /// глядя прямо на снимок.
    #[test]
    fn a_layer_without_a_contour_keeps_its_selection() {
        use crate::module::state::overlay::OverlayState;

        let mut state = state();
        let key = "eodata/store/A.SAFE".to_string();
        state.overlays.push(OverlayState::new(
            key.clone(),
            "A.SAFE".to_string(),
            false,
            None,
            None,
            None,
        ));

        focus(&mut state, &key);
        assert_eq!(state.picked_key(), key, "наводка на слой его не выбрала");

        refresh(&mut state);
        assert_eq!(state.picked_key(), key, "выбор слоя погас на пересборке контуров");

        // А снимок, которого на шаре нет вовсе, выбранным не остаётся: ленте
        // не на чем держаться, и назвать его нечем.
        state.overlays.clear();
        refresh(&mut state);
        assert_eq!(state.picked_key(), "", "выбор пережил уход снимка с шара");
    }

    /// Переход к тому же снимку, что обведён на шаре, ленту не гасит: это одна
    /// и та же подсветка, и гасить её значило бы отвечать не на тот вопрос.
    /// Так и приводят к нему — полосой под самим шаром.
    #[test]
    fn walking_to_the_picked_row_keeps_its_ribbon() {
        let mut state = state();
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
        let key = "eodata/store/A.SAFE".to_string();
        state.outlined = vec![square(&key)];

        pick(&mut state, Some((15.0, 15.0)));
        assert_eq!(state.picked_key(), key);

        browse::reveal(&mut state, view, key.clone());
        assert_eq!(state.picked_key(), key, "лента погасла под собственным переходом");
        assert_eq!(state.target_in(view), key);
    }
}

