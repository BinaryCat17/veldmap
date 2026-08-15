//! Контуры на шаре: что отмечено в списках, то и очерчено.
//!
//! Здесь проходит граница данных и представления. Провайдер отдаёт снимок с
//! его геометрией и ничего не знает о том, что её кто-то рисует; глобус
//! принимает ломаные и ничего не знает о снимках. Свести одно с другим может
//! только тот, у кого на экране и список, и шар, — то есть мы.
//!
//! Источник у контуров ровно один — пакетное выделение: шар очерчивает то, что
//! отметили, а не то, что нашлось. Отмечают в любом списке — в выдаче поиска, в
//! сетевом каталоге, в скачанном, — и набор сводится из всех сразу: шар один, а
//! списков сколько угодно, и «контуры этой вкладки» на вопрос «что на шаре» не
//! отвечает.
//!
//! Геометрия при этом под рукой не у всех: у найденного продукт с контуром уже
//! есть, а у строки каталога или файла на диске — один ключ, и продукт по нему
//! восстанавливает провайдер (`on_locate`). Ответы кэшируются: ключ строки не
//! меняется, а ход к каталогу сетевой.

use std::collections::HashSet;

use crate::module::components::{arrange, rows};
use crate::module::footprint;
use crate::module::state::globe::Outlined;
use crate::module::state::{Highlight, Locate, Located, State, ViewId, ViewKind};
use crate::proto::data_provider::{DataProduct, LocateRequest, LocateResponse};
use crate::proto::globe::{GeoPoint, Outline, Outlines};

/// Отметить снимок в списке или снять отметку.
pub fn toggle(state: &mut State, view: ViewId, key: String) {
    retry(state, &key);
    let Some(listing) = state.listing_mut(view) else { return };
    listing.select(key);
    refresh(state);
}

/// Отметить, если ещё не отмечен. В отличие от [`toggle`] — не переключатель:
/// зовёт это показ снимка на шаре, а его просят у отмеченного и у
/// неотмеченного одинаково, и снять отметку вторым нажатием было бы ответом не
/// на тот вопрос.
///
/// Вид без списка (полоса шара, «На просмотре») отмечать нечем — там и нечего:
/// снимок туда попал уже отмеченным.
pub fn mark(state: &mut State, view: ViewId, key: &str) {
    retry(state, key);
    let Some(listing) = state.listing_mut(view) else { return };
    if !listing.selected.insert(key.to_string()) {
        return;
    }
    refresh(state);
}

/// Отметить заново — это и переспросить: сорвавшийся ход к каталогу метку не
/// переживает (см. [`Located::Failed`]). Ответ каталога — «нет такого» — метку
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
    let keys: Vec<String> =
        arrange::arrange(&rows, listing).marks().map(str::to_string).collect();

    if on {
        for key in &keys {
            retry(state, key);
        }
    }
    let Some(listing) = state.listing_mut(view) else { return };
    for key in keys {
        match on {
            true => listing.selected.insert(key),
            false => listing.selected.remove(&key),
        };
    }
    refresh(state);
}

/// Снять все отметки этого списка — кнопка в заголовке.
///
/// Все, а не показанные: отметка переживает переход в другую папку — шар
/// держит её, пока не снимут, — и «снять видимое» оставило бы контуры, убрать
/// которые стало бы нечем.
pub fn unmark_all(state: &mut State, view: ViewId) {
    let Some(listing) = state.listing_mut(view) else { return };
    if listing.selected.is_empty() {
        return;
    }
    listing.selected.clear();
    refresh(state);
}

/// Убрать один контур — из списка «На просмотре», где он стоит своей строкой.
///
/// Отметка снимается во всех списках сразу: контур один, а отмечен снимок мог
/// быть и в каталоге, и в выдаче, и оставшаяся отметка вернула бы его тут же.
pub fn drop_one(state: &mut State, key: &str) {
    if !state.clear_mark(key) {
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
    state.highlight = key.map(|key| Highlight { key, view: None, on_globe: true });
    send(state);
}

/// Снять контуры с шара — то есть снять отметки во всех списках: контур живёт
/// отметкой, и убрать его иначе нечем.
pub fn clear(state: &mut State) {
    if !state.clear_selection() {
        return;
    }
    state.deselect();
    state.outlined.clear();
    send(state);
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

/// Пересобрать набор контуров из отмеченного во всех списках и отправить его
/// глобусу.
///
/// Зовётся всякий раз, когда меняется отмеченное или то, из чего берётся
/// геометрия: набор у глобуса заменяется целиком (см. `Outlines` в его
/// types.proto), и другого способа сказать ему про контуры нет. Спрашивать
/// перед отправкой «а изменился ли он» не нужно — топик объявлен снимком, и
/// неизменившийся набор до шины не доходит.
pub fn refresh(state: &mut State) {
    let mut wanted: Vec<Outlined> = Vec::new();
    let mut ask: Vec<String> = Vec::new();
    {
        // Один и тот же снимок отмечают и в каталоге, и в выдаче: контур у него
        // один, и рисовать его дважды значит рисовать его вдвое ярче.
        let mut seen: HashSet<&str> = HashSet::new();
        for view in state.views() {
            let Some(listing) = view.kind.listing() else { continue };
            for key in &listing.selected {
                if !seen.insert(key.as_str()) {
                    continue;
                }
                match product(state, key) {
                    Known::Have(found) if !found.footprint.is_empty() => wanted.push(Outlined {
                        key: key.clone(),
                        label: found.name.clone(),
                        folder: found.folder,
                        rings: found.footprint.clone(),
                    }),
                    Known::Ask => ask.push(key.clone()),
                    // Геометрии у снимка нет вовсе — очерчивать нечего.
                    Known::Have(_) | Known::Nothing => {}
                }
            }
        }
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
    // Выбранный контур мог уйти вместе с отметкой — ленте не на чем держаться.
    // Лежащий растром при этом выбранным остаётся: он и без ленты назван
    // полосой под шаром, а гасить выбор снимка, который на шаре виден, значило
    // бы отвечать «ни на что не смотрим», глядя прямо на него.
    let gone = !state.picked_key().is_empty()
        && !state.outlined.iter().any(|outlined| outlined.key == state.picked_key())
        && !state.overlays.iter().any(|overlay| overlay.identifier == state.picked_key());
    if gone {
        state.deselect();
    }
    send(state);
}

/// Что известно про геометрию отмеченного снимка.
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
/// отсюда и одна точка вызова на каждое изменение.
fn send(state: &State) {
    let picked = state.picked_key();
    let outlines = state
        .outlined
        .iter()
        .flat_map(|outlined| {
            let selected = picked == outlined.key;
            outlined.rings.iter().map(move |ring| Outline {
                points: ring
                    .points
                    .iter()
                    .map(|point| GeoPoint { lat: point.lat, lon: point.lon })
                    .collect(),
                selected,
            })
        })
        .collect();

    crate::calls::globe::on_outlines(&Outlines { outlines });
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

