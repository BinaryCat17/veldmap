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
pub mod cull;
pub mod geodesy;
pub mod gpu;
pub mod mesh;
pub mod outlines;
pub mod overlay;
pub mod perf;
pub mod projection;
pub mod wheel;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use camera::Camera;
use gpu::{Device, Target};
use overlay::{Overlay, Raster, Role};
use veldmap_image_tiler_wrap::tiles::{self, Addr, Missed, Passes, Store};
use veldsdk::proto::app as app_proto;
use veldsdk::proto::core::SurfaceDelegated;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest, TileAddr,
    TileResult as ProducedTile,
};
use crate::proto::globe::{OverlayProgress, OverlaysProgress};
use crate::proto::tile_cache::{
    QueryDone, QueryRequest, TileAddr as QueryAddr, TileResult as CachedTile,
};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Бюджет видеопамяти под тайлы наложений, МиБ. Вытеснение — забывание:
    /// тайл остаётся на диске у tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
}

/// Чей это ответ кэша и о чём спрашивали. Отпечаток свой, а не из растра:
/// наложение могли заменить, пока ответ шёл, и класть его тайлы под новый
/// отпечаток нельзя.
pub struct QueryCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

/// То же для производителя.
pub struct ProduceCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

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
    /// Где стои́т волна ряби, 0..1 периода.
    phase: f32,
    /// Сила ряби, если в кадре есть чему рябить; `None` — нечему, и фаза стои́т.
    ///
    /// Считается пересборкой патчей — по тем же ячейкам, которые и рисуются, —
    /// и держится полем потому, что кадровый тик спрашивает её раньше, чем
    /// собирает отпечаток кадра.
    ripple: Option<f32>,
    /// Последний присланный набор контуров. Хранится потому, что приехать он
    /// может раньше места под рендер, а залить его в буферы можно только с
    /// готовым устройством.
    outlines: Vec<crate::proto::globe::Outline>,
    /// Сколько раз набор контуров сменился. Сами контуры в [`Frame`] не
    /// сравнить, не держа их копию только ради сравнения, — а счётчик отвечает
    /// на тот же вопрос: набор с прошлого кадра тот же или уже другой.
    generation: u64,

    /// Наложения в порядке прихода — он же порядок отрисовки.
    overlays: Vec<Overlay>,
    /// Наложения, которых мы не приняли, и почему: без привязки, без единого
    /// открывшегося растра. Своими наложениями они не стали и лежать им негде,
    /// но сказать о них надо — приславший считает их лежащими на шаре, и
    /// молчание оставило бы у него вечно «готовится…».
    ///
    /// Живут до следующего набора: приславший на такое извещение слой убирает,
    /// а набор от этого приезжает заново.
    refused: Vec<(String, String)>,
    /// Текстуры тайлов наложений, общие всем наложениям, с бюджетом.
    tiles: Store,
    /// Идущие проходы производителя, по одному на источник. Не у растра:
    /// проход принадлежит файлу, а растров с одним файлом бывает несколько.
    /// Заказчик здесь — слой и роль растра: снять проход с учёта вправе только
    /// тот, кто его завёл.
    passes: Passes<(String, Role)>,
    /// Собранные варп-патчи: буфер вершин, блоки номеров по густоте и отрисовки.
    batch: gpu::OverlayBatch,
    /// Вершины патчей, пережившие прошлую пересборку. Держатся здесь ради
    /// ёмкости, а не ради содержимого: состав меняется на всякое движение
    /// камеры, а у глобального растра это мегабайты (сотни ячеек, у каждой
    /// узлов по густоте её варп-сетки, размером с [`gpu::OverlayVertex`]),
    /// которые заведённый заново вектор растил бы переаллокациями каждый раз.
    ///
    /// Ёмкость набирается по худшей сцене и назад не отдаётся. Возвращать её
    /// некому: линейную память wasm аллокатор хосту не отдаёт, так что
    /// освобождённый вектор освободил бы место только для себя же.
    patch_vertices: Vec<gpu::OverlayVertex>,
    /// То, из чего собраны нынешние патчи: выборы растров, накрывшие их тайлы и
    /// прозрачность слоёв (она запечена в вершинах). Разошлось с нынешним —
    /// пересборка (см. [`build_patches`]).
    built: Option<Vec<overlay::Built>>,
    /// Сколько раз патчи пересобирались — их вклад в сравнение кадра.
    patches: u64,
    /// Смены состава наложений и хода добычи: принятие, описание, снятие, конец
    /// прохода. Вместе с камерой и поколением хранилища отвечает на вопрос «мог
    /// ли измениться выбор или ход»: пока все трое прежние, кадровый тик не
    /// пересчитывает ни того, ни другого.
    epoch: u64,
    /// При чём выборы проверялись в прошлый раз.
    ///
    /// Взгляд целиком, а не камера: он считается и по камере, и по месту под
    /// кадр (`looking`), так что смена размера области меняет выбор уровня
    /// ничуть не меньше, чем движение камеры. Камерой одной ворота этого не
    /// заметили бы.
    checked: Option<(overlay::Look, u64, u64)>,
    pending_describe: veldsdk::Correlator<(String, Role)>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    /// Чего стоит пересчёт желаемого: сколько занимает обход сетки и сколько
    /// ячеек он проверил впустую. Считается это только здесь.
    perf: perf::Meter,
}

