//! Раскладка между запусками: как экран сложили, таким он и открывается.
//!
//! Пишется она сама — по каждому действию, которое её меняет, а не по кнопке
//! «сохранить»: раскладка не документ, а то, где человек оставил работу, и
//! спрашивать разрешения запомнить это незачем.
//!
//! Сохраняется только то, что определяет, **что показано**: форма дерева,
//! доли границ, род каждой вкладки и то немногое, из чего её содержимое
//! спрашивается заново (папка каталога, условия поиска, снимок просмотра).
//! Само содержимое приходит ответом и в файле ему делать нечего: список папки
//! за неделю станет другим, а показанный из файла был бы вчерашней правдой.
//!
//! Файл — кэш, а не документ: не разобрался или пропал — окно открывается тем,
//! что назвал конфиг, и потеря стоит ровно одного расставления вкладок.

use crate::module::state::listing::Choice;
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{
    Axis, BrowseState, Node, PaneId, SearchState, State, ViewKind,
};
use crate::module::NewTab;
use veldsdk::proto::core::{ResourceHandle, ResourceOpened};
use veldsdk::proto::fs::{FsReadRequest, FsWriteRequest, FsWriteResult};

/// Где лежит запомненное. Рядом с `runtime/`, а не в `data/`: там скачанное, и
/// снос кэша данных не должен уносить с собой расставленные вкладки.
const PATH: &str = "state/data-browser.json";

// -- Сохранённая раскладка --
//
// Отдельные типы, а не сериализация живого состояния: в состоянии лежат
// ресурсы, ожидания ответов и присланные каталогом строки, и ни одно из этого
// не переживает запуск. Здесь — только то, что можно записать словами.

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    root: SavedNode,
    /// Панель, что была под рукой, — номером в порядке обхода дерева. Номер, а
    /// не пометка у самой панели: «под рукой» бывает ровно одна, а пометка у
    /// каждой позволила бы записать две.
    focus: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "node", rename_all = "lowercase")]
enum SavedNode {
    Pane {
        tabs: Vec<SavedTab>,
        /// Показанная вкладка — номером в списке выше.
        active: usize,
    },
    Split {
        axis: String,
        fraction: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "tab", rename_all = "lowercase")]
enum SavedTab {
    Empty,
    Browse {
        path: String,
    },
    /// Условия поиска, а не найденное: выдача приходит от каталога, и вчерашняя
    /// под сегодняшним запросом была бы неправдой.
    Search {
        query: String,
        mission: String,
        period: String,
        from: String,
        to: String,
        cloud: String,
    },
    Downloaded,
    Globe,
    Shown,
    /// Чем снимок подписан и запись библиотеки, если он скачан, — ровно то, из
    /// чего его открывают заново (см. `handlers::preview::reopen`).
    Preview {
        label: String,
        entry: Option<String>,
    },
}

// -- Запись --

/// Пишет раскладку, если она изменилась с прошлой записи.
///
/// Сравнение отпечатком, а не «мы знаем, какие сообщения её меняют»: список
/// таких сообщений пришлось бы держать в голове и однажды забыть в нём новое.
pub fn save_if_changed(state: &mut State) {
    let Ok(body) = veldsdk::serde_json::to_vec(&snapshot(state)) else { return };
    let mark = fingerprint(&body);
    if mark == state.last_saved {
        return;
    }
    state.last_saved = mark;
    write(state, &body);
}

/// Запоминает нынешнюю раскладку как записанную, ничего не записывая, — так
/// восстановленная не переписывает сама себя тем же содержимым.
fn remember(state: &mut State) {
    if let Ok(body) = veldsdk::serde_json::to_vec(&snapshot(state)) {
        state.last_saved = fingerprint(&body);
    }
}

fn fingerprint(body: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    hasher.finish()
}

/// Кладёт байты в файл через fs: CPU-регион → on_write → освобождение по
/// ответу (см. [`on_write_result`]).
fn write(state: &mut State, body: &[u8]) {
    let Some(region_id) = veldsdk::abi::resource_alloc_cpu(body.len() as u64) else {
        veldsdk::log::warn!(target: "handlers", "раскладка не записана: нет памяти под регион");
        return;
    };
    // Во владельца сразу после выделения: сорвись запись, регион освободит
    // Drop, а голый id остался бы висеть на хосте до конца процесса.
    let region = veldsdk::OwnedResource::from_raw_id(region_id);
    if let Err(error) = veldsdk::abi::resource_write(region.id(), 0, body) {
        veldsdk::log::warn!(target: "handlers", "раскладка не записана: {}", error);
        return;
    }

    let size = body.len() as u64;
    let correlation = state.layout_writes.begin(region.id());
    crate::calls::fs::on_write(
        &FsWriteRequest {
            path: PATH.to_string(),
            // Владение уезжает в таблицу ожидания: освободит его ответ.
            handle: Some(ResourceHandle { id: region.into_handle().id, size }),
        },
        &correlation,
    );
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(region) = state.layout_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(region));
    if !result.error.is_empty() {
        // Незаписанная раскладка стоит одного расставления вкладок — жаловаться
        // человеку не о чем, а в логе след нужен.
        veldsdk::log::warn!(target: "handlers", "раскладка не записана: {}", result.error);
    }
}

