//! image-view: канвы просмотра растров поверх тайлового конвейера.
//!
//! Владелец разметки присылает место (on_canvas), ресурс (on_show, владение
//! переходит сюда) и намерения камеры; обратно уезжает только on_view_state.
//! Тайлы спрашиваются у tile-cache, промахи заказываются у image-tiler; его
//! read-грант на ресурс выдаётся здесь, при показе.
//!
//! Проход по источнику один: у формата без произвольного доступа каждый
//! produce — полный проход по файлу, и пускать второй параллельно первому
//! значит читать гигабайты дважды. Пока он идёт, промахи по этому источнику в
//! производство не уходят — ни свои, ни соседней вкладки с тем же файлом;
//! кончился — нужное пересчитывается всем, кто на него смотрит, и почти всё
//! находится в кэше: проход складывает туда все уровни. Убивается проход двумя
//! способами и обоими своим заказчиком: он уходит со снимка (`release_pass`)
//! либо ему нужен уровень грубее, чем производит его же проход
//! (`Passes::stale`).
//!
//! module.rs — состояние и обработчики; камера — camera.rs, кадр — view.rs и
//! gpu.rs. Тайлы в видеопамяти и учёт спрошенного — общие с глобусом
//! (`veldmap_image_tiler_wrap::tiles`).

pub mod camera;
pub mod gpu;
pub mod view;

use std::collections::{HashMap, HashSet};

use veldmap_image_tiler_wrap::tiles::{self, Addr, Missed, Passes, Store};
use veldsdk::proto::app as app_proto;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest,
    TileAddr, TileResult as ProducedTile,
};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, CloseView, ShowRequest, ViewState,
};
use crate::proto::tile_cache::{
    QueryDone, QueryRequest, TileAddr as QueryAddr, TileResult as CachedTile,
};

use camera::Camera;
use gpu::Device;
use view::{Shown, Stamp, View};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Бюджет видеопамяти под тайлы, МиБ. Вытеснение — забывание: тайл
    /// остаётся на диске у tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
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
    tiles: Store,
    pending_describe: veldsdk::Correlator<String>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    /// Идущие проходы производителя, по одному на источник. Не у вида: проход
    /// читает файл, а файл у двух вкладок с одним снимком один и тот же.
    /// Заказчик здесь — имя вкладки: снять проход с учёта вправе только та, что
    /// его завела.
    passes: Passes<String>,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        device: None,
        views: HashMap::new(),
        tiles: Store::new(config.vram_budget_mb * 1024 * 1024),
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
        passes: Passes::default(),
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
                state.tiles = Store::new_like(&state.tiles);
                state.device = Some(device);
            }
            Err(error) => {
                // Причина уезжает на экран, а не в один лишь лог: молчащая
                // пустая вкладка читается как «зависло», а не как отказ.
                complain(state, &msg.view, format!("не собрались ресурсы устройства: {:#}", error));
                return;
            }
        }
    }

    let view = state.views.get_mut(&msg.view).expect("вид только что был");
    match gpu::Target::create(texture.id, surface.width, surface.height) {
        Ok(target) => {
            view.target = Some(target);
            // Место собрано — прежняя жалоба на него больше не про этот кадр.
            view.trouble = None;
        }
        Err(error) => {
            complain(state, &msg.view, format!("не собралось место под кадр: {:#}", error));
            return;
        }
    }

    fit_if_first(state, &msg.view);
    want_tiles(state, &msg.view);
    report(state, &msg.view);
}