/// То, из чего собран кадр. Совпало с нынешним — рисовать нечего.
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    camera: Camera,
    texture: u64,
    generation: u64,
    patches: u64,
    /// Фаза ряби, огрублённая до шага показа. Пока рябить нечему, она стои́т, и
    /// кадр пропускается ровно так же, как до неё; как только в кадре появилась
    /// едущая ячейка, она двигается каждым тиком — и тем самым сама включает
    /// перерисовку на всё время загрузки. Отдельного «перерисовать» поэтому не
    /// нужно: сравнение кадра здесь и есть тот выключатель.
    ripple: u32,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        camera: Camera::default(),
        device: None,
        target: None,
        drawn: None,
        phase: 0.0,
        ripple: None,
        outlines: Vec::new(),
        generation: 0,
        overlays: Vec::new(),
        refused: Vec::new(),
        tiles: Store::new(config.vram_budget_mb * 1024 * 1024),
        passes: Passes::default(),
        batch: gpu::OverlayBatch::new(),
        patch_vertices: Vec::new(),
        built: None,
        patches: 0,
        epoch: 0,
        checked: None,
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
        perf: perf::Meter::default(),
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
        // Без места добыча стои́т: видимого прямоугольника не существует, и
        // `want_tiles` выходит первой же строкой. Сказать об этом надо — иначе
        // у показывающего список остаётся последний присланный счёт, и
        // остановившаяся работа выглядит идущей и зависшей.
        report_progress(state, &[]);
        return;
    };
    if req.width == 0 || req.height == 0 {
        veldsdk::log::warn!(target: "handlers", "место {}x{} — рисовать негде", req.width, req.height);
        return;
    }

    if state.device.as_ref().is_none_or(|device| device.format != req.format) {
        match Device::create(req.format) {
            Ok(device) => {
                // Bind group'ы тайлов собраны под layout прежнего устройства —
                // с новым они несовместимы. Хранилище опустошается: тайлы
                // лежат на диске и вернутся за миллисекунды.
                state.tiles = Store::new_like(&state.tiles);
                state.built = None;
                // Поколение опустевшего хранилища начинается заново, то есть
                // перестаёт расти. Прежний ключ ворот с тем же числом внутри
                // объявил бы пересчёт лишним над пустым местом.
                state.checked = None;
                state.device = Some(device);
            }
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
    want_tiles(state, perf::Pass::Set);
}

pub fn on_camera(state: &mut State, command: crate::proto::globe::CameraCommand) {
    use crate::proto::globe::camera_command::Command;
    match command.command {
        Some(Command::Orbit(orbit)) => state.camera.orbit(orbit.dx, orbit.dy),
        Some(Command::Zoom(zoom)) => {
            state.camera.zoom_at(zoom.delta, zoom.hold_x, zoom.hold_y)
        }
        // Наводка без точки — это наводка в никуда: молча смотреть в центр
        // координат хуже, чем не двигаться вовсе.
        Some(Command::Focus(focus)) => match focus.at {
            Some(at) => state.camera.focus(at.lat, at.lon, focus.radius_deg),
            None => veldsdk::log::warn!(target: "handlers", "наводка без точки"),
        },
        None => {}
    }
    // Приближение меняет уровень тайлов наложений — спросить недостающее.
    //
    // Наводке этого мало: она только назначает цель, а камера доедет за
    // полсекунды, и спрошенное отсюда описывает место, откуда она вылетела.
    // Досматривает за ней кадровый тик (см. [`on_ui_event`]).
    want_tiles(state, perf::Pass::Camera);
}

/// Что под указателем. Отвечаем всегда: «мимо Земли» — такой же ответ, как
/// точка, и спрашивающий вправе его получить. Без места под рендер ответ тот
/// же: кадра нет — значит нет и точки кадра, про которую спрашивают.
pub fn on_probe(state: &mut State, probe: crate::proto::globe::Probe) {
    let at = state.target.as_ref().and_then(|target| {
        state
            .camera
            .probe(probe.x, probe.y, target.aspect())
            .map(|(lat, lon)| crate::proto::globe::GeoPoint { lat, lon })
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

// ── Наложения ──────────────────────────────────────────────────

/// Что наложить на поверхность. Набор целиком: чего не прислали — того больше
/// нет, и его ресурсы освобождаются здесь. Наложение с тем же ключом и теми же
/// ресурсами остаётся как есть — его тайлы и ожидания продолжают жить.
pub fn on_overlay(state: &mut State, msg: crate::proto::globe::Overlays) {
    let keep: HashSet<String> = msg.overlays.iter().map(|o| o.key.clone()).collect();
    // Прошлые отказы кончились вместе с прошлым набором: этот прислали уже
    // зная о них.
    state.refused.clear();

    // Снятые — прочь: производство убить, тайлы забыть, ресурсы освободить.
    let removed: Vec<Overlay> = {
        let (kept, removed) =
            state.overlays.drain(..).partition(|overlay| keep.contains(&overlay.key));
        state.overlays = kept;
        removed
    };
    for overlay in removed {
        veldsdk::log::info!(target: "handlers", "{}: наложение снято", overlay.label);
        drop_overlay(state, overlay);
    }

    // Порядок слоёв — порядок сообщения, поэтому набор пересобирается по нему,
    // а не дополняется. Иначе переставленный у отправителя список ничего бы не
    // переставил: принятое наложение осталось бы на месте своего первого
    // прихода, и «поднять снимок наверх» молча ничего не делало бы.
    let order: Vec<String> = msg.overlays.iter().map(|overlay| overlay.key.clone()).collect();
    for incoming in msg.overlays {
        adopt_overlay(state, incoming);
    }
    state.overlays.sort_by_key(|overlay| {
        order.iter().position(|key| key == &overlay.key).unwrap_or(usize::MAX)
    });

    state.epoch += 1;
    want_tiles(state, perf::Pass::Set);
}

/// Потолок аппетита одного уровня. Правило общее с канвой и живёт у бюджета
/// (`Store::cap_tiles`); здесь только ответ на «сколько пирамид сейчас
/// рисуется».
///
/// Пирамид, а не слоёв: у слоя их до двух — превью и подробный, — и лежат в
/// бюджете обе. Считаются описанные, а не выбранные нынешним кадром: выбор
/// зависит от потолка, и вывести потолок из него значило бы замкнуть их друг
/// на друга. Ошибка тогда в безопасную сторону — потолок ниже, а не выше.
///
/// Спрашивают его двое — тот, кто просит тайлы, и тот, кто собирает патчи, — и
/// разойтись им нельзя: заказанный уровень обязан быть тем же, который рисуют.
fn cap_tiles(state: &State) -> u64 {
    state.tiles.cap_tiles(
        state
            .overlays
            .iter()
            .filter(|overlay| !overlay.hidden)
            .flat_map(|overlay| overlay.budgeted())
            .filter_map(|raster| raster.meta.as_ref())
            .map(|meta| meta.fingerprint.as_str()),
    )
}

/// Как показывать слой: прозрачность и скрытость. Не сказанная прозрачность —
/// непрозрачный слой: нулевое умолчание proto3 означало бы, что отправитель,
/// которому до неё нет дела, гасит снимок молчанием (см. types.proto).
fn look(incoming: &crate::proto::globe::Overlay) -> (f32, bool) {
    (incoming.opacity.unwrap_or(1.0).clamp(0.0, 1.0), incoming.hidden)
}

/// Принять одно наложение из сообщения. Ресурсы растров приходят во владение;
/// то же наложение с теми же ресурсами — не событие, но как его показывать,
/// берётся из нового сообщения в любом случае.
fn adopt_overlay(state: &mut State, incoming: crate::proto::globe::Overlay) {
    if incoming.key.is_empty() {
        veldsdk::log::warn!(target: "handlers", "наложение без ключа — ресурсы освобождаются");
        release_rasters(incoming.rasters);
        return;
    }
    let (opacity, hidden) = look(&incoming);

    // Те же ресурсы под тем же ключом — наложение уже наше, хвост владения
    // остался у нас с прошлого сообщения.
    let incoming_ids: Vec<u64> = incoming
        .rasters
        .iter()
        .flat_map(|raster| [raster.resource.as_ref(), raster.geolocation.as_ref()])
        .flatten()
        .map(|handle| handle.id)
        .collect();
    if let Some(index) = state.overlays.iter().position(|o| o.key == incoming.key) {
        if state.overlays[index].sources == incoming_ids {
            // Растры те же — трогать нечего, кроме показа: слайдер
            // прозрачности и «скрыть» шлют тот же набор, и переоткрывать под
            // них ресурсы значило бы платить за движение ползунка декодом.
            //
            // Рамка из сообщения при этом отбрасывается, и намеренно: слой
            // мог уже привязаться самим растром, а контур каталога — младше
            // (см. `overlay::Binding`). Поправленный каталогом контур доедет
            // только со сменой ресурсов, то есть новым наложением; для тех же
            // растров он и не изменится.
            let overlay = &mut state.overlays[index];
            let was = overlay.hidden;
            overlay.opacity = opacity;
            overlay.hidden = hidden;
            // Скрытый тайлов не просит — и идущий проход тоже его: производство
            // у источника одно, и держать его невидимым слоем значит не давать
            // добывать тем, кого видно. Показ обратно спросит заново, а то, что
            // уже добыто, никуда не делось.
            if hidden && !was {
                let sources: Vec<(Role, String)> = overlay
                    .rasters
                    .iter_mut()
                    .filter_map(|raster| {
                        raster.fetch.reset();
                        raster.meta.as_ref().map(|meta| (raster.role, meta.fingerprint.clone()))
                    })
                    .collect();
                let key = incoming.key.clone();
                for (role, fingerprint) in sources {
                    release_pass(state, &key, role, &fingerprint);
                }
                state.epoch += 1;
            }
            return;
        }
        let old = state.overlays.remove(index);
        drop_overlay(state, old);
    }

    let label = if incoming.label.is_empty() { incoming.key.clone() } else { incoming.label };

    // Привязка: точная рамка UTM либо заявленная аппроксимация квадом. Ранг
    // едет вместе с рамкой — по её виду его не узнать (см. `overlay::Binding`).
    let (frame, binding) = if let Some(utm) = incoming.utm {
        (
            overlay::Frame::utm(
                projection::System::utm(utm.zone, utm.south),
                utm.x0,
                utm.y0,
                utm.x1,
                utm.y1,
            ),
            overlay::Binding::Named,
        )
    } else if incoming.quad.len() == 4 {
        let points = [
            (incoming.quad[0].lat, incoming.quad[0].lon),
            (incoming.quad[1].lat, incoming.quad[1].lon),
            (incoming.quad[2].lat, incoming.quad[2].lon),
            (incoming.quad[3].lat, incoming.quad[3].lon),
        ];
        let frame = match incoming.rough {
            true => overlay::Frame::rough(points),
            false => overlay::Frame::quad(points),
        };
        (frame, overlay::Binding::Catalogue)
    } else {
        release_rasters(incoming.rasters);
        return refuse(state, incoming.key, &label, "снимку негде лежать: привязки нет");
    };

    let mut rasters = Vec::new();
    for raster in incoming.rasters {
        // Координаты приходят во владение вместе с растром, и без него они
        // никому не нужны: отпускать их надо на каждом выходе из этого круга,
        // иначе файл остаётся открытым до конца жизни модуля.
        let Some(handle) = raster.resource else {
            release_coordinates(raster.geolocation);
            continue;
        };
        let ordinal = raster.ordinal;
        let role = match raster.role() {
            crate::proto::globe::OverlayRole::OverlayPreview => Role::Preview,
            crate::proto::globe::OverlayRole::OverlayDetailed => Role::Detailed,
        };
        // Грант тайлеру до владения: при отказе хелпер освобождает ресурс
        // сам, и заворачивать его во владельца было бы вторым освобождением.
        if let Err(error) = veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
            veldsdk::log::warn!(target: "handlers", "{}: грант растра: {}", label, error);
            release_coordinates(raster.geolocation);
            continue;
        }
        // Координаты — второй ресурс того же растра: у Sentinel-3 широта с
        // долготой лежат в соседнем файле, и без них полосе съёмки негде лечь.
        // Не открылись — растр остаётся при своём: привязку он либо несёт сам,
        // либо её не будет вовсе, и слой кончится обычным отказом.
        let coordinates = raster.geolocation.filter(|handle| {
            match veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
                Ok(()) => true,
                Err(error) => {
                    veldsdk::log::warn!(target: "handlers",
                        "{}: грант координат: {}", label, error);
                    false
                }
            }
        });
        // Второй растр той же роли — запасной первого, а не сосед: описывается
        // он, только когда первый не описался (см. [`Raster::spares`]).
        if let Some(first) = rasters.iter_mut().find(|raster: &&mut Raster| raster.role == role) {
            first.spares.push((
                ordinal,
                veldsdk::OwnedResource::new(handle),
                coordinates.map(veldsdk::OwnedResource::new),
            ));
            continue;
        }
        let mut raster = Raster::new(role, veldsdk::OwnedResource::new(handle.clone()));
        raster.ordinal = ordinal;
        raster.geolocation = coordinates.clone().map(veldsdk::OwnedResource::new);

        let correlation = raster.describe.begin();
        state.pending_describe.insert(correlation.clone(), (incoming.key.clone(), role));
        crate::calls::image_tiler::on_describe(
            &DescribeRequest {
                resource: Some(handle),
                label: label.clone(),
                geolocation: coordinates,
                variable: String::new(),
            },
            &correlation,
        );
        rasters.push(raster);
    }
    if rasters.is_empty() {
        return refuse(state, incoming.key, &label, "ни один растр не открылся");
    }

    veldsdk::log::info!(target: "handlers", "{}: наложение из {} растров", label, rasters.len());
    state.overlays.push(Overlay {
        key: incoming.key,
        label,
        frame,
        relaid: 0,
        binding,
        binding_trouble: None,
        rasters,
        sources: incoming_ids,
        opacity,
        hidden,
        error: String::new(),
        progress: overlay::Progress::default(),
    });
}

/// Наложение не принято. Своим оно не стало, поэтому и причина живёт отдельно
/// от набора — до следующего набора (см. `State::refused`).
fn refuse(state: &mut State, key: String, label: &str, why: &str) {
    veldsdk::log::warn!(target: "handlers", "{}: {}", label, why);
    state.refused.push((key, why.to_string()));
}

/// Конец наложения: производство убить, тайлы забыть (если отпечаток не живёт
/// в другом наложении), ресурсы освободить их Drop'ом.
fn drop_overlay(state: &mut State, mut overlay: Overlay) {
    let key = std::mem::take(&mut overlay.key);
    let sources: Vec<(Role, String)> = overlay
        .rasters
        .iter_mut()
        .filter_map(|raster| {
            raster.fetch.reset();
            raster.meta.as_ref().map(|meta| (raster.role, meta.fingerprint.clone()))
        })
        .collect();

    for (role, fingerprint) in sources {
        release_pass(state, &key, role, &fingerprint);
        // Тайлы забываются по другому счёту, чем убивается проход: держит их и
        // скрытый слой — показ обратно тогда мгновенный, — а вот работать на
        // скрытого не за чем.
        let held = state.overlays.iter().any(|other| {
            other.rasters.iter().any(|raster| {
                raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint)
            })
        });
        if !held {
            state.tiles.forget(&fingerprint);
        }
    }
}

/// Растр уходит — вместе со слоем либо под скрытие: его проход уносится с ним.
///
/// Уносится безусловно, даже когда на тот же файл смотрит соседний слой: читает
/// проход не «файл», а тот самый ресурс, который сейчас освободится вместе с
/// растром (см. `tiles::Passes`). Сосед заведёт свой по концу этого.
fn release_pass(state: &mut State, key: &str, role: Role, fingerprint: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &(key.to_string(), role)) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

/// Освободить ресурсы наложения, которое не приживётся.
/// Отпустить координаты растра, который в наложение не попал. Отдельной
/// функцией затем, что выходов из круга приёма несколько, а забытый на любом
/// из них файл держится открытым до выгрузки модуля.
fn release_coordinates(coordinates: Option<veldsdk::proto::core::ResourceHandle>) {
    if let Some(handle) = coordinates {
        veldsdk::resource::release(handle);
    }
}

fn release_rasters(rasters: Vec<crate::proto::globe::OverlayRaster>) {
    for raster in rasters {
        // Координаты приходят во владение так же, как сам растр, и отпускать
        // их надо тем же движением: оставленные, они держали бы открытым файл,
        // о котором больше некому вспомнить.
        for handle in [raster.resource, raster.geolocation].into_iter().flatten() {
            veldsdk::resource::release(handle);
        }
    }
}

// ── Ответы конвейера тайлов ────────────────────────────────────

/// Растр описан — или не описан: наложение живёт тем, что есть, у превью и
/// подробного растра свои судьбы.
///
/// Всякий исход двигает эпоху и пересчитывает нужное, даже отказ: до ответа
/// наложение с привязкой-догадкой не рисуется вовсе (см.
/// `Overlay::binding_pending`), и молчаливый выход оставил бы его невидимым до
/// следующего движения камеры.
pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some((key, role)) = state.pending_describe.take(&correlation) else { return };
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
    if overlay.raster_mut(role).is_none_or(|raster| {
        raster.describe.settle(&correlation) != veldsdk::Reply::Current
    }) {
        return;
    }

    describe_settled(state, &key, role, msg);
    rebuild_bounds(state, &key);

    // Описания кончились — время сказать о том, что видно только сейчас.
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && !overlay.rasters.iter().any(|raster| raster.describe.is_pending())
    {
        // Привязка так и осталась догадкой: снимок вот-вот ляжет по контуру
        // каталога, и повёрнутый снимок на шаре ничем другим не объясняется.
        //
        // Только в лог: четырьмя вершинами футпринта ложится всякий снимок,
        // растр которого о привязке не заговаривал вовсе, — квиклук, картинка
        // без геоключей, — и это штатный исход, а не беда. Подпись у каждого
        // такого слоя была бы ровно тем шумом, против которого затевалась
        // жалоба. Растр, который о привязке заговорил и не дал её, скажет о
        // себе сам — через `complain`.
        if matches!(overlay.frame, overlay::Frame::Quad(_)) {
            veldsdk::log::warn!(target: "handlers",
                "{}: привязка из растров не взята — снимок ложится по контуру каталога, порядок его вершин обходу растра не обязан совпадать",
                overlay.label);
        }
        // А вот описаться не вышло ни одному — тогда слою нечем лечь вовсе, и
        // ждать больше нечего: молчание оставило бы у приславшего вечное
        // «готовится…».
        if !overlay.rasters.iter().any(|raster| raster.meta.is_some()) {
            veldsdk::log::warn!(target: "handlers",
                "{}: ни один растр не описался — накладывать нечего", overlay.label);
            // Наружу уезжает не «не описался», а то, чем именно: причину знает
            // тайлер и уже сказал её словами («это измерения, а не
            // изображение», «скачайте его, и он покажется»), а показывает её
            // приславший — у него список и место под подпись. Общая фраза на
            // её месте объясняла бы ровно ничего и отнимала бы у человека
            // единственное, что он может с этим сделать.
            //
            // Растров бывает два, и причины у них разные: обе и называются —
            // их собирает `trouble`. Пусто — сказать нечего, и тогда общая
            // фраза честнее пустой строки.
            let said = overlay.trouble();
            overlay.error = match said.is_empty() {
                true => "ни один растр не описался".to_string(),
                false => said,
            };
        } else if !overlay.frame.measurable() {
            // Рамка есть, а протяжённости у неё нет: контур каталога сошёлся в
            // точку или в линию. Ни ячейки по ней не нарисовать, ни уровня не
            // выбрать — а молча оставленный, такой слой висит «готовится…»
            // и не показывает ничего.
            veldsdk::log::warn!(target: "handlers",
                "{}: у контура каталога нет протяжённости — класть снимок не по чему",
                overlay.label);
            overlay.error = said_with(
                &overlay.binding_trouble,
                "контур снимка сошёлся в точку: класть растр не по чему",
            );
        } else if matches!(overlay.frame, overlay::Frame::Rough(_)) {
            // Место держали в расчёте на решётку из растра, а её не оказалось.
            // Габарит сложного контура привязкой не является (см.
            // `Frame::Rough`), и растянуть по нему снимок было бы неправдой.
            veldsdk::log::warn!(target: "handlers",
                "{}: контур каталога сложнее четырёхугольника, а привязка из растра не взята",
                overlay.label);
            overlay.error = said_with(
                &overlay.binding_trouble,
                "привязки нет: из растра её взять не удалось, а контур каталога сложнее \
                 четырёхугольника",
            );
        }
    }

    state.epoch += 1;
    want_tiles(state, perf::Pass::Set);
}