// -- Чтение --

/// Спросить сохранённую раскладку. Ответ разбирает [`on_read_result`]; он же
/// открывает стартовую вкладку, когда вспоминать нечего, — до ответа окно
/// пустое, и это единственное честное состояние: что показывать, ещё неизвестно.
pub fn load(state: &mut State) {
    let correlation = state.layout_read.begin();
    crate::calls::fs::on_read(&FsReadRequest { path: PATH.to_string() }, &correlation);
}

/// Файл раскладки прочитан — или не прочитан вовсе: первый запуск, снесённый
/// runtime, чужая версия формата. Все три случая значат одно: вспоминать
/// нечего, открываем то, что назвал конфиг.
pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let correlation = veldsdk::correlation();
    if state.layout_read.settle(&correlation) != veldsdk::Reply::Current {
        // Ответ на вытесненный вопрос: ресурс всё равно наш, а собирать по нему
        // экран второй раз нельзя.
        return veldsdk::resource::discard("fs/on_read_result", opened);
    }

    let Ok(handle) = veldsdk::resource::accept(&opened) else { return fresh(state) };
    // RAII-гард: регион освобождается при любом выходе ниже.
    let resource = veldsdk::OwnedResource::new(handle.clone());
    let Ok(bytes) = veldsdk::abi::resource_read(resource.id(), 0, handle.size) else {
        return fresh(state);
    };
    let saved = match veldsdk::serde_json::from_slice::<Saved>(&bytes) {
        Ok(saved) => saved,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "раскладка не разобралась: {}", error);
            return fresh(state);
        }
    };

    restore(state, &saved);
    remember(state);
}

/// Первый запуск: одна вкладка, названная конфигом.
fn fresh(state: &mut State) {
    super::nav::on_new_tab(state, state.focused(), state.start);
}

// -- Снимок живого состояния --

fn snapshot(state: &State) -> Saved {
    let mut panes = Vec::new();
    let root = save_node(state, state.layout().root(), &mut panes);
    let focus = panes.iter().position(|pane| *pane == state.focused()).unwrap_or(0);
    Saved { root, focus }
}

/// Узел дерева и, попутно, порядок обхода панелей — им называется та, что была
/// под рукой.
fn save_node(state: &State, node: &Node, panes: &mut Vec<PaneId>) -> SavedNode {
    match node {
        Node::Leaf(leaf) => {
            panes.push(leaf.id);
            let tabs: Vec<SavedTab> = state.views_in(leaf.id).map(|view| save_tab(&view.kind)).collect();
            let active = state
                .active_in(leaf.id)
                .and_then(|id| state.views_in(leaf.id).position(|view| view.id == id))
                .unwrap_or(0);
            SavedNode::Pane { tabs, active }
        }
        Node::Split(split) => SavedNode::Split {
            axis: split.axis.key().to_string(),
            fraction: split.fraction,
            first: Box::new(save_node(state, &split.first, panes)),
            second: Box::new(save_node(state, &split.second, panes)),
        },
    }
}

fn save_tab(kind: &ViewKind) -> SavedTab {
    match kind {
        ViewKind::Empty => SavedTab::Empty,
        ViewKind::Browse(browse) => SavedTab::Browse { path: browse.current_path.clone() },
        ViewKind::Search(search) => SavedTab::Search {
            query: search.query.clone(),
            mission: search.mission.key().to_string(),
            period: search.period.key().to_string(),
            from: search.from.clone(),
            to: search.to.clone(),
            cloud: search.cloud.key().to_string(),
        },
        ViewKind::Downloaded(_) => SavedTab::Downloaded,
        ViewKind::Globe(_) => SavedTab::Globe,
        ViewKind::Shown => SavedTab::Shown,
        ViewKind::Preview(preview) => {
            SavedTab::Preview { label: preview.label.clone(), entry: preview.entry.clone() }
        }
    }
}

// -- Сборка живого состояния из снимка --

fn restore(state: &mut State, saved: &Saved) {
    let mut panes = Vec::new();
    rebuild(state, state.focused(), &saved.root, &mut panes);
    // Взгляд — туда, где он был; панели вне списка не бывает, но список мог
    // приехать от другой версии формата.
    if let Some(pane) = panes.get(saved.focus).or(panes.first())
        && let Some(view) = state.active_in(*pane)
    {
        state.focus(view);
    }
}

