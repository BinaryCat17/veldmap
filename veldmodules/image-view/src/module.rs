//! image-view: канвы просмотра растров поверх тайлового конвейера.
//!
//! Владелец разметки присылает место (on_canvas), ресурс (on_show, владение
//! переходит сюда) и намерения камеры; обратно уезжает только on_view_state.
//! Тайлы спрашиваются у tile-cache, промахи заказываются у image-tiler; его
//! read-грант на ресурс выдаётся здесь, при показе.
//!
//! Пока производство идёт, новые запросы канвы откладываются: у формата без
//! произвольного доступа каждый produce — полный проход по файлу, и пускать
//! второй параллельно первому значит читать гигабайты дважды. Проход кончился
//! — want_tiles() пересчитает нужное, и почти всё найдётся в кэше: проход
//! складывает туда все уровни. Устаревает produce только сменой уровня или
//! показа — тогда он убивается, это и есть отмена-приоритизация.
//!
//! module.rs — состояние и обработчики; камера — camera.rs, тайлы в
//! видеопамяти — tiles.rs, кадр — view.rs и gpu.rs.

pub mod camera;
pub mod gpu;
pub mod tiles;
pub mod view;

use std::collections::HashMap;

use veldmap_image_tiler_wrap::pyramid;
use veldsdk::proto::app as app_proto;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest,
    TileAddr, TileResult as ProducedTile,
};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, CloseView, ShowRequest, ViewState,
};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileResult as CachedTile};

use camera::Camera;
use gpu::Device;
use tiles::{Addr, TileStore};
use view::{Meta, Shown, Stamp, View};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Бюджет видеопамяти под тайлы, МиБ. Вытеснение — забывание: тайл
    /// остаётся на диске у tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    256
}

/// Чей это ответ кэша и о чём спрашивали. Отпечаток свой, а не из вида:
/// показ могли сменить, пока ответ шёл, и класть его тайлы под новый
/// отпечаток нельзя.
pub struct QueryCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

/// То же для производителя.
pub struct ProduceCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct State {
    /// Появляется на первом месте под канву: до него ни формата таргета, ни
    /// повода что-то собирать.
    device: Option<Device>,
    views: HashMap<String, View>,
    tiles: TileStore,
    pending_describe: veldsdk::Correlator<String>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        device: None,
        views: HashMap::new(),
        tiles: TileStore::new(config.vram_budget_mb * 1024 * 1024),
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
    })
}

// ── Входы владельца разметки ───────────────────────────────────

/// Место канвы. Пустая поверхность — отзыв: вид жив, рисовать негде.
pub fn on_canvas(state: &mut State, msg: Canvas) {
    if msg.view.is_empty() {
        veldsdk::log::warn!(target: "handlers", "канва без имени вида");
        return;
    }
    let Some(surface) = msg.surface else { return };

    let view = state.views.entry(msg.view.clone()).or_insert_with(|| View::new(msg.view.clone()));
    let Some(texture) = surface.surface else {
        veldsdk::log::info!(target: "handlers", "{}: место отозвано", view.label);
        view.target = None;
        return;
    };
    if surface.width == 0 || surface.height == 0 {
        veldsdk::log::warn!(target: "handlers", "{}: место {}x{} — рисовать негде",
            view.label, surface.width, surface.height);
        return;
    }

    if state.device.as_ref().is_none_or(|device| device.format != surface.format) {
        match Device::create(surface.format) {
            Ok(device) => {
                // Bind group'ы тайлов собраны под layout прежнего устройства —
                // с новым они несовместимы. Хранилище опустошается: тайлы
                // лежат на диске и вернутся за миллисекунды.
                state.tiles = TileStore::new_like(&state.tiles);
                state.device = Some(device);
            }
            Err(error) => {
                veldsdk::log::error!(target: "handlers", "ресурсы устройства: {:#}", error);
                if let Some(view) = state.views.get_mut(&msg.view) {
                    view.target = None;
                }
                return;
            }
        }
    }

    let view = state.views.get_mut(&msg.view).expect("вид только что был");
    match gpu::Target::create(texture.id, surface.width, surface.height) {
        Ok(target) => view.target = Some(target),
        Err(error) => {
            veldsdk::log::error!(target: "handlers", "{}: view таргета: {:#}", view.label, error);
            view.target = None;
            return;
        }
    }

    fit_if_first(state, &msg.view);
    want_tiles(state, &msg.view);
    report(state, &msg.view);
}