/// Отказ слоя вместе с причиной, если растр её назвал.
///
/// Внутри общей фразы, а не рядом с ней: по `error` приславший слой снимает, и
/// вместе со слоем уходит его `trouble`, где причина и лежала. Другого случая
/// сказать её не будет — а без неё смотрящий читает про свой снимок ровно то
/// же, что и про всякий другой не легший.
fn said_with(trouble: &Option<String>, general: &str) -> String {
    match trouble {
        Some(said) => format!("{} ({})", general, said),
        None => general.to_string(),
    }
}

/// Пересчитать шары ячеек у всех растров слоя.
///
/// У всех, а не у описавшегося: шары строятся по привязке, а привязка
/// принадлежит слою и меняется описанием любого из растров — решётка опорных
/// точек старше проекции и вытесняет её (см. `Binding`). Оставленные шары
/// превью тогда описывали бы отменённую геометрию, и ячейки терялись бы молча
/// у той самой базы, которую видно первой.
///
/// Место одно на все ветви `describe_settled` — она вся кончается простыми
/// `return`, и любой из них приводит сюда.
fn rebuild_bounds(state: &mut State, key: &str) {
    let Some(at) = state.overlays.iter().position(|o| o.key == key) else { return };
    // Считается всё до единой мутации: шары зависят от слоя и от растра, а оба
    // спрашиваются по ссылке, и разделить эти два заимствования иначе нечем.
    let overlay = &state.overlays[at];
    let built: Vec<Vec<Vec<cull::Ball>>> = overlay
        .rasters
        .iter()
        .map(|raster| match &raster.meta {
            Some(meta) => overlay.bounds(meta),
            None => Vec::new(),
        })
        .collect();
    for (raster, bounds) in state.overlays[at].rasters.iter_mut().zip(built) {
        raster.bounds = bounds;
    }
}

