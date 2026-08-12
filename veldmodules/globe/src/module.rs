//! Трёхмерный вид на Землю.
//!
//! Модуль знает про Землю и камеру и не знает больше ничего: ни про вкладки, ни
//! про курсор, ни про то, что ещё в это время на экране. Место под себя он
//! получает готовым (`on_set_surface`) — так же, как ui-service получает
//! поверхность окна, — а движение камеры приезжает уже намерением
//! (`on_camera`), потому что решать, каким жестом его вызвать, может только
//! тот, кто рисует интерфейс.
//!
//! Координаты — настоящие: WGS84 и ECEF, тот же эллипсоид и те же оси, в
//! которых приходят широта с долготой снаружи (см. `geodesy`).

pub mod camera;
pub mod geodesy;
pub mod gpu;
pub mod mesh;
pub mod outlines;

use camera::Camera;
use gpu::{Device, Target};
use veldsdk::proto::app as app_proto;
use veldsdk::proto::core::SurfaceDelegated;

/// Настраивать нечего: всё, что нужно знать о месте под рендер, приезжает
/// вместе с самим местом.
#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    camera: Camera,
    /// Появляется на первом делегировании места: до него ни формата таргета,
    /// под который собирать пайплайны, ни повода что-то выделять.
    device: Option<Device>,
    target: Option<Target>,
    /// Камера и текстура, в которых записан последний кадр. Совпали с
    /// нынешними — повторять кадр незачем: Земля сама не движется, и пока её
    /// не двигают, в таргете уже лежит ровно то, что нарисовалось бы снова.
    /// Так же простаивает и вкладка, которую увели с экрана: событий камеры
    /// оттуда не приходит, а знать, что её не показывают, нам неоткуда.
    ///
    /// Именно сравнение, а не флаг «пора перерисовать»: флаг пришлось бы
    /// ставить в каждом месте, где что-то меняется, и однажды не поставить.
    /// Новую текстуру этого сравнения довольно, чтобы отличить: id ресурсов
    /// монотонны и не переиспользуются (см. `ResourceRegistry::register`).
    drawn: Option<Frame>,
    /// Последний присланный набор контуров. Хранится потому, что приехать он
    /// может раньше места под рендер, а залить его в буферы можно только с
    /// готовым устройством.
    outlines: Vec<crate::proto::globe::Outline>,
    /// Сколько раз набор контуров сменился. Сами контуры в [`Frame`] не
    /// сравнить, не держа их копию только ради сравнения, — а счётчик отвечает
    /// на тот же вопрос: набор с прошлого кадра тот же или уже другой.
    generation: u64,
}

/// То, из чего собран кадр. Совпало с нынешним — рисовать нечего.
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    camera: Camera,
    texture: u64,
    generation: u64,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State {
        camera: Camera::default(),
        device: None,
        target: None,
        drawn: None,
        outlines: Vec::new(),
        generation: 0,
    })
}

/// Место под вид: владелец выделил текстуру и выдал нам право записи. Пустая
/// поверхность — отзыв: место кончилось вместе со своим хозяином.
///
/// Устройство при отзыве остаётся: геометрия и пайплайны от места не зависят,
/// а вкладку глобуса обычно закрывают и открывают снова.
///
/// Пайплайны собраны под формат таргета, поэтому смена формата их
/// пересобирает. На практике он не меняется, но узнать об этом отсюда нельзя —
/// а расхождение вскрылось бы отказом отрисовки без внятной причины.
pub fn on_set_surface(state: &mut State, req: SurfaceDelegated) {
    let Some(surface) = req.surface else {
        veldsdk::log::info!(target: "handlers", "место отозвано");
        state.target = None;
        return;
    };
    if req.width == 0 || req.height == 0 {
        veldsdk::log::warn!(target: "handlers", "место {}x{} — рисовать негде", req.width, req.height);
        return;
    }

    if state.device.as_ref().is_none_or(|device| device.format != req.format) {
        match Device::create(req.format) {
            Ok(device) => state.device = Some(device),
            Err(error) => {
                // Прежний таргет бросаем по той же причине, что и при отказе
                // ниже: владелец уже освободил его текстуру, выделяя эту, и
                // держать view — значит держать её живой без зрителей.
                veldsdk::log::error!(target: "handlers", "ресурсы устройства: {:#}", error);
                state.target = None;
                return;
            }
        }
    }

    // Контуры могли приехать раньше устройства, а пересобранное устройство
    // забирает буферы с собой — в обоих случаях залить их нужно заново.
    upload_outlines(state);

    match Target::create(surface.id, req.width, req.height) {
        Ok(target) => {
            veldsdk::log::info!(target: "handlers",
                "место: текстура {} ({}x{})", surface.id, req.width, req.height);
            state.target = Some(target);
        }
        Err(error) => {
            // Прежний таргет бросаем: он ссылается на текстуру, которую
            // владелец уже освободил, выделяя эту. Рисовать в неё некуда —
            // показывать её перестали, — а держать её значит держать живой и
            // саму текстуру, и буфер глубины под неё.
            veldsdk::log::error!(target: "handlers", "буфер глубины: {:#}", error);
            state.target = None;
        }
    }
}