/// Показать снимок. Ресурс уже наш: владение передал отправитель.
pub fn on_show(state: &mut State, msg: ShowRequest) {
    let Some(resource) = msg.resource else {
        veldsdk::log::warn!(target: "handlers", "показ без ресурса");
        return;
    };
    if msg.view.is_empty() {
        veldsdk::log::warn!(target: "handlers", "показ без имени вида — ресурс освобождается");
        veldsdk::resource::release(resource);
        return;
    }

    let view = state.views.entry(msg.view.clone()).or_insert_with(|| View::new(msg.view.clone()));
    if !msg.label.is_empty() {
        view.label = msg.label.clone();
    }

    // Прежний показ кончается здесь: производство убивается, ожидания
    // обнуляются. Хвосты в полёте опознаются по отпечатку и выбрасываются.
    if let Some((correlation, _)) = view.produce.take() {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    view.inflight.clear();
    view.failed.clear();
    view.error = None;
    view.read_bytes = 0;
    view.total_bytes = resource.size;
    view.camera = None;
    view.drawn = None;
    view.shown = None;

    // Грант до владения: при отказе хелпер освобождает ресурс сам, и
    // заворачивать его во владельца было бы вторым освобождением.
    if let Err(error) = veldsdk::resource::grant_read_or_free(resource.id, "image-tiler") {
        view.error = Some(error);
        report(state, &msg.view);
        return;
    }
    view.shown = Some(Shown {
        resource: veldsdk::OwnedResource::new(resource.clone()),
        meta: None,
    });

    let correlation = view.describe.begin();
    state.pending_describe.insert(correlation.clone(), msg.view.clone());
    crate::calls::image_tiler::on_describe(&DescribeRequest {
        resource: Some(resource),
        label: view.label.clone(),
    }, &correlation);

    report(state, &msg.view);
}

/// Конец вида: всё его — прочь. Тайлы в хранилище общие и остаются до
/// вытеснения: та же вкладка, открытая заново, начнёт с них.
pub fn on_close(state: &mut State, msg: CloseView) {
    let Some(mut view) = state.views.remove(&msg.view) else { return };
    if let Some((correlation, _)) = view.produce.take() {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    veldsdk::log::debug!(target: "handlers", "{}: вид закрыт", view.label);
}

pub fn on_camera(state: &mut State, msg: CameraCommand) {
    let Some(view) = state.views.get_mut(&msg.view) else { return };
    let (Some(target), Some(meta)) = (&view.target, view.shown.as_ref().and_then(|s| s.meta.as_ref()))
    else {
        return;
    };
    let img = (meta.width, meta.height);
    let canvas = (target.width, target.height);

    match msg.command {
        Some(Command::Fit(_)) => view.camera = Some(Camera::fit(img.0, img.1, canvas.0, canvas.1)),
        Some(Command::ZoomAt(zoom)) => {
            if let Some(camera) = &mut view.camera {
                camera.zoom_at(f64::from(zoom.x), f64::from(zoom.y), f64::from(zoom.factor), img, canvas);
            }
        }
        Some(Command::Pan(pan)) => {
            if let Some(camera) = &mut view.camera {
                camera.pan(f64::from(pan.dx), f64::from(pan.dy), img);
            }
        }
        None => {}
    }

    want_tiles(state, &msg.view);
    report(state, &msg.view);
}

// ── Ответы конвейера тайлов ────────────────────────────────────

pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some(key) = state.pending_describe.take(&correlation) else { return };
    let Some(view) = state.views.get_mut(&key) else { return };
    if view.describe.settle(&correlation) != veldsdk::Reply::Current {
        // Показ успели сменить — и ресурс того описания уже освобождён.
        return;
    }

    if !msg.error.is_empty() {
        view.error = Some(msg.error);
        report(state, &key);
        return;
    }
    if msg.width == 0 || msg.height == 0 || msg.levels == 0 {
        view.error = Some("источник описан пустым".to_string());
        report(state, &key);
        return;
    }
    // Арифметика ячеек считается общим pyramid.rs — он и производитель обязаны
    // быть собраны под одну сторону тайла.
    if msg.tile != pyramid::TILE {
        view.error = Some(format!(
            "сторона тайла {} у производителя против {} у канвы", msg.tile, pyramid::TILE
        ));
        report(state, &key);
        return;
    }

    veldsdk::log::info!(target: "handlers",
        "{}: {}×{}, уровней {}, {}", view.label, msg.width, msg.height, msg.levels,
        if msg.random_access { "произвольный доступ" } else { "последовательный проход" });

    if let Some(shown) = &mut view.shown {
        shown.meta = Some(Meta {
            fingerprint: msg.fingerprint,
            width: msg.width,
            height: msg.height,
            levels: msg.levels,
        });
    }

    fit_if_first(state, &key);
    want_tiles(state, &key);
    report(state, &key);
}

/// Тайл из дискового кэша.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return discard_tile(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Тайл от производителя.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return discard_tile(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Кэш ответил всем, чем мог; промахи — производителю.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let Some(view) = state.views.get_mut(&ctx.view) else { return };

    // Показ сменили, пока ответ шёл: ожидания этого запроса уже не наши.
    if view.meta().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
        return;
    }

    if !msg.error.is_empty() {
        for addr in &ctx.cells {
            view.inflight.remove(addr);
        }
        view.error = Some(msg.error);
        report(state, &ctx.view);
        return;
    }

    // Промахи, которые всё ещё видимы, — на производство; уехавшие с экрана
    // выбрасываются из ожиданий и переспросятся, когда вернутся в кадр.
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);
    let missed: Vec<Addr> = msg.misses.iter().map(|addr| (level, addr.x, addr.y)).collect();
    let desired: std::collections::HashSet<Addr> = view::desired_cells(view)
        .map(|(_, cells)| cells.into_iter().collect())
        .unwrap_or_default();

    let mut produce_list: Vec<Addr> = Vec::new();
    for addr in missed {
        if !ctx.cells.contains(&addr) {
            continue;
        }
        if desired.contains(&addr) && !view.failed.contains(&addr) && view.produce.is_none() {
            produce_list.push(addr);
        } else {
            view.inflight.remove(&addr);
        }
    }
    if produce_list.is_empty() {
        report(state, &ctx.view);
        return;
    }

    let Some(shown) = &view.shown else { return };
    let correlation = state.pending_produce.begin(ProduceCtx {
        view: ctx.view.clone(),
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    view.produce = Some((correlation.clone(), level));
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(shown.resource.handle()),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label: view.label.clone(),
    }, &correlation);

    report(state, &ctx.view);
}