fn describe_settled(state: &mut State, key: &str, role: Role, msg: Described) {
    let Some(mut overlay) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
    let label = overlay.label.clone();

    // Годность описания решает общее правило: тайлер один, пирамида одна, и
    // разойтись с канвой в том, какой ответ считать пригодным, нечем. Своё
    // здесь одно — что делать с непригодным: растров у наложения два, и
    // отказавший один оставляет слой жить вторым, поэтому ошибкой слоя это не
    // становится. Но и молчанием тоже: приблизившийся ждёт резкости, которая
    // теперь не придёт никогда, а по пустой подписи «ещё едет» от «не будет»
    // не отличить. Причина уезжает подписью рядом с полосой хода.
    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: описание растра: {}", label, error);
            // За не описавшимся растром бывает запасной файл — следующий
            // встаёт на его место и описывается сам (см. [`Raster::spares`]).
            let spare = overlay.raster_mut(role).and_then(|raster| {
                let next = raster.next_spare()?;
                Some((raster.describe.begin(), raster.spares.len(), next))
            });
            if let Some((correlation, left, (handle, coordinates))) = spare {
                let named = match role {
                    Role::Detailed => "подробный",
                    Role::Preview => "превью",
                };
                veldsdk::log::info!(target: "handlers",
                    "{}: запасной растр ({}) — ресурс {}, запасных ещё {}", label, named, handle.id, left);
                state.pending_describe.insert(correlation.clone(), (key.to_string(), role));
                crate::calls::image_tiler::on_describe(
                    &DescribeRequest { resource: Some(handle), label, geolocation: coordinates, variable: String::new() },
                    &correlation,
                );
                return;
            }
            // Причина живёт у своего растра: их два, и вторая не отменяет
            // первую — слой живёт, пока жив хоть один, и сказать надо про оба.
            let said = match role {
                Role::Detailed => format!("подробный растр не открылся: {}", error),
                Role::Preview => format!("превью не открылось: {}", error),
            };
            if let Some(raster) = overlay.raster_mut(role) {
                raster.trouble = Some(said);
            }
            return;
        }
    };

    veldsdk::log::info!(target: "handlers", "{}: {}", label, meta.note());

    // Узлы сетки — в доли растра: растров у наложения два и они разного
    // размера, а лежат оба одинаково, и привязка у наложения одна.
    let ties: Vec<overlay::Tie> = msg
        .ties
        .iter()
        .map(|tie| overlay::Tie {
            fx: tie.px / f64::from(msg.width),
            fy: tie.py / f64::from(msg.height),
            lat: tie.lat,
            lon: tie.lon,
        })
        .collect();

    let Some(raster) = overlay.raster_mut(role) else { return };
    raster.describe_as(meta);
    // Описался — своей жалобы у него больше нет. Чужую он не трогает: причина
    // соседа от его успеха никуда не делась.
    raster.trouble = None;

    // Описались оба — и подробный оказался не подробнее базы. Слою он не
    // нужен, но сказать об этом надо: приблизившийся ждёт резкости, которая
    // не придёт, и по молчанию «ещё едет» от «не будет» не отличить.
    //
    // Отпущенное здесь же: ячейки, отложенные до конца прохода, отпускает
    // выбор растров, а выбор к этому растру больше не придёт — и слой стоял
    // бы в «набирается пирамида» до самого снятия.
    let mut eclipsed = None;
    if overlay.detail_eclipsed()
        && let Some(raster) = overlay.raster_mut(Role::Detailed)
    {
        let said = "не подробнее превью: подробный растр на шар не кладётся";
        if raster.trouble.as_deref() != Some(said) {
            veldsdk::log::info!(target: "handlers", "{}: {}", label, said);
        }
        raster.trouble = Some(said.to_string());
        eclipsed = raster.meta.as_ref().map(|meta| meta.fingerprint.clone());
        raster.fetch.reset();
    }
    if let Some(fingerprint) = eclipsed {
        release_pass(state, key, Role::Detailed, &fingerprint);
        // Наложение взято заново: заимствование выше кончилось вместе с
        // отпусканием прохода, а ниже слой нужен целиком.
        let Some(again) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
        overlay = again;
    }

    // Привязка из самого растра главнее всего, что сказал о снимке каталог: там
    // сказано, где он, а здесь — каким пикселем куда. Кто кого перебивает,
    // решает род привязки, а не порядок описания (см. `overlay::Binding`).
    if !ties.is_empty() {
        match overlay::Grid::new(&ties) {
            Some(grid) => {
                // Решётка в четыре узла описывает то же линейное, что и
                // проекция файла, и старше её не становится (см.
                // `overlay::Binding::Lattice`).
                let rank = match grid.is_dense() {
                    true => overlay::Binding::Lattice,
                    false => overlay::Binding::Projected,
                };
                if overlay.binding >= rank {
                    return;
                }
                veldsdk::log::info!(target: "handlers",
                    "{}: привязка сеткой из {} узлов", label, ties.len());
                overlay.relay(overlay::Frame::Grid(grid), rank);
                return;
            }
            // Точки есть, а решётки не вышло — молчать об этом нельзя: снимок
            // ляжет по контуру каталога, то есть, скорее всего, повёрнутым.
            //
            // И выйти отсюда нельзя тоже: файл несёт и опорные точки, и
            // проекцию, и не сложившиеся точки не отменяют вторую. Дальше по
            // тексту её и пробуют.
            None => {
                let said = format!("{} опорных точек не сложились в решётку", ties.len());
                veldsdk::log::warn!(target: "handlers",
                    "{}: {} — привязка остаётся по контуру", label, said);
                overlay.complain(said);
            }
        }
    }

    // Растр лежит в проекции. Код системы толкуется здесь, а не в тайлере:
    // ряды Крюгера в дереве одни (`projection.rs`), и вторая их копия сошлась
    // бы с первой на глаз и разошлась на числах.
    let Some(found) = msg.placement else {
        // Привязки растр не принёс вовсе — и если сказал почему, то это и есть
        // ответ смотрящему: снимок ляжет по контуру каталога, а вопрос у него
        // один — сами мы не сумели или в файле про место не сказано.
        if !msg.binding_trouble.is_empty() {
            veldsdk::log::warn!(target: "handlers",
                "{}: {} — привязка остаётся по контуру", label, msg.binding_trouble);
            overlay.complain(msg.binding_trouble);
        }
        return;
    };
    if overlay.binding >= overlay::Binding::Projected {
        return;
    }
    // Поле в поле, а не по порядку: имена здесь и там одни и те же, и
    // перепутанная пара видна прямо в строке (см. `overlay::Placement`).
    match overlay::Frame::from_placement(&overlay::Placement {
        epsg: found.epsg,
        x_per_i: found.x_per_i,
        x_per_j: found.x_per_j,
        x0: found.x0,
        y_per_i: found.y_per_i,
        y_per_j: found.y_per_j,
        y0: found.y0,
        width: msg.width,
        height: msg.height,
    }) {
        Ok(frame) => {
            veldsdk::log::info!(target: "handlers",
                "{}: привязка проекцией EPSG:{}", label, found.epsg);
            overlay.relay(frame, overlay::Binding::Projected);
        }
        // Система названа, а перевести её нечем. Сказать об этом надо кодом: по
        // «привязки нет» неумение от молчания файла не отличить, и разбирать
        // такую жалобу будет не по чему.
        //
        // И сказать надо не только в лог: снимок ляжет по контуру каталога, то
        // есть, скорее всего, повёрнутым, а смотрящий на шар про лог не знает.
        Err(why) => {
            veldsdk::log::warn!(target: "handlers",
                "{}: растр привязан к {} — привязка остаётся по контуру", label, why);
            overlay.complain(format!("привязка не взята: {}", why));
        }
    }
}