/// Место под кадр не собралось: причина уходит и в лог, и на экран — но
/// жалобой, а не приговором.
///
/// Разница не косметическая. Приговор (`ViewState.error`) заказчик читает как
/// «смотреть не на что» и **убирает канву из разметки**; а без канвы к нам не
/// придёт и следующее делегирование места — то есть вкладка остаётся мёртвой
/// до закрытия, даже когда отказ был мгновенным (текстуру успели сменить,
/// пока событие шло к нам). Жалоба оставляет канву на месте.
///
/// Само по себе это места ещё не возвращает: выделяет его владелец разметки, а
/// спрашивают его только сменой размера, которой тут не было. Поэтому о нужде
/// сказано отдельным полем — `ViewState.needs_place`, — и по нему владелец
/// выдаёт место заново (см. `report`).
///
/// Приговор остаётся для того, что делегированием не лечится: источник не
/// открылся или не описался — там и правда смотреть не на что.
fn complain(state: &mut State, key: &str, why: String) {
    veldsdk::log::error!(target: "handlers", "{}: {}", key, why);
    if let Some(view) = state.views.get_mut(key) {
        view.target = None;
        view.trouble = Some(why);
    }
    report(state, key);
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

    // Прежний показ кончается здесь, и его проход уходит вместе с ним: читает
    // тот ресурс, который сейчас освободится. Хвосты в полёте опознаются по
    // отпечатку и выбрасываются.
    let previous =
        state.views.get(&msg.view).and_then(|view| view.meta()).map(|meta| meta.fingerprint.clone());
    if let Some(fingerprint) = previous {
        release_pass(state, &fingerprint, &msg.view);
    }

    let view = state.views.entry(msg.view.clone()).or_insert_with(|| View::new(msg.view.clone()));
    if !msg.label.is_empty() {
        view.label = msg.label.clone();
    }

    view.fetch.reset();
    view.error = None;
    view.trouble = None;
    // И жалоба на застрявший кадр: она про прошлый снимок, а показывают уже
    // другой — вид в этой вкладке переиспользуется, а не заводится заново.
    view.stuck = None;
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
        // Канве привязка не нужна вовсе: она показывает растр как картинку, а
        // не кладёт его на Землю.
        geolocation: None,
    }, &correlation);

    report(state, &msg.view);
}

/// Конец вида: всё его — прочь. Тайлы в хранилище общие и остаются до
/// вытеснения: та же вкладка, открытая заново, начнёт с них.
pub fn on_close(state: &mut State, msg: CloseView) {
    let Some(mut view) = state.views.remove(&msg.view) else { return };
    view.fetch.reset();
    if let Some(fingerprint) = view.meta().map(|meta| meta.fingerprint.clone()) {
        release_pass(state, &fingerprint, &msg.view);
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

    // Годность описания решает общее правило: тайлер один, пирамида одна, и
    // разойтись с наложением в том, какой ответ считать пригодным, нечем.
    // Своё здесь одно — что делать с непригодным: у канвы есть место на
    // экране, и причина уезжает туда.
    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            view.error = Some(error);
            report(state, &key);
            return;
        }
    };

    veldsdk::log::info!(target: "handlers", "{}: {}", view.label, meta.note());

    if let Some(shown) = &mut view.shown {
        shown.meta = Some(meta);
    }

    fit_if_first(state, &key);
    want_tiles(state, &key);
    report(state, &key);
}

