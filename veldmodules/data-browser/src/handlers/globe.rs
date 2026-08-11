//! Вкладка глобуса: выделить место под чужой рендер и перевести жест в
//! движение камеры.
//!
//! Разделение обязанностей здесь ровно одно, и всё остальное из него следует:
//! **жест наш, камера — его**. Какой кнопкой вращают и что делает колесо, видно
//! только отсюда — это решение об интерфейсе, и принимать его в модуле, где нет
//! ни кнопок, ни остального экрана, было бы неоткуда. Во что превращается
//! поворот, знает только он.
//!
//! Поэтому наружу уходит намерение в долях области, а не пиксели: то же
//! протаскивание в окне вдвое шире обязано повернуть шар на столько же.

use crate::module::state::State;
use crate::proto::globe::{camera_command::Command, CameraCommand, Orbit, Zoom};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};
use veldsdk::graphics::TextureFormat;

/// Формат места под шар. Не тот, что у окна: у окна он продиктован свопчейном
/// (BGRA), а здесь выбираем мы. sRGB — потому что рендерер пишет линейные
/// значения, а sRGB-текстура кодирует их сама, и ui-service, сэмплящий её,
/// получает обратно ровно линейные (см. globe.wgsl).
const SURFACE_FORMAT: TextureFormat = TextureFormat::TexRgba8UnormSrgb;

/// Области досталось новое место. Пока размер тот же, ничего не делаем:
/// перевыделение сменило бы id текстуры, а по нему рендерер решает, что таргет
/// сменился, — и пересобирал бы под него буфер глубины на каждый пересчёт
/// разметки.
pub fn on_resized(state: &mut State, size: ViewportSize) {
    let scale = state.scale;
    let Some(globe) = state.active_globe_mut() else { return };

    if veldsdk::surface::Delegated::covers(globe.surface.as_ref(), size.width, size.height) {
        return;
    }

    globe.surface = veldsdk::surface::delegate(
        globe.surface.take(),
        size.width,
        size.height,
        scale,
        SURFACE_FORMAT as i32,
        "globe",
        // Показывает её разметка, а рисует её глобус: право чтения нужно
        // именно рендереру разметки, чтобы построить по ней bind group.
        &["ui-service"],
        crate::calls::globe::on_set_surface,
    );
}

/// Указатель над областью. Здесь он перестаёт быть указателем.
pub fn on_pointer(state: &mut State, event: PointerEvent) {
    // Логируется до всех отказов: жест, до которого не дошло, отличим от жеста,
    // которого не было, только здесь.
    veldsdk::log::trace!(target: "handlers", "указатель: {:?} ({}, {}) кнопка {} колесо {}",
        event.action(), event.x, event.y, event.button, event.scroll_y);

    let Some(globe) = state.active_globe_mut() else { return };
    // Размер области — та мера, в долях которой считается жест. Без места
    // считать не от чего: событие пришло раньше, чем мы успели его выделить.
    let Some((width, height)) = globe.surface.as_ref().map(|s| (s.width as f32, s.height as f32))
    else {
        return;
    };

    match event.action() {
        // Вращает левая кнопка. Правая и средняя оставлены свободными: под
        // ними будут выделение области и панорамирование, и занимать их
        // «пока чем-нибудь» значит переучивать потом.
        PointerAction::PointerPressed if event.button == 1 => {
            globe.dragging = Some((event.x, event.y));
        }
        PointerAction::PointerMoved => {
            let Some((last_x, last_y)) = globe.dragging else { return };
            globe.dragging = Some((event.x, event.y));
            let (dx, dy) = ((event.x - last_x) / width, (event.y - last_y) / height);
            if dx != 0.0 || dy != 0.0 {
                send(Command::Orbit(Orbit { dx, dy }));
            }
        }
        // Отпускание и уход курсора кончают жест одинаково. Уход, впрочем,
        // приходит только когда кнопка не нажата: удержание тянет движения и
        // за краем области (см. `PointerAction` в types.proto).
        PointerAction::PointerReleased | PointerAction::PointerLeft => {
            globe.dragging = None;
        }
        // Прокрутка приезжает в щелчках колеса (см. `PointerEvent` в
        // types.proto), а шаг приближения меряется ими же — переводить нечего.
        // Щелчок при этом растянут инерцией на несколько кадров, и приближение
        // вместе с ним выходит плавным, а не скачком.
        PointerAction::PointerScrolled => {
            if event.scroll_y != 0.0 {
                send(Command::Zoom(Zoom { delta: event.scroll_y }));
            }
        }
        _ => {}
    }
}

fn send(command: Command) {
    crate::calls::globe::on_camera(&CameraCommand { command: Some(command) });
}
