//! Показ списка: отбор, порядок, группировка, страница и меню.
//!
//! Все шесть сообщений одинаковы для трёх видов и правят одно и то же
//! состояние (см. `ViewKind::listing_mut`) — поэтому и обработчик у них общий.
//! Своей ветки на каждый вид здесь нет и быть не должно: список один, и
//! правила показа у него одни.

use crate::module::state::listing::{Filter, Grouping, Menu, Sorting};
use crate::module::state::State;

pub fn on_filter(state: &mut State, filter: Filter) {
    if let Some(listing) = state.active_listing_mut() {
        listing.filter = filter;
        listing.refine();
    }
}

pub fn on_group(state: &mut State, grouping: Grouping) {
    if let Some(listing) = state.active_listing_mut() {
        listing.grouping = grouping;
        listing.refine();
    }
}

pub fn on_sort(state: &mut State, sorting: Sorting) {
    if let Some(listing) = state.active_listing_mut() {
        listing.sorting = sorting;
        listing.refine();
    }
}

pub fn on_query(state: &mut State, query: String) {
    if let Some(listing) = state.active_listing_mut() {
        listing.query = query;
        // Страница сбрасывается, меню закрывается: набранное меняет состав
        // списка, и прежняя страница относилась к другому списку.
        listing.refine();
    }
}

pub fn on_page(state: &mut State, page: usize) {
    if let Some(listing) = state.active_listing_mut() {
        listing.page = page;
        listing.menu = Menu::Closed;
    }
}

/// Раскрыть меню, закрыть открытое (`Menu::Closed`) или переключить то же
/// самое — повторное нажатие на чип.
pub fn on_menu(state: &mut State, menu: Menu) {
    // Всё раскрытое закрывается разом (см. State::close_menus), и только
    // потом переключается своё: два раскрытых меню сразу — состояние,
    // которого не бывает.
    state.close_menus();
    if let Some(listing) = state.active_listing_mut() {
        listing.toggle(menu);
    }
}