/// Тайл из дискового кэша.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::discard(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Тайл от производителя.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::discard(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// Кэш ответил всем, чем мог; промахи — производителю.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let cap = cap_tiles(state);
    let Some(view) = state.views.get_mut(&ctx.view) else { return };

    // Показ сменили, пока ответ шёл: ожидания этого запроса уже не наши.
    if view.meta().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
        return;
    }

    // Кэш отказал — но снимок от этого не портится: ожидания снимаются, и
    // ячейки спросятся заново следующим пересчётом. Показывать причину вместо
    // кадра значило бы стереть картинку из-за осечки, которая пройдёт сама.
    if !msg.error.is_empty() {
        view.fetch.forget_asked(&ctx.cells);
        view.trouble = Some(format!("неполно: {}", msg.error));
        report(state, &ctx.view);
        return;
    }

    // Промахи, которые всё ещё видимы, — на производство; уехавшие с экрана
    // выбрасываются из ожиданий и переспросятся, когда вернутся в кадр.
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);
    let desired: HashSet<Addr> = view::wanted(view, &state.tiles, cap)
        .map(|want| want.cells.into_iter().collect())
        .unwrap_or_default();
    let missed = view.fetch.missed(
        &state.passes,
        &ctx.fingerprint,
        &ctx.cells,
        msg.misses.iter().map(|addr| (level, addr.x, addr.y)),
        |addr| desired.contains(&addr),
    );
    // Ручку источника и подпись берём здесь же: дальше `state` занят учётом, и
    // держать на нём ссылку в вид одновременно нельзя.
    let (handle, label) = match view.shown.as_ref() {
        Some(shown) => (shown.resource.handle(), view.label.clone()),
        None => return,
    };

    let produce_list = match missed {
        Missed::Produce(cells) => cells,
        // Ждём чужой проход молча: его конец пересчитает нужное всем, кто на
        // этот источник смотрит.
        Missed::Waiting => {
            report(state, &ctx.view);
            return;
        }
        // Кэш закрыл заказ целиком — ступень пройдена, и спросить следующую
        // надо здесь (см. `Missed::Closed`).
        Missed::Closed => {
            want_tiles(state, &ctx.view);
            report(state, &ctx.view);
            return;
        }
    };

    let correlation = state.pending_produce.begin(ProduceCtx {
        view: ctx.view.clone(),
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    state.passes.begin(&ctx.fingerprint, ctx.view.clone(), correlation.clone(), level);
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(handle),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label,
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

    // Сторож снимается раньше всего и безусловно: он стоял на источнике, а не
    // на виде, и заказчика прохода могли закрыть, пока проход шёл. Уйди мы
    // отсюда, не сняв его, — соседние вкладки с тем же файлом ждали бы конца,
    // которого уже не будет.
    state.passes.finish(&correlation);

    if let Some(view) = state.views.get_mut(&ctx.view) {
        // Показ мог смениться, пока проход шёл: тогда его ячейки уже не про
        // этот вид, и ни ожидания, ни отказ к нему не относятся.
        let ours = view.meta().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
        let failed = ours && !msg.error.is_empty();
        view.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
        if ours {
            // Прочитанное — про кончившийся проход, и пережить его оно не
            // вправе: между ступенями не читается ничего, а полоса состояния
            // всё это время показывала бы «читается… 512 МБ из 512 МБ».
            (view.read_bytes, view.total_bytes) = (0, 0);
        }
        if failed {
            // Не переспрашивать то, что уже не произвелось: каждый сдвиг камеры
            // долбил бы производителя тем же отказом.
            //
            // Но и смотреть при этом есть на что: сорвавшийся проход — это
            // недоехавшие ячейки одной ступени, а не негодный снимок. Ступени
            // выше уже нарисованы, и заменить их причиной значило бы стереть
            // готовую картинку из-за одного оборванного чтения.
            veldsdk::log::warn!(target: "handlers", "{}: производство: {}", view.label, msg.error);
            view.trouble = Some(format!("неполно: {}", msg.error));
        }
    }

    // Пока проход шёл, запросы откладывались — пересчитать нужное сейчас, и не
    // только заказчику: соседняя вкладка с тем же файлом всё это время ждала
    // молча, и разбудить её больше некому.
    for key in watchers(state, &ctx.fingerprint) {
        want_tiles(state, &key);
        report(state, &key);
    }
}

/// Вкладки, смотрящие на этот источник.
fn watchers(state: &State, fingerprint: &str) -> Vec<String> {
    state
        .views
        .iter()
        .filter(|(_, view)| view.meta().is_some_and(|meta| meta.fingerprint == fingerprint))
        .map(|(key, _)| key.clone())
        .collect()
}

/// Вкладка уходит со снимка — или закрывается совсем: её проход уносится с ней.
///
/// Уносится безусловно, даже когда на тот же файл смотрит соседняя вкладка:
/// читает проход не «файл», а тот самый ресурс, который сейчас освободится
/// вместе с показом (см. `tiles::Passes`). Сосед заведёт свой по концу этого —
/// со своим ресурсом, который жив.
fn release_pass(state: &mut State, fingerprint: &str, view: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &view.to_string()) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

// ── Кадровый тик ───────────────────────────────────────────────

pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    if !matches!(event.event, Some(app_proto::ui_event::Event::Frame(_))) {
        return;
    }
    let cap = cap_tiles(state);
    // Виды, у которых кадр не записался: о такой жалобе надо ещё и сказать, а
    // рассылка состояния трогает `state` целиком — значит после обхода.
    let mut complained: Vec<String> = Vec::new();
    {
    let State { views, tiles, device: Some(device), .. } = state else { return };

    for (key, view) in views.iter_mut() {
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

        let quads = view::quads(view, tiles, cap);
        let target = view.target.as_ref().expect("место проверено выше");
        match gpu::render(device, target, &mut view.vertices, &quads) {
            // Снимается только СВОЯ жалоба — на застрявший кадр. Чужую
            // («производство сорвалось») успешный кадр не трогает: она держится
            // до приехавшего тайла (см. `View::landed`), а перерисовка той же
            // дыры дырой её и оставляет.
            Ok(()) => {
                view.drawn = Some(stamp);
                if view.stuck.take().is_some() {
                    complained.push(key.clone());
                }
            }
            Err(error) => {
                veldsdk::log::error!(target: "render", "{}: кадр не записан: {:#}", view.label, error);
                // Именно `trouble`, а не `error`: смотреть по-прежнему есть на
                // что, а вот кадр застыл. И отметиться нарисованным: без этого
                // попытка повторяется каждым кадровым тиком, то есть шестьдесят
                // раз в секунду пишет в журнал и жжёт кадр впустую, а причина у
                // неё та же самая.
                view.drawn = Some(stamp);
                view.stuck = Some(format!("кадр застыл: {}", error));
                complained.push(key.clone());
            }
        }
    }
    }
    // Жалоба, о которой не сказали, — это молчащая канва: кадрового тика
    // рассылка не делает, а других событий у застывшего вида не бывает.
    for key in complained {
        report(state, &key);
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

/// Пересчёт нужного: видимые ячейки без имеющихся, ожидаемых и провальных — в
/// запрос кэшу. Свой проход, который производит уровень подробнее нужного, тут
/// же и убивается: он уже не про то, на что смотрят.
fn want_tiles(state: &mut State, key: &str) {
    let cap = cap_tiles(state);
    let Some(view) = state.views.get(key) else { return };
    let Some(want) = view::wanted(view, &state.tiles, cap) else { return };
    let Some(fingerprint) = view.meta().map(|meta| meta.fingerprint.clone()) else { return };
    let label = view.label.clone();

    if let Some(correlation) = state.passes.stale(&fingerprint, &key.to_string(), want.level) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    let view = state.views.get_mut(key).expect("вид только что был");
    let Some(cells) = view.fetch.ask(&state.tiles, &fingerprint, want.cells) else { return };

    // Ход добычи видно только здесь: какая ступень понадобилась, сколько её
    // ячеек не хватает и далеко ли ещё до резкости (та же строка, что у
    // наложений на шаре).
    veldsdk::log::debug!(target: "handlers", "{}: ступень {} ({} из {}), ячеек {}",
        label, want.level, want.climbed + 1, want.steps, cells.len());

    let correlation = state.pending_query.begin(QueryCtx {
        view: key.to_string(),
        fingerprint: fingerprint.clone(),
        cells: cells.clone(),
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level: want.level,
        tiles: cells.iter().map(|&(_, x, y)| QueryAddr { x, y }).collect(),
        label,
    }, &correlation);
}

/// Потолок аппетита одной пирамиды. Правило общее с глобусом и живёт у бюджета
/// (`Store::cap_tiles`); здесь только ответ на «сколько пирамид сейчас
/// рисуется».
///
/// Пирамид, а не вкладок: две вкладки с одним снимком — это один набор тайлов и
/// одна доля бюджета.
fn cap_tiles(state: &State) -> u64 {
    state.tiles.cap_tiles(
        state
            .views
            .values()
            .filter(|view| view.target.is_some() && view.camera.is_some())
            .filter_map(|view| view.meta().map(|meta| meta.fingerprint.as_str())),
    )
}

/// Тайл приехал — в хранилище и вон из ожиданий.
///
/// С ожиданий ячейка снимается в любом случае: ответ про неё пришёл, а не
/// легший тайл — это промах, и переспросится он следующим пересчётом. Оставь
/// мы его в ожиданиях — не переспросился бы никогда.
fn accept_tile(
    state: &mut State,
    view_key: &str,
    fingerprint: &str,
    addr: Addr,
    texture: Option<veldsdk::proto::core::ResourceHandle>,
    width: u32,
    height: u32,
) {
    let landed = match &state.device {
        Some(device) => state.tiles.land(fingerprint, addr, texture, width, height, |view| {
            device.tile_bind_group(view)
        }),
        // Устройства нет — рисовать нечем и класть некуда; ячейка при этом ни в
        // чём не виновата и спросится заново, когда место под канву появится.
        None => {
            tiles::discard(texture);
            true
        }
    };

    if let Some(view) = state.views.get_mut(view_key)
        && view.meta().is_some_and(|meta| meta.fingerprint == fingerprint)
    {
        match landed {
            true => {
                view.fetch.arrived(addr);
                view.landed();
            }
            false => view.fetch.rejected(addr),
        }
    }
}

/// Рассылка состояния показа. Зовётся на всяком смысловом изменении, а уезжает
/// только изменившееся: топик объявлен снимком, и повтор отсекает его стаб
/// (см. schema.yaml).
fn report(state: &State, key: &str) {
    let Some(view) = state.views.get(key) else { return };
    let meta = view.shown.as_ref().and_then(|shown| shown.meta.as_ref());
    // Желаемое считается заново, а не берётся с прошлого раза: «работа идёт»
    // держится в том числе на непройденном пути к цели, а путь этот меряется
    // тем, что нужно сейчас. Считать дёшево — у канвы видимое это
    // прямоугольник камеры, без проекций.
    let want = view::wanted(view, &state.tiles, cap_tiles(state));
    let current = ViewState {
        view: key.to_string(),
        source_width: meta.map_or(0, |meta| meta.width),
        source_height: meta.map_or(0, |meta| meta.height),
        scale: view.camera.map_or(0.0, |camera| camera.scale),
        read_bytes: view.read_bytes,
        total_bytes: view.total_bytes,
        busy: view.busy(&state.passes, want.as_ref()),
        error: view.error.clone().unwrap_or_default(),
        // Жалоба одна на провод, а поводов два: застрявший кадр важнее
        // неполноты — неполный кадр хотя бы рисуется. Обе приезжают уже
        // сказанными словами: подписать их заново некому, заказчик о разнице
        // не знает (см. `View::stuck`).
        trouble: view.stuck.clone().or_else(|| view.trouble.clone()).unwrap_or_default(),
        // Место не собралось — значит выдать его заново может только владелец
        // разметки: сами мы его не выделяем (см. `complain`). Приговорённому
        // виду место не нужно: канвы для него в разметке всё равно нет, и
        // текстура под неё выделилась бы впустую.
        needs_place: view.target.is_none() && view.shown.is_some() && view.error.is_none(),
    };
    crate::emit::on_view_state(&current);
}