/// Тайл из дискового кэша.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Тайл от производителя.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Кэш ответил всем, чем мог; промахи — производителю.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);

    // Что слою нужно прямо сейчас — этим и отсеиваются промахи. Пока кэш искал,
    // камера могла уехать, и производство единственно: отдав ему ячейку, уже
    // покинувшую кадр, мы занимаем его на минуту тем, чего не видно, — а видимое
    // всё это время ждёт. Уехавшее не теряется: оно переспросится, когда
    // вернётся в кадр (так же считает канва).
    let cap = cap_tiles(state);
    let (toll, began) = (perf::Toll::default(), Instant::now());
    let desired: HashSet<Addr> = looking(state)
        .and_then(|look| {
            let overlay = state.overlays.iter().find(|o| o.key == ctx.key)?;
            Some(
                overlay
                    .wanted(&look, cap, &state.tiles, &toll)
                    .into_iter()
                    .filter(|wanted| wanted.choice.role == ctx.role)
                    .flat_map(|wanted| wanted.want.cells)
                    .collect(),
            )
        })
        .unwrap_or_default();
    state.perf.pass(perf::Pass::Answer, &toll, began.elapsed(), Instant::now());

    let Some((label, produce_list, resource)) = ({
        let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) else { return };
        let label = overlay.label.clone();
        // Слой скрыли, пока кэш искал: заводить под него проход — занять
        // единственное производство тем, чего не видно.
        let hidden = overlay.hidden;
        let Some(raster) = overlay.raster_mut(ctx.role) else { return };
        if hidden {
            raster.fetch.forget_asked(&ctx.cells);
            return;
        }

        // Наложение успели заменить, пока ответ шёл: ожидания того запроса — не
        // об этом растре.
        if raster.meta.as_ref().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
            return;
        }
        // Договорённый хостом ответ — тот же отказ, и путь у него тот же: свои
        // ячейки снять с ожидания, сказать вслух, двинуть эпоху. Принятый за
        // удачу, он оставил бы их висеть навсегда (см. `veldsdk::reply::undelivered`).
        let error = veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone());
        if !error.is_empty() {
            raster.fetch.forget_asked(&ctx.cells);
            veldsdk::log::warn!(target: "handlers", "{}: кэш тайлов: {}", label, error);
            // И на экран: снимок жив, но резче не станет, а по молчанию
            // «ещё едет» от «не будет» не отличить. Тем же полем и теми же
            // словами, что у канвы, — работа у них одна.
            raster.trouble = Some(format!("неполно: {}", error));
            // Эпоху двигаем и здесь: без неё `build_patches` выйдет по
            // сравнению, ход добычи больше не пересчитается, и последнее
            // присланное «идёт работа» останется действующим до конца запуска.
            // Спрашивать заново при этом нечего — отказ кэша не про сеть, а про
            // сам запрос, и повтор дал бы тот же ответ по кругу.
            state.epoch += 1;
            return;
        }

        // Точечно ли читается запрошенный уровень: от этого зависит, ждать ли
        // конца своего прохода молча или переспрашивать кэш (проход кладёт в
        // него больше, чем отдаёт заказчику).
        let pointwise =
            raster.meta.as_ref().is_some_and(|meta| meta.pointwise(level));
        let missed = raster.fetch.missed(
            &state.passes,
            &ctx.fingerprint,
            &(ctx.key.clone(), ctx.role),
            pointwise,
            &ctx.cells,
            msg.misses.iter().map(|addr| (level, addr.x, addr.y)),
            |addr| desired.contains(&addr),
        );
        match missed {
            Missed::Produce(cells) => Some((label, cells, raster.resource.handle())),
            // Ждём чужой проход молча: его конец пересчитает нужное всем.
            Missed::Waiting => return,
            Missed::Closed => None,
        }
    }) else {
        // Идти некуда и ждать нечего: кэш закрыл заказ целиком либо остальное
        // уехало с глаз. Спросить нужное под нынешний кадр надо здесь же —
        // иначе добыча встанет до ближайшего движения камеры (см.
        // `Missed::Closed`).
        state.epoch += 1;
        want_tiles(state, perf::Pass::Fetch);
        return;
    };

    let correlation = state.pending_produce.begin(ProduceCtx {
        key: ctx.key.clone(),
        role: ctx.role,
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    state.passes.begin(
        &ctx.fingerprint,
        (ctx.key.clone(), ctx.role),
        correlation.clone(),
        level,
        produce_list.clone(),
    );
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(resource),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label,
        // Шар величины не называет: у него выбор тайлера.
        variable: String::new(),
    }, &correlation);
}

/// Ход прохода производителя. Единственное, что о работе можно сказать, пока
/// последовательный источник читается насквозь: ячеек за это время не
/// прибавляется ни одной, а читать бывает минуту.
pub fn on_produce_progress(state: &mut State, msg: ProduceProgress) {
    veldsdk::log::debug!(target: "handlers", "производство: {} из {} МиБ, тайлов {}/{}",
        msg.read_bytes >> 20, msg.total_bytes >> 20, msg.done_tiles, msg.want_tiles);

    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else { return };
    let (key, role) = (ctx.key.clone(), ctx.role);
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && let Some(raster) = overlay.raster_mut(role)
    {
        raster.pass = (msg.read_bytes, msg.total_bytes);
    }
    // Эпоху двигаем: без неё кадровый тик выйдет по сравнению, и ход добычи не
    // пересчитается — а он только что и изменился.
    state.epoch += 1;
}

