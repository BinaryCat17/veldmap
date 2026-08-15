//! Сетевой каталог: переходы по папкам и подгрузка раскрытых строк.

use crate::module::components::{arrange, folder_of, folder_path, rows};
use crate::module::state::browse::BrowseItem;
use crate::module::state::{Highlight, Listing, State, ViewId, ViewKind};
use crate::proto::data_provider::{ListEntry, ListPathRequest, ListPathResponse};

/// Сколько записей забирать из одной папки. Хранилище отдаёт листинг
/// страницами, и страницы дочитываются одна за другой — иначе папка с сотней
/// продуктов показывала бы первые двести и молчала об остальных. Потолок нужен
/// затем, что корень бакета не кончается.
pub const MAX_ITEMS: usize = 1000;

/// Запрашивает листинг пути у названного вида и переводит его в loading.
/// Единственная точка входа — и для навигации в папку, и для «вверх», и для
/// открытия вкладки (см. handlers::nav): три копии этих десяти строк
/// расходились в мелочах.
pub fn request_path(state: &mut State, view: ViewId, path: String) {
    // Путь папки — всегда со слэшем; корень — пустой. Продукт из поиска
    // приходит голым ключом (`eodata/…/S1C_….SAFE`), а листинг по префиксу
    // без слэша показал бы саму папку вместо её содержимого.
    let path = if path.is_empty() || path.ends_with('/') { path } else { path + "/" };
    let correlation_id = {
        let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };
        browse.current_path = path.clone();
        browse.error = None;
        // Содержимое накапливается ответами (см. on_list_path_result), поэтому
        // новый обход начинается с пустого списка, а не заменяет старый в конце:
        // иначе на экране до первого ответа стояла бы чужая папка.
        browse.items.clear();
        // Раскрытое принадлежало прошлой папке: её строк здесь больше нет, а
        // одноимённая раскрылась бы сама собой, ничего для этого не сделав.
        browse.children.clear();
        browse.listing.expanded.clear();
        browse.listing.page = 0;
        browse.request.begin()
    };
    // Ушли из папки, к строке которой привёл переход, — подсвечивать больше
    // нечего.
    state.drop_target_in(view);
    state.listings.insert(correlation_id.clone(), Listing::Path(view));

    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: String::new(),
        recursive: false,
    }, &correlation_id);
}

/// Спросить содержимое раскрытой строки. Зовётся только на раскрытие: закрытая
/// строка своё содержимое не показывает, и листать его незачем — в корне бакета
/// папок сотни.
pub fn request_children(state: &mut State, view: ViewId, key: String) {
    let path = folder_path(&key);
    let Some(children) = state.get_mut(view).and_then(ViewKind::children_mut) else { return };
    // Спрашивали уже — второй раз незачем: строку раскрывают и закрывают
    // сколько угодно, а содержимое папки от этого не меняется.
    if children.known(&path) {
        return;
    }
    children.begin(path.clone());

    let correlation_id = state.listings.begin(Listing::Children(view, path.clone()));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: String::new(),
        recursive: false,
    }, &correlation_id);
}

/// Перейти в папку. Пустой путь — перечитать текущую: так же приходит и
/// «обновить», у которого своего пути нет.
///
/// Из вида, у которого каталога нет (скачанное, поиск), переход ведёт в
/// каталог — открытый, а не новый: показать папку — это перейти в неё, а не
/// завести под неё вкладку (см. `handlers::nav::catalog`).
pub fn on_enter(state: &mut State, view: ViewId, path: String) {
    let Some(browse) = state.browse_mut(view) else {
        if !path.is_empty() {
            let opened = super::nav::catalog(state, view);
            request_path(state, opened, path);
        }
        return;
    };
    let target = if path.is_empty() { browse.current_path.clone() } else { path };
    request_path(state, view, target);
}

pub fn on_up(state: &mut State, view: ViewId) {
    let Some(browse) = state.browse_mut(view) else { return };
    let mut path = browse.current_path.clone();

    if path.ends_with('/') {
        path.pop();
    }
    match path.rfind('/') {
        Some(idx) => path.truncate(idx + 1),
        None => path.clear(), // Root
    }
    request_path(state, view, path);
}

/// Показать запись в этом каталоге: перейти в её папку и встать на её строку.
///
/// Переход к уже открытой папке не перечитывает её: содержимое под рукой, и
/// сетевой ход ради того же самого только заставил бы ждать.
pub fn reveal(state: &mut State, view: ViewId, key: String) {
    let folder = match folder_of(&key) {
        "" => String::new(),
        path => format!("{}/", path),
    };
    let here = matches!(state.get(view), Some(ViewKind::Browse(browse)) if browse.shows(&folder));
    if !here {
        request_path(state, view, folder);
    }
    // Привели к другому снимку, чем обведён на шаре, — лента гаснет: подсвечена
    // на экране одна строка, и переход — свежий ответ на тот же вопрос, что и
    // щелчок по шару (см. `state::Highlight`). К тому же самому снимку приводит
    // как раз полоса под шаром, и там гасить нечего.
    let same = state.picked_key() == key;
    if !same {
        super::outline::deselect(state);
    }
    if let Some(listing) = state.listing_mut(view) {
        // Просьба новая, даже если строка та же: без этого повторный переход к
        // ней ничего бы не сдвинул (см. `ListingState::aim`).
        listing.aim = listing.aim.wrapping_add(1);
    }
    state.highlight = Some(Highlight { key, view: Some(view), on_globe: same });
    // Папку ещё листают — считать страницу не по чему; посчитает её конец
    // листинга (см. [`aim`]).
    if here {
        aim(state, view);
    }
}

