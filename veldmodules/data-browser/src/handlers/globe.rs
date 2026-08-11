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
//!
//! Из того же разделения следует и обратный ход. Куда пришёлся щелчок, знает
//! только камера: одна и та же точка кадра означает разные места на разной
//! высоте. Поэтому спрашиваем её теми же долями области, получаем ответ местом
//! на Земле, а что там за снимок — решаем уже сами (см. `handlers::search`).

use crate::module::state::globe::Drag;
use crate::module::state::State;
use crate::proto::globe::{camera_command::Command, CameraCommand, Orbit, Probe, Probed, Zoom};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};
use veldsdk::graphics::TextureFormat;

/// Формат места под шар. Не тот, что у окна: у окна он продиктован свопчейном
/// (BGRA), а здесь выбираем мы.
///
/// UNORM, а не sRGB: показывает эту текстуру разметка тем же путём, что и любую
/// другую картинку, а картинки у неё лежат в UNORM и хранят sRGB-числа —
/// сэмплируя их, она линеаризует прочитанное сама (см. shaders.wgsl в
/// ui-service). sRGB-текстура на этом пути линеаризуется дважды и приезжает на
/// экран заметно темнее, чем нарисована.
const SURFACE_FORMAT: TextureFormat = TextureFormat::TexRgba8Unorm;

/// Насколько указателю позволено уехать, чтобы нажатие всё ещё считалось
/// щелчком. Пиксели области: рука дрожит на пиксель-другой, а протаскивание за
/// это время не успевает повернуть шар на заметный угол.
const CLICK_SLOP: f32 = 4.0;

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

/// Указатель над областью. Здесь он перестаёт быть указателем: дальше едет
/// либо движение камеры, либо вопрос о месте под ним.
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
            globe.dragging = Some(Drag { last: (event.x, event.y), travelled: 0.0 });
        }
        PointerAction::PointerMoved => {
            let Some(drag) = &mut globe.dragging else { return };
            let (dx, dy) = (event.x - drag.last.0, event.y - drag.last.1);
            drag.last = (event.x, event.y);
            drag.travelled += dx.abs() + dy.abs();
            if dx != 0.0 || dy != 0.0 {
                // Обе оси меряются высотой области, а не своей стороной: угол
                // обзора задан по вертикали, и доля высоты означает один и тот
                // же угол, куда бы она ни вела. Делённое на ширину поперёк
                // растягивалось бы вместе с окном, и жест по диагонали шёл бы
                // не за курсором, а мимо.
                send(Command::Orbit(Orbit { dx: dx / height, dy: dy / height }));
            }
        }
        // Отпускание на месте — это щелчок: указывают им же, чем и вращают, и
        // отличает одно от другого только пройденный путь.
        PointerAction::PointerReleased => {
            let travelled = globe.dragging.take().map(|drag| drag.travelled);
            if travelled.is_some_and(|travelled| travelled <= CLICK_SLOP) {
                probe(state, event.x / width, event.y / height);
            }
        }
        // Уход курсора кончает жест молча. Приходит он только когда кнопка не
        // нажата: удержание тянет движения и за краем области (см.
        // `PointerAction` в types.proto).
        PointerAction::PointerLeft => {
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

/// Спросить, куда пришёлся щелчок. Своими силами на это не ответить: одна и та
/// же точка кадра означает разные места на разной высоте, а высота — у камеры,
/// и камера не наша.
fn probe(state: &mut State, x: f32, y: f32) {
    let correlation_id = state.probe.begin();
    crate::calls::globe::on_probe(&Probe { x, y }, &correlation_id);
}

/// Ответ на него. Мимо Земли или мимо всех контуров — выбор снимается: щелчок
/// по пустому месту значит ровно это.
pub fn on_probed(state: &mut State, probed: Probed) {
    if state.probe.settle(&veldsdk::correlation()) != veldsdk::Reply::Current {
        return;
    }
    // Место щелчка — единственное, по чему видно, промахнулись мы мимо контура
    // или мимо самой Земли: снаружи обе неудачи выглядят одинаково.
    match &probed.at {
        Some(at) => veldsdk::log::debug!(target: "handlers", "щелчок по шару: {:.4}, {:.4}", at.lat, at.lon),
        None => veldsdk::log::debug!(target: "handlers", "щелчок мимо Земли"),
    }
    super::search::pick(state, probed.at.map(|at| (at.lat, at.lon)));
}

fn send(command: Command) {
    crate::calls::globe::on_camera(&CameraCommand { command: Some(command) });
}