pub fn on_camera(state: &mut State, command: crate::proto::globe::CameraCommand) {
    use crate::proto::globe::camera_command::Command;
    match command.command {
        Some(Command::Orbit(orbit)) => state.camera.orbit(orbit.dx, orbit.dy),
        Some(Command::Zoom(zoom)) => state.camera.zoom(zoom.delta),
        // Наводка без точки — это наводка в никуда: молча смотреть в центр
        // координат хуже, чем не двигаться вовсе.
        Some(Command::Focus(focus)) => match focus.at {
            Some(at) => state.camera.focus(at.lat, at.lon, focus.radius_deg),
            None => veldsdk::log::warn!(target: "handlers", "наводка без точки"),
        },
        None => {}
    }
}

/// Что под указателем. Отвечаем всегда: «мимо Земли» — такой же ответ, как
/// точка, и спрашивающий вправе его получить. Без места под рендер ответ тот
/// же: кадра нет — значит нет и точки кадра, про которую спрашивают.
pub fn on_probe(state: &mut State, probe: crate::proto::globe::Probe) {
    let at = state.target.as_ref().and_then(|target| {
        let (eye, direction) = state.camera.ray(probe.x, probe.y, target.aspect());
        geodesy::intersect(eye, direction).map(|point| {
            let (lat, lon) = geodesy::surface_at(point);
            crate::proto::globe::GeoPoint { lat, lon }
        })
    });

    crate::emit::on_probed(
        &crate::proto::globe::Probed { at },
        &veldsdk::correlation(),
    );
}

/// Что очертить на поверхности. Набор целиком заменяет прежний.
pub fn on_outlines(state: &mut State, outlines: crate::proto::globe::Outlines) {
    state.outlines = outlines.outlines;
    state.generation += 1;
    upload_outlines(state);
}

/// Перестраивает геометрию контуров и заливает её в буферы устройства.
///
/// Без устройства делать нечего и не страшно: набор лежит в состоянии, и его
/// зальёт то делегирование места, которое устройство создаст.
fn upload_outlines(state: &mut State) {
    let State { device: Some(device), outlines, .. } = state else { return };
    let built = outlines::Outlines::build(outlines);
    // Числа полезны ровно при разборе «почему их не видно»: контуры бывают
    // мелкими (клетка Sentinel-2 — около градуса) и бывают за горизонтом, и
    // отличить это от «не доехали» больше нечем.
    veldsdk::log::debug!(target: "render", "контуры: {} колец, {} вершин",
        outlines.len(), built.vertices.len());
    if let Err(error) = device.set_outlines(&built) {
        veldsdk::log::error!(target: "render", "контуры не залиты: {:#}", error);
    }
}

/// Кадровый тик. Остальные события окна не наши: курсор и клавиши разбирает
/// тот, кто рисует разметку, и до нас доходит уже разобранное.
pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    if !matches!(event.event, Some(app_proto::ui_event::Event::Frame(_))) {
        return;
    }
    let (Some(device), Some(target)) = (&state.device, &state.target) else { return };

    let now = Frame {
        camera: state.camera,
        texture: target.texture_id,
        generation: state.generation,
    };
    if state.drawn == Some(now) {
        return;
    }

    if let Err(error) = gpu::render(device, target, &state.camera) {
        veldsdk::log::error!(target: "render", "кадр не записан: {:#}", error);
        return;
    }
    state.drawn = Some(now);
}