/// Единственный конец производства, каким бы он ни был, — за убитое отвечает
/// хост пустым сообщением.
pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };
    // Сторож снимается раньше всего и безусловно: он стоял на источнике, а не
    // на растре, и наложение могли снять, пока проход шёл. Уйди мы отсюда, не
    // сняв его, — соседние растры того же файла ждали бы конца, которого уже
    // не будет.
    // Сняли проход мы сами — это обычный ход приближения, а не срыв: хост
    // договаривает ответ за снятого тем же пустым сообщением, что и за
    // упавшего, и различить их может только тот, кто снимал.
    let ours_kill = state.passes.finish(&correlation);

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) {
        let label = overlay.label.clone();
        if let Some(raster) = overlay.raster_mut(ctx.role) {
            // Наложение успели заменить, пока проход шёл: его ячейки уже не про
            // этот растр, и относить к нему ни ожидания, ни отказ нельзя.
            let ours =
                raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
            // Пустой ответ, договорённый хостом за упавшего, — это сорвавшийся
            // проход, а не удачный: принятый за удачу, он ещё и простил бы
            // ячейки, которых никто не производил (`Fetch::forgive`).
            let error = match ours_kill {
                true => msg.error.clone(),
                false => veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone()),
            };
            let failed = ours && !error.is_empty();
            raster.pass = (0, 0);
            raster.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
            if failed {
                // Не переспрашивать то, что уже не произвелось: каждый кадр
                // долбил бы производителя тем же отказом.
                veldsdk::log::warn!(target: "handlers", "{}: производство: {}", label, error);
                raster.trouble = Some(format!("неполно: {}", error));
            }
        }
    }

    // Пока проход шёл, запросы по этому источнику откладывались — и не только
    // у заказчика: соседний слой с тем же файлом всё это время ждал молча.
    // Пересчёт идёт по всем наложениям, и разбудить их больше некому.
    //
    // Эпоха двигается и здесь: конец прохода видно только по ней — тайлов он
    // может не принести вовсе, а ход добычи о нём сказать обязан.
    state.epoch += 1;
    want_tiles(state, perf::Pass::Fetch);
}

/// Пересчёт нужного всем наложениям: выбор растра и уровня под взгляд, запрос
/// недостающего у кэша. Производство одного растра единственно: пока оно идёт,
/// новые запросы по нему откладываются; цель загрубилась ниже его уровня —
/// оно убивается (откат ступени целью не является, см. `tiles::Passes::stale`).
///
/// Повод приходит доводом: зовущих мест много, стоят они одинаково, а значат
/// разное (см. `perf::Pass`). Считать их одним числом значит не уметь сказать,
/// чем всплеск вызван.
fn want_tiles(state: &mut State, from: perf::Pass) {
    let Some(look) = looking(state) else { return };
    let cap = cap_tiles(state);

    // Скрытые не просят ничего: тайл, которого не видно, вытеснил бы из
    // бюджета тот, который видно.
    let keys: Vec<String> = state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .map(|overlay| overlay.key.clone())
        .collect();
    // Замер копится по всем наложениям и записывается одним проходом: сборка
    // патчей считает их разом, и записанные здесь поштучно они дали бы по
    // проходу на слой против одного у неё — на том же самом счёте.
    let (toll, mut spent) = (perf::Toll::default(), Duration::ZERO);
    for key in keys {
        let began = Instant::now();
        let wanted = state
            .overlays
            .iter()
            .find(|o| o.key == key)
            .map(|overlay| overlay.wanted(&look, cap, &state.tiles, &toll))
            .unwrap_or_default();
        spent += began.elapsed();
        for wanted in wanted {
            want_overlay(state, &key, wanted);
        }
    }
    state.perf.pass(from, &toll, spent, Instant::now());
}

/// Взгляд, которым меряется желаемое. `None` — места под кадр ещё нет, и
/// видимого прямоугольника не существует.
fn looking(state: &State) -> Option<overlay::Look> {
    let target = state.target.as_ref()?;
    Some(overlay::Look {
        view_proj: state.camera.view_projection(target.aspect()),
        eye: state.camera.eye(),
        mpp: state.camera.metres_per_pixel(target.height),
    })
}

fn want_overlay(state: &mut State, key: &str, wanted: overlay::Wanted) {
    let Some(overlay) = state.overlays.iter().find(|o| o.key == key) else { return };
    let Some(raster) = overlay.raster(wanted.choice.role) else { return };
    let Some(meta) = raster.meta.as_ref() else { return };
    let fingerprint = meta.fingerprint.clone();
    let label = overlay.label.clone();

    let owner = (key.to_string(), wanted.choice.role);
    let pointwise = meta.pointwise(wanted.want.level);
    if let Some(correlation) = state.passes.stale(&fingerprint, &owner, &wanted.want, pointwise) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    let going = state.passes.going(&fingerprint);
    let overlay = state.overlays.iter_mut().find(|o| o.key == key).expect("наложение только что было");
    let raster = overlay.raster_mut(wanted.choice.role).expect("растр только что был");
    // Прохода нет — отложенным на его время ячейкам ждать больше нечего.
    if !going {
        raster.fetch.resume();
    }
    let Some(cells) = raster.fetch.ask(&state.tiles, &fingerprint, wanted.want.cells) else {
        return;
    };

    // Своей строки состояния у глобуса нет, и видно ход добычи только здесь:
    // какой уровень какого растра понадобился и сколько его ячеек не хватает.
    veldsdk::log::debug!(target: "handlers", "{}: {:?} уровень {}, ячеек {}",
        label, wanted.choice.role, wanted.choice.level, cells.len());

    let correlation = state.pending_query.begin(QueryCtx {
        key: key.to_string(),
        role: wanted.choice.role,
        fingerprint: fingerprint.clone(),
        cells: cells.clone(),
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level: wanted.choice.level,
        tiles: cells.iter().map(|&(_, x, y)| QueryAddr { x, y }).collect(),
        label,
    }, &correlation);
}

/// Тайл приехал — в хранилище и вон из ожиданий.
///
/// С ожиданий ячейка снимается в любом случае: ответ про неё пришёл, а не
/// легший тайл — это промах, и переспросится он следующим пересчётом. Оставь
/// мы его в ожиданиях — не переспросился бы никогда.
fn accept_tile(
    state: &mut State,
    key: &str,
    role: Role,
    fingerprint: &str,
    addr: Addr,
    texture: Option<veldsdk::proto::core::ResourceHandle>,
    width: u32,
    height: u32,
) {
    let landed = match &state.device {
        Some(device) => state.tiles.land(fingerprint, addr, texture, width, height, |view| {
            device.overlay_bind_group(view)
        }),
        // Устройства нет — рисовать нечем и класть некуда; ячейка при этом ни в
        // чём не виновата и спросится заново, когда место под кадр появится.
        None => {
            tiles::release(texture);
            tiles::Landing::Retry
        }
    };

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && let Some(raster) = overlay.raster_mut(role)
        && raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint)
    {
        match landed {
            tiles::Landing::Landed => raster.fetch.arrived(addr),
            // Осечка наша: ячейка ни в чём не виновата и спросится заново.
            // Ход добычи от этого не меняется — ожидание просто снято.
            tiles::Landing::Retry => {
                // Вторая осечка подряд — уже приговор, и ход добычи меняется
                // ровно так же, как у ветки ниже.
                if raster.fetch.stumbled(addr) {
                    state.epoch += 1;
                }
            }
            tiles::Landing::Verdict => {
                // Безнадёжная ячейка перестаёт держать ступень
                // (`Fetch::hopeless`) и больше не спрашивается, то есть ход
                // добычи меняется. Поколение хранилища об этом не скажет:
                // непринятый тайл в него не лёг и счётчика не сдвинул.
                raster.fetch.rejected(addr);
                state.epoch += 1;
            }
        }
    }
}

