//! Реализация сервиса app (контракт — veldcore/interface/modules/app/app.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем (generated/,
//! buildgen): State, init и сигнатуры — по конвенции, как в wasm-модулях.
//!
//! Здесь только входная половина контракта. Выходную (on_ui_event,
//! on_window_resized, on_ready) публикует раннер через emit-стабы: эти события
//! порождает цикл событий ОС, а им владеет он. Поэтому модуль общий для всех
//! раннеров — winit'а тут нет.

use std::sync::Arc;
use veldmap_host_util::bindings::proto::app::{RevealPath, SetSurface};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{Caller, HostContext, Surfaces};

pub struct State {
    surfaces: Surfaces,
    /// Нужен для резолва путей: раскладка runtime известна только хосту.
    ctx: Arc<HostContext>,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { surfaces: Surfaces::new(&ctx), ctx }
}

/// Владелец окна аллоцировал текстуру, делегировал её рендереру и просит
/// композить именно её. Права проверяет фасад; свап делает кадровый цикл,
/// забрав поверхность из очереди — подменять текстуру можно только между кадрами.
pub fn on_set_surface(state: &State, req: SetSurface, caller: Caller) {
    let Some(surface) = req.surface else {
        log::warn!(target: "app", "set_surface for '{}' carries no surface handle", req.plugin_id);
        return;
    };
    state.surfaces.attach(&req.plugin_id, surface.id, caller.instance);
}

/// Показать путь в файловом менеджере системы.
///
/// Здесь, а не у заказчика: рабочий стол есть только у платформы. Команда
/// системная и у каждой своя, поэтому выбор идёт по цели сборки, а не по
/// содержимому пути.
///
/// Путь резолвится и проверяется тем же способом, что у fs: наружу runtime мы
/// не показываем ничего — иначе строка из модуля открывала бы человеку любой
/// каталог машины.
pub fn on_reveal_path(state: &State, req: RevealPath, _caller: Caller) {
    if !is_path_safe(&req.path) {
        log::warn!(target: "app", "reveal_path '{}': отказано", req.path);
        return;
    }
    let path = resolve_path(&state.ctx, &req.path);
    // Файл показываем вместе с его каталогом: выделять именно файл умеют не все
    // менеджеры, а открыть каталог умеет каждый.
    let target = match path.is_dir() {
        true => path,
        false => match path.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return,
        },
    };
    if !target.exists() {
        log::warn!(target: "app", "reveal_path: нечего показывать — {}", target.display());
        return;
    }

    let command = match std::env::consts::OS {
        "windows" => "explorer",
        "macos" => "open",
        _ => "xdg-open",
    };
    match std::process::Command::new(command).arg(&target).spawn() {
        Ok(_) => {}
        Err(error) => log::warn!(target: "app", "reveal_path: {} не запустился: {}", command, error),
    }
}