/// Встать на страницу, где стоит строка перехода.
///
/// Считается по тем же строкам и тому же отбору, что показаны: страница —
/// свойство показа, и второй способ её вычислить указал бы не туда ровно
/// тогда, когда список отсортирован не по умолчанию.
fn aim(state: &mut State, view: ViewId) {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else { return };
    let key = state.target_in(view).to_string();
    if key.is_empty() {
        return;
    }
    let page = arrange::page_of(&rows, listing, &key);
    // Строка в папке есть, а на странице её нет — значит её убрал отбор,
    // набранный в этой вкладке. Промолчать тут нельзя: снаружи «привели, но
    // никуда» и «не сработало» выглядят одинаково.
    let hidden = page.is_none() && rows.iter().any(|row| row.named(&key));

    if let Some(page) = page {
        if let Some(listing) = state.listing_mut(view) {
            listing.page = page;
        }
    } else if hidden {
        state.notice = Some("Строка есть в папке, но её скрыл отбор".to_string());
    }
}

/// Broadcast-топик: кому этот листинг, знает таблица маршрутов. Не найден — не
/// наш.
pub fn on_list_path_result(state: &mut State, response: ListPathResponse) {
    let correlation_id = veldsdk::correlation();
    let Some(asked) = state.listings.take(&correlation_id) else { return };
    match asked {
        Listing::Path(view) => on_path(state, view, correlation_id, response),
        Listing::Children(view, key) => on_children(state, view, key, correlation_id, response),
        Listing::Snapshot { product, files, queued } => {
            let counted = super::library::Counted { product, files, queued };
            super::library::on_snapshot_files(state, counted, correlation_id, response)
        }
    }
}

/// Содержимое папки, которую показывает вкладка.
///
/// Вид не найден — вкладку закрыли, пока ответ шёл, и показывать его негде.
/// Свой, но устаревший (пользователь успел уйти в другую папку) отбрасываем
/// тоже: его содержимое под нынешним путём было бы неправдой.
///
/// Ответ бывает не последним: пока хранилище отдаёт продолжение, запрос
/// остаётся тем же — та же корреляция, тот же учёт, — и с учёта снимается
/// только на последней странице.
fn on_path(state: &mut State, view: ViewId, correlation_id: String, response: ListPathResponse) {
    let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };

    if browse.request.status(&correlation_id) != veldsdk::Reply::Current {
        browse.request.settle(&correlation_id);
        return;
    }

    if !response.error.is_empty() {
        browse.request.settle(&correlation_id);
        browse.error = Some(response.error);
        browse.items.clear();
        return;
    }
    browse.error = None;
    browse.items.extend(response.entries.into_iter().map(item));

    let more = !response.next_token.is_empty() && browse.items.len() < MAX_ITEMS;
    if !more {
        browse.request.settle(&correlation_id);
        // Строка перехода могла приехать только что — теперь видно, на какой
        // она странице.
        aim(state, view);
        return;
    }

    let path = browse.current_path.clone();
    state.listings.insert(correlation_id.clone(), Listing::Path(view));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: response.next_token,
        recursive: false,
    }, &correlation_id);
}

/// Содержимое раскрытой строки.
///
/// Отказ вид не ломает — папка одна из многих, — но и не запоминается: папка
/// забывается целиком, и следующее раскрытие спросит её заново. Оставленный
/// пустой ключ значил бы «спрашивали, и там пусто», то есть закрыл бы дорогу
/// повтору навсегда.
fn on_children(
    state: &mut State,
    view: ViewId,
    key: String,
    correlation_id: String,
    response: ListPathResponse,
) {
    let Some(children) = state.get_mut(view).and_then(ViewKind::children_mut) else { return };

    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "'{}' не раскрылась: {}", key, response.error);
        children.forget(&key);
        return;
    }
    // Папку успели забыть, пока страница шла, — цепочку обрываем: её хвост в
    // набор не кладут (см. `Children::extend`).
    if !children.extend(&key, response.entries.into_iter().map(item)) {
        return;
    }

    if response.next_token.is_empty() {
        children.settle(&key);
        return;
    }
    // Потолок обхода. Молча обрывать нельзя: раскрытая папка с тысячей записей
    // и раскрытая папка, которой оборвали хвост, с виду одинаковы.
    if children.get(&key).len() >= MAX_ITEMS {
        let shown = children.get(&key).len();
        children.settle(&key);
        state.notice = Some(format!("«{}»: показаны первые {} записей", key, shown));
        return;
    }

    state.listings.insert(correlation_id.clone(), Listing::Children(view, key.clone()));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path: key,
        token: response.next_token,
        recursive: false,
    }, &correlation_id);
}

/// Запись листинга → то, чем её помнит вид. Одно место на все три листинга:
/// признак папки и короткое имя выводятся из ключа, и три копии этого правила
/// однажды ответили бы по-разному.
fn item(entry: ListEntry) -> BrowseItem {
    BrowseItem {
        is_folder: entry.key.ends_with('/'),
        product: entry.product,
        name: entry
            .key
            .split('/')
            .filter(|part| !part.is_empty())
            .next_back()
            .unwrap_or("")
            .to_string(),
        identifier: entry.key,
        size: entry.size,
        modified: entry.modified,
    }
}
