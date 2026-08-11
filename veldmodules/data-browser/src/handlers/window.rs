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
    // Первое объявление окна — самый ранний момент, когда спрашивать других
    // уже можно: к нему все плагины загружены и подписаны (см. runners/desktop,
    // `announce`). До него запросы стартовой вкладки некому доставить.
    if state.window == (0, 0) {
        super::nav::bootstrap(state);
    }
    state.window = (ev.width, ev.height);
    state.scale = ev.scale_factor;
    state.window_surface = veld_ui_service_wrap::surface::delegate(
        &ev,
        state.window_surface,
        crate::calls::ui_service::on_set_surface,
        crate::calls::app::on_set_surface,
    );
}
