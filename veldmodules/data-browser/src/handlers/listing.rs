//! Показ списка: отбор, порядок, группировка, страница и меню.
//!
//! Все шесть сообщений одинаковы для трёх видов и правят одно и то же
//! состояние (см. `ViewKind::listing_mut`) — поэтому и обработчик у них общий.
//! Своей ветки на каждый вид здесь нет и быть не должно: список один, и
//! правила показа у него одни.

use crate::module::state::listing::{Filter, Grouping, Menu, Sorting};
use crate::module::state::{Open, State, ViewId};

pub fn on_filter(state: &mut State, view: ViewId, filter: Filter) {
    if let Some(listing) = state.listing_mut(view) {
        listing.filter = filter;
    }
    state.refine(view);
}

pub fn on_group(state: &mut State, view: ViewId, grouping: Grouping) {
    if let Some(listing) = state.listing_mut(view) {
        listing.grouping = grouping;
    }
    state.refine(view);
}

pub fn on_sort(state: &mut State, view: ViewId, sorting: Sorting) {
    if let Some(listing) = state.listing_mut(view) {
        listing.sorting = sorting;
    }
    state.refine(view);
}

pub fn on_query(state: &mut State, view: ViewId, query: String) {
    if let Some(listing) = state.listing_mut(view) {
        listing.query = query;
    }
    state.refine(view);
}

/// Раскрыть строку в её содержимое или свернуть обратно.
///
/// Содержимое папки каталога подгружается лениво, и спрашивается оно ровно
/// здесь: показать его без раскрытия некому, а листать сотню папок наперёд
/// значит сходить в сеть сто раз ради одной строки.
pub fn on_expand(state: &mut State, view: ViewId, key: String) {
    let Some(listing) = state.listing_mut(view) else { return };
    if !listing.expand(key.clone()) {
        return;
    }
    super::browse::request_children(state, view, key);
}

pub fn on_page(state: &mut State, view: ViewId, page: usize) {
    state.close_menus();
    if let Some(listing) = state.listing_mut(view) {
        listing.page = page;
    }
    // Ушли со страницы, к которой привёл переход, — вести больше не к чему,
    // и подсветка на другой странице говорила бы неправду.
    state.drop_target_in(view);
}

/// Раскрыть меню, закрыть раскрытое (`None`) или переключить то же самое —
/// повторное нажатие на чип.
pub fn on_menu(state: &mut State, view: ViewId, menu: Option<Menu>) {
    match menu {
        Some(menu) => state.toggle_open(Open::Listing(view, menu)),
        None => state.close_menus(),
    }
}
