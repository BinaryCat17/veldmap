//! Жизненный цикл поверхности окна.
//!
//! Хост не знает, кто рендерит наше окно: на app/window_resized мы сами
//! выделяем текстуру нужного размера, делегируем её рендереру write-lease'ом
//! и аттачим хосту. Весь ритуал — в ui-service-wrap `surface::delegate`.

use crate::module::state::State;
use veldsdk::proto::app::WindowResized;

pub fn on_sub_window_resized(state: &mut State, ev: WindowResized) {
    // Топик broadcast: событие могло быть адресовано окну другого модуля.
    if ev.plugin_id != crate::SERVICE_NAME {
        return;
    }
    state.window_surface = veld_ui_service_wrap::surface::delegate(&ev, state.window_surface);
}