/// Пересборка варп-патчей, когда изменилось то, из чего они собраны: состав
/// наложений, их выборы (растр и уровень) или поколение хранилища тайлов.
///
/// Взгляд входит сюда, но не в саму геометрию: вершины патча лежат по привязке
/// и от того, откуда смотрят, не зависят — взгляд двигает только uniform. Зато
/// им отбираются ячейки и берётся уровень, а это и есть выбор.
fn build_patches(state: &mut State) {
    let Some(look) = looking(state) else {
        // Рябить негде: без места под кадр не рисуется ничего, и волна,
        // оставленная идти, звала бы перерисовку впустую.
        state.ripple = None;
        // Места под кадр нет — считать по нему нечего, а сказать есть о чём:
        // отвергнутый слой и слой, кончившийся ошибкой, известны и без кадра.
        // Молча оставленные, они висят у приславшего «готовится…» до тех пор,
        // пока вкладку шара не откроют, — а `on_overlay_progress` у него
        // единственный путь, которым слой снимается по `error`.
        report_progress(state, &[]);
        return;
    };

    // Пока взгляд, хранилище и состав наложений прежние, прежние и выборы —
    // холостой тик не строит даже списка для сравнения.
    let now = (look, state.tiles.generation, state.epoch);
    if state.checked == Some(now) {
        return;
    }
    state.checked = Some(now);

    let cap = cap_tiles(state);

    // Прозрачность едет в списке вместе с выбором, потому что она в патчах и
    // запечена (вершиной): движение ползунка выборов не меняет, и без неё
    // сравнение решило бы, что пересобирать нечего.
    //
    // Порядок списка — порядок отрисовки, а он и есть порядок набора: скрытые
    // из него выпадают целиком.
    let (toll, began) = (perf::Toll::default(), Instant::now());
    let wanted: Vec<(String, f32, overlay::Wanted)> = state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .flat_map(|overlay| {
            overlay
                .wanted(&look, cap, &state.tiles, &toll)
                .into_iter()
                .map(|wanted| (overlay.key.clone(), overlay.opacity, wanted))
                .collect::<Vec<_>>()
        })
        .collect();
    // Замер записывается здесь, а не концом функции: ниже лежит выход по
    // совпавшему `built`, и записанный после него проход терялся бы как раз в
    // самом частом случае — когда обход посчитан, а выбор не изменился.
    state.perf.pass(perf::Pass::Patches, &toll, began.elapsed(), Instant::now());
    // Ход добычи считается здесь же, из того же списка: спрашивают его о том
    // же самом — что нужно наложению прямо сейчас, — и посчитанный отдельно он
    // разошёлся бы с рисуемым на глазах у смотрящего в список.
    report_progress(state, &wanted);

    // Носители спрашиваются до сравнения отпечатка, а не в самой сборке: ими он
    // и меряется. Пока ячейки накрыты теми же тайлами, вершины вышли бы теми же,
    // сколько бы ни менялось хранилище вокруг (см. `Built::cells`).
    //
    // Стои́т это обхода цепочки предков на ячейку, и платится он теперь на
    // всяком тике, прошедшем ворота, а не только на пересборке. Обращения при
    // этом продлевают тайлам жизнь в бюджете — но не ради этого: вытеснение
    // идёт только с приездом (`Store::accept`), а приезд двигает поколение и
    // открывает ворота сам.
    let mut pieces: Vec<Vec<overlay::Piece>> = Vec::with_capacity(wanted.len());
    {
        let State { overlays, tiles, .. } = &mut *state;
        for (key, _, wanted) in &wanted {
            let found = overlays.iter().find(|o| &o.key == key);
            pieces.push(found.map_or_else(Vec::new, |o| overlay::pieces(o, wanted, tiles)));
        }
    }

    // Сила ряби — до сравнения отпечатка: ячейки уже посчитаны, а ниже лежит
    // выход по совпадению, за которым её было бы негде взять. Наибольшая из
    // слоёв: рябь одна на кадр, и слой, которому ехать дольше всех, задаёт её
    // целиком.
    state.ripple = wanted
        .iter()
        .zip(&pieces)
        .filter(|(_, pieces)| pieces.iter().any(overlay::Piece::coming))
        .filter_map(|((key, ..), _)| state.overlays.iter().find(|o| &o.key == key))
        .map(|overlay| ripple_strength(overlay.progress.within))
        .max_by(f32::total_cmp);

    // Что вошло в отпечаток и почему — у него самого (`Overlay::built`).
    let stamp: Vec<overlay::Built> = wanted
        .iter()
        .zip(&pieces)
        .filter_map(|((key, _, wanted), pieces)| {
            let drawn: Vec<overlay::Drawn> = pieces.iter().map(overlay::Piece::drawn).collect();
            Some(state.overlays.iter().find(|o| &o.key == key)?.built(wanted, &drawn))
        })
        .collect();
    if state.built.as_ref() == Some(&stamp) {
        return;
    }

    let mut draws = Vec::new();
    let assembled = Instant::now();
    let State { overlays, patch_vertices, .. } = state;
    patch_vertices.clear();
    for ((key, _, wanted), pieces) in wanted.iter().zip(&pieces) {
        let Some(overlay) = overlays.iter_mut().find(|o| &o.key == key) else { continue };
        overlay::patches(overlay, wanted, pieces, patch_vertices, &mut draws);
    }

    let filled = state.batch.fill(&state.patch_vertices, &draws);
    // Замер здесь, а не сразу после сборки, и до разбора исхода: заливка — та же
    // пересборка, и уезжает в неё столько же, сколько собрано, а неудавшаяся
    // стоила ровно столько же, сколько удавшаяся.
    state.perf.rebuilt(state.patch_vertices.len(), assembled.elapsed());
    match filled {
        Ok(()) => {
            state.patches += 1;
            state.built = Some(stamp);
        }
        Err(error) => {
            veldsdk::log::error!(target: "render", "патчи наложений не залиты: {:#}", error);
        }
    }
}

/// Период волны ряби в секундах: столько занимает один её пробег через фазу.
const RIPPLE_PERIOD_S: f32 = 1.6;

/// На сколько долей периода бьётся фаза в отпечатке кадра. Тридцать шагов в
/// секунду — предел, за которым глаз перестаёт различать движение волны, а
/// кадры сверх него были бы перерисовкой ради одного и того же.
const RIPPLE_STEPS: f32 = RIPPLE_PERIOD_S * 30.0;

/// Куда ушла фаза за этот тик и чем кадр отличается от прошлого.
///
/// Чистой функцией, потому что стережёт она главную опасность правки — вечно
/// рисующий шар: пока рябить нечему, огрублённая фаза обязана стоять на месте,
/// иначе кадровое сравнение перестанет пропускать покой.
fn advance_ripple(phase: f32, dt: f32, ripple: Option<f32>) -> (f32, u32) {
    let phase = match ripple {
        Some(_) => (phase + dt / RIPPLE_PERIOD_S).fract(),
        None => phase,
    };
    (phase, (phase * RIPPLE_STEPS) as u32)
}

