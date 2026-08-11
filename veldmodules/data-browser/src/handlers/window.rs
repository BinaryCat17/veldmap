//! Жизненный цикл поверхности окна.
//!
//! Хост не знает, кто рендерит наше окно: на app/window_resized мы сами
//! выделяем текстуру нужного размера, делегируем её рендереру write-lease'ом
//! и аттачим хосту. Выделение и грант — общий обряд `veldsdk::surface`
//! (им же отдаётся место под глобус); аттач — уже наше дело, окно наше.
//!
//! Топик адресован (target = имя владельца окна): диспетчер хоста доставляет
//! это событие только нам, фильтровать по plugin_id самим не нужно.

use crate::module::state::State;
use veldsdk::proto::app::{SetSurface, WindowResized};

pub fn on_window_resized(state: &mut State, ev: WindowResized) {
    // Первое объявление окна — самый ранний момент, когда спрашивать других
    // уже можно: к нему все плагины загружены и подписаны (см. runners/desktop,
    // `announce`). До него запросы стартовой вкладки некому доставить.
    if state.window == (0, 0) {
        super::nav::bootstrap(state);
    }
    state.window = (ev.width, ev.height);
    state.scale = ev.scale_factor;

    state.window_surface = veldsdk::surface::delegate(
        state.window_surface.take(),
        ev.width,
        ev.height,
        ev.scale_factor,
        ev.format,
        "ui-service",
        // Читателей нет: поверхность окна забирает композитор хоста, а он не
        // модуль и в правах реестра не значится.
        &[],
        crate::calls::ui_service::on_set_surface,
    );

    // Аттачится то, что лежит в состоянии, а не то, что вернул вызов: при
    // отказе там осталась прежняя текстура, и композить надо именно её.
    if let Some(surface) = &state.window_surface {
        crate::calls::app::on_set_surface(&SetSurface {
            plugin_id: ev.plugin_id.clone(),
            surface: Some(surface.handle()),
        });
    }
}
