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
pub mod overlay;
pub mod projection;
pub mod tiles;

use std::collections::HashSet;

use camera::Camera;
use gpu::{Device, Target};
use overlay::{Overlay, Raster, Role};
use tiles::{Addr, TileStore};
use veldmap_image_tiler_wrap::pyramid;
use veldsdk::proto::app as app_proto;
use veldsdk::proto::core::SurfaceDelegated;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest, TileAddr,
    TileResult as ProducedTile,
};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileResult as CachedTile};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Бюджет видеопамяти под тайлы наложений, МиБ. Вытеснение — забывание:
    /// тайл остаётся на диске у tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    256
}

/// Байт в текстуре одного тайла — потолками аппетита меряется в них.
const TILE_BYTES: u64 = (pyramid::TILE as u64) * (pyramid::TILE as u64) * 4;

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
    /// Текстуры тайлов наложений, общие всем наложениям, с бюджетом.
    tiles: TileStore,
    /// Собранные варп-патчи и то, из чего они собраны: поколение хранилища и
    /// выборы растров. Разошлось с нынешним — пересборка (см. build_patches).
    batch: gpu::OverlayBatch,
    built: Option<(u64, Vec<(String, overlay::Choice)>)>,
    /// Сколько раз патчи пересобирались — их вклад в сравнение кадра.
    patches: u64,
    /// Смены состава наложений (принятие, описание, снятие). Вместе с камерой
    /// и поколением хранилища отвечает на вопрос «мог ли измениться выбор»:
    /// пока все трое прежние, кадровый тик не пересчитывает выборы вовсе.
    epoch: u64,
    /// При чём выборы проверялись в прошлый раз.
    checked: Option<(Camera, u64, u64)>,
    pending_describe: veldsdk::Correlator<(String, Role)>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
}

/// То, из чего собран кадр. Совпало с нынешним — рисовать нечего.
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    camera: Camera,
    texture: u64,
    generation: u64,
    patches: u64,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        camera: Camera::default(),
        device: None,
        target: None,
        drawn: None,
        outlines: Vec::new(),
        generation: 0,
        overlays: Vec::new(),
        tiles: TileStore::new(config.vram_budget_mb * 1024 * 1024),
        batch: gpu::OverlayBatch::new(),
        built: None,
        patches: 0,
        epoch: 0,
        checked: None,
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
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
            Ok(device) => {
                // Bind group'ы тайлов собраны под layout прежнего устройства —
                // с новым они несовместимы. Хранилище опустошается: тайлы
                // лежат на диске и вернутся за миллисекунды.
                state.tiles = TileStore::new_like(&state.tiles);
                state.built = None;
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
    want_tiles(state);
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
    // Приближение меняет уровень тайлов наложений — спросить недостающее.
    want_tiles(state);
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

// ── Наложения ──────────────────────────────────────────────────

/// Что наложить на поверхность. Набор целиком: чего не прислали — того больше
/// нет, и его ресурсы освобождаются здесь. Наложение с тем же ключом и теми же
/// ресурсами остаётся как есть — его тайлы и ожидания продолжают жить.
pub fn on_overlay(state: &mut State, msg: crate::proto::globe::Overlays) {
    let keep: HashSet<String> = msg.overlays.iter().map(|o| o.key.clone()).collect();

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

    for incoming in msg.overlays {
        adopt_overlay(state, incoming);
    }
    state.epoch += 1;
    want_tiles(state);
}

/// Принять одно наложение из сообщения. Ресурсы растров приходят во владение;
/// то же наложение с теми же ресурсами — не событие.
fn adopt_overlay(state: &mut State, incoming: crate::proto::globe::Overlay) {
    if incoming.key.is_empty() {
        veldsdk::log::warn!(target: "handlers", "наложение без ключа — ресурсы освобождаются");
        release_rasters(incoming.rasters);
        return;
    }

    // Те же ресурсы под тем же ключом — наложение уже наше, хвост владения
    // остался у нас с прошлого сообщения.
    let incoming_ids: Vec<u64> = incoming
        .rasters
        .iter()
        .filter_map(|raster| raster.resource.as_ref().map(|handle| handle.id))
        .collect();
    if let Some(index) = state.overlays.iter().position(|o| o.key == incoming.key) {
        let existing: Vec<u64> =
            state.overlays[index].rasters.iter().map(|raster| raster.resource.id()).collect();
        if existing == incoming_ids {
            return;
        }
        let old = state.overlays.remove(index);
        drop_overlay(state, old);
    }

    let label = if incoming.label.is_empty() { incoming.key.clone() } else { incoming.label };

    // Привязка: точная рамка UTM либо заявленная аппроксимация квадом.
    let frame = if let Some(utm) = incoming.utm {
        overlay::Frame::Utm {
            zone: projection::Zone { number: utm.zone, south: utm.south },
            x0: utm.x0,
            y0: utm.y0,
            x1: utm.x1,
            y1: utm.y1,
        }
    } else if incoming.quad.len() == 4 {
        overlay::Frame::quad([
            (incoming.quad[0].lat, incoming.quad[0].lon),
            (incoming.quad[1].lat, incoming.quad[1].lon),
            (incoming.quad[2].lat, incoming.quad[2].lon),
            (incoming.quad[3].lat, incoming.quad[3].lon),
        ])
    } else {
        veldsdk::log::warn!(target: "handlers", "{}: наложение без привязки — снимку негде лежать", label);
        release_rasters(incoming.rasters);
        return;
    };

    let mut rasters = Vec::new();
    for raster in incoming.rasters {
        let Some(handle) = raster.resource else { continue };
        let role = match raster.role() {
            crate::proto::globe::OverlayRole::OverlayPreview => Role::Preview,
            crate::proto::globe::OverlayRole::OverlayDetailed => Role::Detailed,
        };
        // Грант тайлеру до владения: при отказе хелпер освобождает ресурс
        // сам, и заворачивать его во владельца было бы вторым освобождением.
        if let Err(error) = veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
            veldsdk::log::warn!(target: "handlers", "{}: грант растра: {}", label, error);
            continue;
        }
        let mut raster = Raster::new(role, veldsdk::OwnedResource::new(handle.clone()));

        let correlation = raster.describe.begin();
        state.pending_describe.insert(correlation.clone(), (incoming.key.clone(), role));
        crate::calls::image_tiler::on_describe(
            &DescribeRequest { resource: Some(handle), label: label.clone() },
            &correlation,
        );
        rasters.push(raster);
    }
    if rasters.is_empty() {
        veldsdk::log::warn!(target: "handlers", "{}: у наложения нет растров", label);
        return;
    }

    veldsdk::log::info!(target: "handlers", "{}: наложение из {} растров", label, rasters.len());
    state.overlays.push(Overlay { key: incoming.key, label, frame, rasters });
}