/// Насколько заметна рябь на ступени, дошедшей до такой доли.
///
/// Не самой долей: ступень, только начатая, идёт ничуть не меньше той, что
/// вот-вот кончится, и невидимая рябь в её начале означала бы «ничего не
/// происходит» ровно там, где ждать дольше всего. Поэтому доля не задаёт силу,
/// а прибавляет к её основанию.
fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Пересчитать ход добычи по тому, что слою нужно прямо сейчас, и разослать
/// набор целиком.
///
/// Меряется он двумя величинами, и обе выведены из заказа, а не из кадра. Доля
/// пути — это ступени пирамиды: от вершины, с которой начинают, до уровня, на
/// котором картинка станет резкой; каждая закрывается один раз, и панорама их
/// не двигает. Счёт ячеек — это последний оформленный заказ
/// (`tiles::Fetch::ordered`), а не то, что попало в кадр на этом кадре.
/// Посчитанное по видимому ездит взад-вперёд на каждом движении камеры и не
/// сообщает ничего.
///
/// Пустой `wanted` — считать не по чему (места под кадр нет, слой скрыт или
/// растр ещё описывают): тогда о слое рассказывается то, что посчитал последний
/// живой кадр. Работа при этом стоящей не объявляется: описание идёт и без
/// кадра, и оно же — самая долгая часть пути растра по сети (см.
/// `Overlay::working`).
///
/// Уезжает только изменившийся набор — топик объявлен снимком, и повтор
/// отсекает его стаб (см. schema.yaml).
fn report_progress(state: &mut State, wanted: &[(String, f32, overlay::Wanted)]) {
    for overlay in &mut state.overlays {
        // Последняя запись, а не сумма: превью и подробный — две разные
        // пирамиды с разными отпечатками, и сложенные в один счёт они меняют
        // знаменатель ровно в тот миг, когда экранный пиксель переходит через
        // родное разрешение превью. Резкость доводит последняя, о ней и речь.
        let Some((.., mine)) = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back()
        else {
            continue;
        };
        let Some(raster) = overlay.rasters.iter().find(|raster| raster.role == mine.choice.role)
        else {
            continue;
        };
        // Байты берутся у того же растра, что и ступени: растров у слоя бывает
        // два, и сложенные из разных пирамид числа рассказали бы о двух разных
        // работах сразу.
        overlay.progress = overlay::progress_of(raster, &mine.want);
    }

    let overlays: Vec<OverlayProgress> = state
        .overlays
        .iter()
        .map(|overlay| {
            // Слою, которому не дают считать (скрыт, места под кадр нет),
            // добывать нечего — потому и `live`. Описание в этот счёт не
            // идёт: оно кадра не спрашивает (см. `Overlay::working`). Всё
            // остальное решает общее правило (`tiles::working`): оно и есть
            // «путь не пройден», а не «что-то в полёте».
            //
            // Второго признака рядом нет намеренно, хотя напрашивается:
            // `share < 1.0` — тот же самый факт, записанный дробью. Доля
            // внутри ступени доходит до единицы ровно тогда, когда пустеют
            // ожидания (`Fetch::ordered` считает дошедшим и то, в чём
            // отказали), так что дробь меньше единицы ровно при «ячейки в
            // полёте либо ступень не последняя». Держать это двумя способами
            // значило бы однажды поправить один и не поправить другой.
            let mine = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back();
            overlay.report(mine.map(|(.., wanted)| wanted), &state.passes)
        })
        // Отвергнутые — теми же строками: наложения у нас нет, а сказать о нём
        // надо, иначе приславший будет считать его лежащим на шаре вечно.
        .chain(state.refused.iter().map(|(key, why)| OverlayProgress {
            key: key.clone(),
            ready: 0,
            total: 0,
            working: false,
            share: 0.0,
            error: why.clone(),
            trouble: String::new(),
            step: 0,
            steps: 0,
            blank: false,
            pass_read: 0,
            pass_total: 0,
            detailed: None,
            detailed_trouble: String::new(),
            detailed_variable: None,
        }))
        .collect();

    crate::emit::on_overlay_progress(&OverlaysProgress { overlays });
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
    veldsdk::log::debug!(target: "render",
        "контуры: {} штук, {} вершин линий, {} вершин ленты, {} вершин штриховки",
        outlines.len(), built.vertices.len(), built.ribbon.len(), built.hatch.len());
    if let Err(error) = device.set_outlines(&built) {
        veldsdk::log::error!(target: "render", "контуры не залиты: {:#}", error);
    }
}

/// Кадровый тик. Остальные события окна не наши: курсор и клавиши разбирает
/// тот, кто рисует разметку, и до нас доходит уже разобранное.
pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    let Some(app_proto::ui_event::Event::Frame(frame)) = event.event else {
        return;
    };
    // Отчёт измерителя — здесь, до всего остального: ниже функция кончается
    // тремя выходами (нет устройства, кадр совпал с нарисованным, отказ
    // записи), и поставленный за любым из них счётчик считал бы не кадры, а
    // подмножество кадров. Выше ставить нельзя тоже: в этот топик едут и
    // движения курсора, и прокрутка, и они завысили бы счёт ровно во время
    // жеста, то есть там, где на него и смотрят.
    if let Some(report) = state.perf.frame(Instant::now()) {
        veldsdk::log::debug!(target: "perf", "{}", report);
    }
    // Наводка на снимок едет, а не прыгает, и ведёт её тик: сама она только
    // назначает цель (см. `Camera::focus`). Подвинувшаяся камера видна ниже
    // сравнением с нарисованным кадром — отдельного «перерисовать» не нужно, а
    // вот тайлы наложений спросить надо: уровень и набор видимых ячеек считают
    // по камере, и заказанное до перелёта описывает место, откуда она вылетела.
    // Тем же движением, что и при жесте, — там `want_tiles` зовётся на каждое
    // событие, и здесь на каждый кадр перелёта.
    if state.camera.advance(frame.dt) {
        want_tiles(state, perf::Pass::Camera);
    }
    // Патчи наложений — до сравнения кадра: пришедшие тайлы и смена уровня
    // видны как их пересборка, и она же двигает счётчик в [`Frame`]. Она же
    // отвечает, есть ли в кадре чему рябить.
    build_patches(state);

    // Фаза двигается только когда есть чему рябить, и огрублённая уезжает в
    // отпечаток кадра — тем и включается перерисовка ровно на время загрузки.
    let (phase, stepped) = advance_ripple(state.phase, frame.dt, state.ripple);
    state.phase = phase;

    let (Some(device), Some(target)) = (&state.device, &state.target) else { return };

    let now = Frame {
        camera: state.camera,
        texture: target.texture_id,
        generation: state.generation,
        patches: state.patches,
        ripple: stepped,
    };
    if state.drawn == Some(now) {
        return;
    }

    let ripple = gpu::Ripple { phase, strength: state.ripple.unwrap_or(0.0) };
    if let Err(error) = gpu::render(device, target, &state.camera, &state.batch, ripple) {
        veldsdk::log::error!(target: "render", "кадр не записан: {:#}", error);
        return;
    }
    state.drawn = Some(now);
}

#[cfg(test)]
mod tests {
    use super::{advance_ripple, ripple_strength, RIPPLE_PERIOD_S};

    /// Пока рябить нечему, огрублённая фаза стои́т — и кадр, собранный из неё,
    /// совпадает с прошлым. Это и есть выключатель перерисовки: сломайся он —
    /// и шар начнёт писать кадр шестьдесят раз в секунду на покое, ничем себя
    /// не выдав, кроме нагретой видеокарты.
    #[test]
    fn на_покое_фаза_ряби_стои́т() {
        let dt = 1.0 / 60.0;
        let (phase, first) = advance_ripple(0.3, dt, None);
        let (_, second) = advance_ripple(phase, dt, None);

        assert_eq!(phase, 0.3, "фаза сдвинулась без повода");
        assert_eq!(first, second, "кадр покоя отличается от прошлого");
    }

    /// А пока есть чему рябить — соседние тики дают разные кадры, иначе волна
    /// не двинется вовсе.
    #[test]
    fn при_ряби_соседние_тики_дают_разные_кадры() {
        let dt = 1.0 / 60.0;
        let (phase, first) = advance_ripple(0.0, dt, Some(1.0));
        let (_, second) = advance_ripple(phase, dt, Some(1.0));

        assert!(phase > 0.0);
        assert_ne!(first, second, "волна стои́т на месте");
    }

    /// Фаза ходит по кругу и за период возвращается к началу: без этого она
    /// росла бы без предела, а огрубление её — вместе с ней.
    #[test]
    fn фаза_ходит_по_кругу() {
        let mut phase = 0.0;
        // Тиками по кадру, а не одним прыжком: округление шага накапливается
        // ровно так же, как в жизни.
        for _ in 0..(RIPPLE_PERIOD_S * 60.0) as u32 {
            phase = advance_ripple(phase, 1.0 / 60.0, Some(1.0)).0;
        }
        assert!((0.0..1.0).contains(&phase), "фаза ушла за период: {phase}");
    }

    /// Сила растёт к концу ступени, но и в самом её начале рябь видна: только
    /// начатая ступень идёт ничуть не меньше той, что вот-вот кончится, и
    /// невидимая рябь означала бы «ничего не происходит» там, где ждать дольше
    /// всего.
    #[test]
    fn рябь_видна_и_в_начале_ступени() {
        assert!(ripple_strength(0.0) > 0.2, "начало ступени не видно вовсе");
        assert!(ripple_strength(1.0) > ripple_strength(0.0), "конец не заметнее начала");
        assert!(ripple_strength(1.0) <= 1.0);
        // Доля приходит посчитанной, но приходит она издалека, и упереться
        // рябь обязана в свои пределы, а не в чужую арифметику.
        assert!(ripple_strength(-1.0) >= 0.0 && ripple_strength(7.0) <= 1.0);
    }
}
