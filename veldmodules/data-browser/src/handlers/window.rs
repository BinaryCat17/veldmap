//! Жизненный цикл поверхности окна.
//!
//! Хост не знает, кто рендерит наше окно: на app/window_resized мы сами
//! выделяем текстуру нужного размера, делегируем её рендереру write-lease'ом
//! и аттачим хосту. Весь ритуал — в ui-service-wrap `surface::delegate`.
//!
//! Топик адресован (target = имя владельца окна): диспетчер хоста доставляет
//! это событие только нам, фильтровать по plugin_id самим не нужно.

use crate::module::state::State;
use veldsdk::proto::app::WindowResized;

pub fn on_window_resized(state: &mut State, ev: WindowResized) {
    state.window_surface = veld_ui_service_wrap::surface::delegate(&ev, state.window_surface);
}