/// Конец наложения: производство убить, тайлы забыть (если отпечаток не живёт
/// в другом наложении), ресурсы освободить их Drop'ом.
fn drop_overlay(state: &mut State, mut overlay: Overlay) {
    for raster in &mut overlay.rasters {
        if let Some((correlation, _)) = raster.produce.take() {
            crate::cancel::image_tiler::on_produce(&correlation);
        }
        if let Some(meta) = &raster.meta {
            let shared = state.overlays.iter().any(|other| {
                other
                    .rasters
                    .iter()
                    .any(|r| r.meta.as_ref().is_some_and(|m| m.fingerprint == meta.fingerprint))
            });
            if !shared {
                state.tiles.forget(&meta.fingerprint);
            }
        }
    }
}

/// Освободить ресурсы наложения, которое не приживётся.
fn release_rasters(rasters: Vec<crate::proto::globe::OverlayRaster>) {
    for raster in rasters {
        if let Some(handle) = raster.resource {
            veldsdk::resource::release(handle);
        }
    }
}

// ── Ответы конвейера тайлов ────────────────────────────────────

pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some((key, role)) = state.pending_describe.take(&correlation) else { return };
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
    let label = overlay.label.clone();
    let Some(raster) = overlay.raster_mut(role) else { return };
    if raster.describe.settle(&correlation) != veldsdk::Reply::Current {
        return;
    }

    // Растр без описания просто не рисуется: наложение живёт тем, что есть, —
    // у превью и подробного растра свои судьбы.
    if !msg.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "{}: описание растра: {}", label, msg.error);
        return;
    }
    if msg.width == 0 || msg.height == 0 || msg.levels == 0 {
        veldsdk::log::warn!(target: "handlers", "{}: растр описан пустым", label);
        return;
    }
    if msg.tile != pyramid::TILE {
        veldsdk::log::warn!(target: "handlers",
            "{}: сторона тайла {} у производителя против {} у глобуса", label, msg.tile, pyramid::TILE);
        return;
    }

    veldsdk::log::info!(target: "handlers",
        "{}: {}×{}, уровней {}, {}", label, msg.width, msg.height, msg.levels,
        if msg.random_access { "произвольный доступ" } else { "последовательный проход" });
    raster.meta = Some(overlay::Meta {
        fingerprint: msg.fingerprint,
        width: msg.width,
        height: msg.height,
        levels: msg.levels,
    });
    state.epoch += 1;
    want_tiles(state);
}