pub fn on_produce_progress(state: &mut State, msg: ProduceProgress) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else { return };
    let key = ctx.view.clone();
    if let Some(view) = state.views.get_mut(&key) {
        view.read_bytes = msg.read_bytes;
        view.total_bytes = msg.total_bytes;
        report(state, &key);
    }
}

/// Единственный конец производства, каким бы он ни был, — в том числе за
/// убитое отвечает хост пустым сообщением.
pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };
    let Some(view) = state.views.get_mut(&ctx.view) else { return };

    if view.produce.as_ref().is_some_and(|(active, _)| *active == correlation) {
        view.produce = None;
    }
    if view.meta().is_some_and(|meta| meta.fingerprint == ctx.fingerprint) {
        for addr in &ctx.cells {
            view.inflight.remove(addr);
        }
        if !msg.error.is_empty() {
            // Не переспрашивать то, что уже не произвелось: каждый сдвиг
            // камеры долбил бы производителя тем же отказом.
            view.failed.extend(ctx.cells.iter().copied());
            view.error = Some(msg.error);
            veldsdk::log::warn!(target: "handlers", "{}: производство: {}",
                view.label, view.error.as_deref().unwrap_or_default());
        }
    }

    // Пока проход шёл, запросы откладывались — пересчитать нужное сейчас.
    want_tiles(state, &ctx.view);
    report(state, &ctx.view);
}

// ── Кадровый тик ───────────────────────────────────────────────

pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    if !matches!(event.event, Some(app_proto::ui_event::Event::Frame(_))) {
        return;
    }
    let State { views, tiles, device: Some(device), .. } = state else { return };

    for view in views.values_mut() {
        let Some(camera) = view.camera else { continue };
        let Some(target) = &view.target else { continue };
        let stamp = Stamp {
            camera,
            generation: tiles.generation,
            texture: target.texture_id,
        };
        if view.drawn == Some(stamp) {
            continue;
        }

        let quads = view::quads(view, tiles);
        let target = view.target.as_ref().expect("место проверено выше");
        match gpu::render(device, target, &mut view.vertices, &quads) {
            Ok(()) => view.drawn = Some(stamp),
            Err(error) => {
                veldsdk::log::error!(target: "render", "{}: кадр не записан: {:#}", view.label, error);
            }
        }
    }
}

// ── Внутреннее ─────────────────────────────────────────────────

/// Первое вписывание: камера появляется, когда впервые известны и снимок, и
/// место. Дальше её живут команды.
fn fit_if_first(state: &mut State, key: &str) {
    let Some(view) = state.views.get_mut(key) else { return };
    if view.camera.is_some() {
        return;
    }
    let (Some(target), Some(meta)) = (&view.target, view.shown.as_ref().and_then(|s| s.meta.as_ref()))
    else {
        return;
    };
    view.camera = Some(Camera::fit(meta.width, meta.height, target.width, target.height));
}

/// Пересчёт нужного: видимые ячейки без имеющихся, ожидаемых и провальных —
/// в запрос кэшу. Пока идёт производство, новых запросов нет (см. заголовок);
/// производство другого уровня при этом убивается — оно уже не про то, на что
/// смотрят.
fn want_tiles(state: &mut State, key: &str) {
    let Some(view) = state.views.get(key) else { return };
    let Some((level, cells)) = view::desired_cells(view) else { return };

    let stale = view
        .produce
        .as_ref()
        .filter(|(_, produce_level)| *produce_level != level)
        .map(|(correlation, _)| correlation.clone());

    let needed: Vec<Addr> = {
        let meta = view.meta().expect("desired_cells требует меты");
        cells
            .into_iter()
            .filter(|addr| {
                !state.tiles.contains(&meta.fingerprint, *addr)
                    && !view.inflight.contains(addr)
                    && !view.failed.contains(addr)
            })
            .collect()
    };

    let view = state.views.get_mut(key).expect("вид только что был");
    if let Some(correlation) = stale {
        crate::cancel::image_tiler::on_produce(&correlation);
        view.produce = None;
    }
    if view.produce.is_some() || needed.is_empty() {
        return;
    }

    let Some(meta) = view.meta() else { return };
    let fingerprint = meta.fingerprint.clone();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for &(_, x, y) in &needed {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    }
    view.inflight.extend(needed.iter().copied());

    let label = view.label.clone();
    let correlation = state.pending_query.begin(QueryCtx {
        view: key.to_string(),
        fingerprint: fingerprint.clone(),
        cells: needed,
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level,
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
    view_key: &str,
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

    if let Some(view) = state.views.get_mut(view_key) {
        if view.meta().is_some_and(|meta| meta.fingerprint == fingerprint) {
            view.inflight.remove(&addr);
        }
    }
}

/// Рассылка состояния — на каждом смысловом изменении и только при изменении.
fn report(state: &mut State, key: &str) {
    let Some(view) = state.views.get_mut(key) else { return };
    let meta = view.shown.as_ref().and_then(|shown| shown.meta.as_ref());
    let current = ViewState {
        view: key.to_string(),
        source_width: meta.map_or(0, |meta| meta.width),
        source_height: meta.map_or(0, |meta| meta.height),
        scale: view.camera.map_or(0.0, |camera| camera.scale),
        read_bytes: view.read_bytes,
        total_bytes: view.total_bytes,
        busy: view.busy(),
        error: view.error.clone().unwrap_or_default(),
    };
    if view.reported.as_ref() != Some(&current) {
        crate::emit::on_view_state(&current);
        view.reported = Some(current);
    }
}

fn discard_tile(texture: Option<veldsdk::proto::core::ResourceHandle>) {
    if let Some(handle) = texture {
        veldsdk::resource::release(handle);
    }
}
