//! Показ списка: отбор, порядок, группировка, страница и меню.
//!
//! Все шесть сообщений одинаковы для трёх видов и правят одно и то же
//! состояние (см. `ViewKind::listing_mut`) — поэтому и обработчик у них общий.
//! Своей ветки на каждый вид здесь нет и быть не должно: список один, и
//! правила показа у него одни.

use crate::module::state::listing::{Filter, Grouping, Menu, Sorting};
use crate::module::state::{State, ViewId};

pub fn on_filter(state: &mut State, view: ViewId, filter: Filter) {
    if let Some(listing) = state.listing_mut(view) {
        listing.filter = filter;
        listing.refine();
    }
}

pub fn on_group(state: &mut State, view: ViewId, grouping: Grouping) {
    if let Some(listing) = state.listing_mut(view) {
        listing.grouping = grouping;
        listing.refine();
    }
}

pub fn on_sort(state: &mut State, view: ViewId, sorting: Sorting) {
    if let Some(listing) = state.listing_mut(view) {
        listing.sorting = sorting;
        listing.refine();
    }
}

pub fn on_query(state: &mut State, view: ViewId, query: String) {
    if let Some(listing) = state.listing_mut(view) {
        listing.query = query;
        // Страница сбрасывается, меню закрывается: набранное меняет состав
        // списка, и прежняя страница относилась к другому списку.
        listing.refine();
    }
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
    if let Some(listing) = state.listing_mut(view) {
        listing.page = page;
        listing.menu = Menu::Closed;
        // Ушли со страницы, к которой привёл переход, — вести больше не к
        // чему, и подсветка на другой странице говорила бы неправду.
        listing.target = None;
    }
}

/// Раскрыть меню, закрыть открытое (`Menu::Closed`) или переключить то же
/// самое — повторное нажатие на чип.
pub fn on_menu(state: &mut State, view: ViewId, menu: Menu) {
    // Было ли это меню раскрыто, спрашивается ДО общего закрытия: оно гасит и
    // его тоже, и после него «то же самое» уже неотличимо от «другое» —
    // повторное нажатие на чип открывало бы его заново вместо того, чтобы
    // закрыть.
    let same = state.listing_mut(view).is_some_and(|listing| listing.menu == menu);
    // Всё раскрытое закрывается разом (см. State::close_menus): два раскрытых
    // меню сразу — состояние, которого не бывает.
    state.close_menus();
    if !same && let Some(listing) = state.listing_mut(view) {
        listing.menu = menu;
    }
}