/// Тайл из дискового кэша.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return discard_tile(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Тайл от производителя.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return discard_tile(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Кэш ответил всем, чем мог; промахи — производителю. Желанность здесь не
/// пересчитывается: желаемое наложения — весь выбранный уровень, а устаревший
/// уровень убьёт следующий want_tiles, как у канвы.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) else { return };
    let label = overlay.label.clone();
    let Some(raster) = overlay.raster_mut(ctx.role) else { return };

    // Наложение успели заменить, пока ответ шёл: ожидания того запроса — не
    // об этом растре.
    if raster.meta.as_ref().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
        return;
    }

    if !msg.error.is_empty() {
        for addr in &ctx.cells {
            raster.inflight.remove(addr);
        }
        veldsdk::log::warn!(target: "handlers", "{}: кэш тайлов: {}", label, msg.error);
        return;
    }

    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);
    let missed: Vec<Addr> = msg.misses.iter().map(|addr| (level, addr.x, addr.y)).collect();
    let mut produce_list: Vec<Addr> = Vec::new();
    for addr in missed {
        if !ctx.cells.contains(&addr) {
            continue;
        }
        if !raster.failed.contains(&addr) && raster.produce.is_none() {
            produce_list.push(addr);
        } else {
            raster.inflight.remove(&addr);
        }
    }
    if produce_list.is_empty() {
        return;
    }

    let correlation = state.pending_produce.begin(ProduceCtx {
        key: ctx.key.clone(),
        role: ctx.role,
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    raster.produce = Some((correlation.clone(), level));
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(raster.resource.handle()),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label,
    }, &correlation);
}

pub fn on_produce_progress(_state: &mut State, msg: ProduceProgress) {
    // Ход прохода виден в логе: своей строки состояния у глобуса нет, а
    // parent-fallback уже показывает грубое вместо пустоты.
    veldsdk::log::debug!(target: "handlers", "производство: {} из {} МиБ, тайлов {}/{}",
        msg.read_bytes >> 20, msg.total_bytes >> 20, msg.done_tiles, msg.want_tiles);
}

/// Единственный конец производства, каким бы он ни был, — за убитое отвечает
/// хост пустым сообщением.
pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) else { return };
    let label = overlay.label.clone();
    let Some(raster) = overlay.raster_mut(ctx.role) else { return };

    if raster.produce.as_ref().is_some_and(|(active, _)| *active == correlation) {
        raster.produce = None;
    }
    if raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == ctx.fingerprint) {
        for addr in &ctx.cells {
            raster.inflight.remove(addr);
        }
        if !msg.error.is_empty() {
            // Не переспрашивать то, что уже не произвелось: каждый кадр
            // долбил бы производителя тем же отказом.
            raster.failed.extend(ctx.cells.iter().copied());
            veldsdk::log::warn!(target: "handlers", "{}: производство: {}", label, msg.error);
        }
    }

    // Пока проход шёл, запросы откладывались — пересчитать нужное сейчас.
    want_tiles(state);
}

/// Пересчёт нужного всем наложениям: выбор растра и уровня под взгляд, запрос
/// недостающего у кэша. Производство одного растра единственно: пока оно идёт,
/// новые запросы по нему откладываются; уровень сменился — оно убивается.
fn want_tiles(state: &mut State) {
    let Some(target) = &state.target else { return };
    let mpp = state.camera.metres_per_pixel(target.height);
    // Потолок аппетита одного уровня — половина бюджета: уровень целиком не
    // имеет права вымыть из памяти всё остальное.
    let cap = (state.tiles.capacity_tiles(TILE_BYTES) / 2).max(1);

    let keys: Vec<String> = state.overlays.iter().map(|overlay| overlay.key.clone()).collect();
    for key in keys {
        let choices = state
            .overlays
            .iter()
            .find(|o| o.key == key)
            .map(|overlay| overlay.choices(mpp, cap))
            .unwrap_or_default();
        for choice in choices {
            want_overlay(state, &key, choice);
        }
    }
}