fn rebuild(state: &mut State, pane: PaneId, node: &SavedNode, panes: &mut Vec<PaneId>) {
    match node {
        SavedNode::Pane { tabs, active } => {
            panes.push(pane);
            for tab in tabs {
                open_tab(state, pane, tab);
            }
            // Показанной делаем ту, что была показана: открытие делает активной
            // каждую вкладку, поэтому это отдельный шаг после всех.
            let shown = state.views_in(pane).nth(*active).map(|view| view.id);
            if let Some(view) = shown {
                state.focus(view);
            }
        }
        SavedNode::Split { axis, fraction, first, second } => {
            // Ось неизвестна — файл из другой версии формата; тогда деления
            // просто не будет, а вкладки обеих его половин лягут в одну панель.
            let side = match Axis::from_key(axis) {
                Some(axis) => axis.tail(),
                None => {
                    rebuild(state, pane, first, panes);
                    rebuild(state, pane, second, panes);
                    return;
                }
            };
            let Some(fresh) = state.split_pane(pane, side, *fraction) else { return };
            rebuild(state, pane, first, panes);
            rebuild(state, fresh, second, panes);
        }
    }
}

/// Заводит вкладку и просит то, чем она живёт, — тем же ходом, каким её
/// заводит человек.
fn open_tab(state: &mut State, pane: PaneId, tab: &SavedTab) {
    // Синглтон, уже восстановленный в другой панели, второй раз не заводим:
    // файл могли поправить руками, а два глобуса — это два места под кадр у
    // модуля, который рисует один (см. `handlers::nav::singleton`).
    if let Some(kind) = singleton_kind(tab)
        && super::nav::singleton(state, kind).is_some()
    {
        return;
    }

    match tab {
        SavedTab::Empty => {
            state.open_in(pane, ViewKind::Empty);
        }
        SavedTab::Browse { path } => {
            let id = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
            super::browse::request_path(state, id, path.clone());
        }
        SavedTab::Search { query, mission, period, from, to, cloud } => {
            let search = SearchState {
                query: query.clone(),
                mission: Mission::from_key(mission).unwrap_or_default(),
                period: Period::from_key(period).unwrap_or_default(),
                from: from.clone(),
                to: to.clone(),
                cloud: Cloud::from_key(cloud).unwrap_or_default(),
                ..Default::default()
            };
            let id = state.open_in(pane, ViewKind::Search(search));
            super::search::run(state, id);
        }
        SavedTab::Downloaded => {
            // Каталог библиотеки уже спрошен на старте (см. nav::bootstrap), и
            // приедет он рассылкой — своего запроса вкладке не нужно.
            state.open_in(pane, ViewKind::Downloaded(Default::default()));
        }
        SavedTab::Globe => {
            state.open_in(pane, ViewKind::Globe(Default::default()));
        }
        SavedTab::Shown => {
            state.open_in(pane, ViewKind::Shown);
        }
        SavedTab::Preview { label, entry } => {
            super::preview::reopen(state, pane, label.clone(), entry.clone());
        }
    }
}

/// Род вкладки, которой не бывает двух. `None` — таких заводят сколько угодно.
fn singleton_kind(tab: &SavedTab) -> Option<NewTab> {
    match tab {
        SavedTab::Downloaded => Some(NewTab::Downloaded),
        SavedTab::Globe => Some(NewTab::Globe),
        SavedTab::Shown => Some(NewTab::Shown),
        SavedTab::Search { .. } => Some(NewTab::Search),
        SavedTab::Empty | SavedTab::Browse { .. } | SavedTab::Preview { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::state::Side;

    fn state() -> State {
        State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние")
    }

    /// Собранный экран переживает запись и чтение: те же панели, те же доли,
    /// те же вкладки в тех же местах.
    #[test]
    fn layout_survives_a_round_trip() {
        let mut before = state();
        let left = before.focused();
        let right = before.split_pane(left, Side::Right, 0.3).expect("панель заведена");
        let under = before.split_pane(right, Side::Below, 0.75).expect("панель заведена");
        before.open_in(left, ViewKind::Browse(BrowseState::default()));
        before.open_in(left, ViewKind::Empty);
        before.open_in(right, ViewKind::Globe(Default::default()));
        before.open_in(under, ViewKind::Shown);
        // Показана в левой панели первая, а не последняя открытая.
        let first = before.views_in(left).next().expect("вкладка").id;
        before.focus(first);

        let body = veldsdk::serde_json::to_vec(&snapshot(&before)).expect("снимок");

        let mut after = state();
        let saved: Saved = veldsdk::serde_json::from_slice(&body).expect("разбор снимка");
        restore(&mut after, &saved);

        assert_eq!(after.layout().count(), 3);
        assert_eq!(
            veldsdk::serde_json::to_vec(&snapshot(&after)).expect("снимок"),
            body,
            "раскладка после чтения не та же"
        );
    }

    /// Второй глобус из поправленного руками файла не заводится: место под
    /// кадр у рисующего модуля одно.
    #[test]
    fn a_second_singleton_is_dropped() {
        let mut restored = state();
        let saved = Saved {
            root: SavedNode::Pane {
                tabs: vec![SavedTab::Globe, SavedTab::Globe, SavedTab::Shown],
                active: 0,
            },
            focus: 0,
        };
        restore(&mut restored, &saved);
        assert_eq!(restored.views().len(), 2);
    }
}