fn want_overlay(state: &mut State, key: &str, choice: overlay::Choice) {
    let Some(overlay) = state.overlays.iter().find(|o| o.key == key) else { return };
    let Some(raster) = overlay.raster(choice.role) else { return };
    let Some(meta) = raster.meta.as_ref() else { return };

    // Недостающие ячейки выбранного уровня — весь уровень: у наложения нет
    // видимого прямоугольника, гранула либо в кадре, либо нет.
    let grid_w = pyramid::grid(pyramid::level_size(meta.width, choice.level));
    let grid_h = pyramid::grid(pyramid::level_size(meta.height, choice.level));
    let mut needed: Vec<Addr> = Vec::new();
    for y in 0..grid_h {
        for x in 0..grid_w {
            let addr = (choice.level, x, y);
            if !state.tiles.contains(&meta.fingerprint, addr)
                && !raster.inflight.contains(&addr)
                && !raster.failed.contains(&addr)
            {
                needed.push(addr);
            }
        }
    }

    let stale = raster
        .produce
        .as_ref()
        .filter(|(_, level)| *level != choice.level)
        .map(|(correlation, _)| correlation.clone());
    let fingerprint = meta.fingerprint.clone();
    let label = overlay.label.clone();

    let overlay = state.overlays.iter_mut().find(|o| o.key == key).expect("наложение только что было");
    let raster = overlay.raster_mut(choice.role).expect("растр только что был");
    if let Some(correlation) = stale {
        crate::cancel::image_tiler::on_produce(&correlation);
        raster.produce = None;
    }
    if raster.produce.is_some() || needed.is_empty() {
        return;
    }

    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for &(_, x, y) in &needed {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    }
    raster.inflight.extend(needed.iter().copied());

    let correlation = state.pending_query.begin(QueryCtx {
        key: key.to_string(),
        role: choice.role,
        fingerprint: fingerprint.clone(),
        cells: needed,
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level: choice.level,
        x0,
        y0,
        x1,
        y1,
        label,
    }, &correlation);
}

/// Тайл приехал — в хранилище и вон из ожиданий. Чей бы он ни был, текстура
/// уже наша: не принять её значит потерять видеопамять.
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
    let Some(texture) = texture else { return };
    let Some(device) = &state.device else {
        return veldsdk::resource::release(texture);
    };
    if width == 0 || height == 0 {
        return veldsdk::resource::release(texture);
    }

    if let Err(error) = state.tiles.insert(device, fingerprint, addr, texture, width, height) {
        veldsdk::log::warn!(target: "handlers", "тайл {}:{}:{} не принят: {}",
            addr.0, addr.1, addr.2, error);
    }

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key) {
        if let Some(raster) = overlay.raster_mut(role) {
            if raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint) {
                raster.inflight.remove(&addr);
            }
        }
    }
}

fn discard_tile(texture: Option<veldsdk::proto::core::ResourceHandle>) {
    if let Some(handle) = texture {
        veldsdk::resource::release(handle);
    }
}

/// Пересборка варп-патчей, когда изменилось то, из чего они собраны: состав
/// наложений, их выборы (растр и уровень) или поколение хранилища тайлов.
/// Камера сюда не входит: мир патчей от неё не зависит, взгляд двигает только
/// uniform.
fn build_patches(state: &mut State) {
    let Some(target) = &state.target else { return };

    // Пока камера, хранилище и состав наложений прежние, прежние и выборы —
    // холостой тик не строит даже списка для сравнения.
    let now = (state.camera, state.tiles.generation, state.epoch);
    if state.checked == Some(now) {
        return;
    }
    state.checked = Some(now);

    let mpp = state.camera.metres_per_pixel(target.height);
    let cap = (state.tiles.capacity_tiles(TILE_BYTES) / 2).max(1);

    let choices: Vec<(String, overlay::Choice)> = state
        .overlays
        .iter()
        .flat_map(|overlay| {
            overlay
                .choices(mpp, cap)
                .into_iter()
                .map(|choice| (overlay.key.clone(), choice))
                .collect::<Vec<_>>()
        })
        .collect();
    if state.built.as_ref() == Some(&(state.tiles.generation, choices.clone())) {
        return;
    }

    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    let State { overlays, tiles, .. } = state;
    for (key, choice) in &choices {
        let Some(overlay) = overlays.iter().find(|o| &o.key == key) else { continue };
        overlay::patches(overlay, choice, tiles, &mut vertices, &mut draws);
    }

    match state.batch.fill(&vertices, draws) {
        Ok(()) => {
            state.patches += 1;
            state.built = Some((state.tiles.generation, choices));
        }
        Err(error) => {
            veldsdk::log::error!(target: "render", "патчи наложений не залиты: {:#}", error);
        }
    }
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
    // Патчи наложений — до сравнения кадра: пришедшие тайлы и смена уровня
    // видны как их пересборка, и она же двигает счётчик в [`Frame`].
    build_patches(state);

    let (Some(device), Some(target)) = (&state.device, &state.target) else { return };

    let now = Frame {
        camera: state.camera,
        texture: target.texture_id,
        generation: state.generation,
        patches: state.patches,
    };
    if state.drawn == Some(now) {
        return;
    }

    if let Err(error) = gpu::render(device, target, &state.camera, &state.batch) {
        veldsdk::log::error!(target: "render", "кадр не записан: {:#}", error);
        return;
    }
    state.drawn = Some(now);
}
