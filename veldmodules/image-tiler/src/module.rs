//! image-tiler: ресурс с растром → тайлы пирамиды (RGBA8 sRGB).
//!
//! Откуда байты — не его забота: заказчик открывает ресурс сам и даёт
//! read-грант, а читаются они окнами одинаково — файл, сеть или память.
//! Durable-состояние — тайловый кэш, им владеет tile-cache, и сюда приходят
//! только его промахи. Поэтому убийство посреди самого долгого прохода ничего
//! не разматывает: уже отданные тайлы у заказчика, уже отправленные — в кэше,
//! остальное честно пропало.
//!
//! Состояния, от которого зависел бы ответ, у модуля нет; разбор источника он
//! держит между запросами (см. [`Parsed`]) — но это memo, а не память: ответ
//! от него не меняется, меняется только цена.
//!
//! module.rs — фасад: State, init, обработчики и приёмник тайлов (Sink).
//! Арифметика пирамиды — pyramid.rs (общий с потребителями через wrap),
//! проход — cascade.rs, форматы — adapters/, идентичность — fingerprint.rs.

pub mod adapters;
pub mod cascade;
pub mod fingerprint;
pub mod pyramid;
pub mod resample;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use veldsdk::graphics::{self as gfx, TextureFormat};

use crate::proto::image_tiler::{
    Described, DescribeRequest, GeoTie, ProduceDone, ProduceProgress, ProduceRequest, TileResult,
};
use crate::proto::tile_cache::StoreTile;

/// Формат текстуры тайла. sRGB, потому что в тайле лежит готовое к показу
/// содержимое: сэмплер потребителя отдаст линейные значения, и фильтрация
/// дробных масштабов пройдёт в линейном пространстве.
///
/// Своя константа у каждого из двух поставщиков тайлов — общего крейта у них
/// нет и быть не может (`image-tiler` зависит от `tile-cache`, обратная
/// зависимость замкнула бы граф сборки), а факт этот объявлен там же, где и
/// сам тайл: в комментарии к `TileResult` обоих контрактов. Расходиться им
/// нельзя: тайлы из кэша и от производителя ложатся в один кадр, и разный
/// формат дал бы там разную яркость.
const TILE_FORMAT: TextureFormat = TextureFormat::TexRgba8UnormSrgb;

/// Шаг прогресса по прочитанному: чаще — шум, реже — «висит».
const PROGRESS_STEP: u64 = 8 * 1024 * 1024;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

/// Разобранный источник, оставленный до следующего запроса.
///
/// Запрос обслуживается целиком в обработчике, и состояния, от которого
/// зависел бы ответ, здесь по-прежнему нет: это memo, а не память. Убийство
/// посреди прохода его теряет вместе с инстансом, и следующий запрос просто
/// разберёт файл заново — ответ от этого не меняется, меняется только цена.
///
/// Цена и есть причина. `describe` и `produce` приходят парой на один и тот же
/// ресурс, а разбор — это распаковка всей плоскости: у гранулы OLCI это восемь
/// миллионов отсчётов, и без memo они распаковываются дважды на один показ.
/// Файл при этом читается один раз и без него (блоки держит носитель), а
/// платится именно распаковка и пробы величин.
///
/// Один слот: пара «описали — произвели» идёт подряд, а держать два разбора
/// сразу значит держать две плоскости, и лимит памяти инстанса этого не
/// переживёт. Чужой разбор отпускается ДО того, как начнётся новый.
struct Parsed {
    resource: u64,
    info: adapters::Info,
}

pub struct State {
    parsed: Option<Parsed>,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { parsed: None })
}

/// Разбор источника — из memo, если это тот же ресурс, иначе заново.
///
/// Отдаётся ссылкой, а не значением: за одним описанием идёт столько проходов,
/// сколько у источника ступеней (у тайлового TIFF это четыре-пять), и разбор,
/// расходуемый первым же из них, экономил бы ровно один.
fn parsed<'a>(
    state: &'a mut State,
    resource_id: u64,
    size: u64,
    bytes: &Rc<Cell<u64>>,
) -> Result<&'a adapters::Info, String> {
    if state.parsed.as_ref().is_none_or(|kept| kept.resource != resource_id) {
        // Чужой разбор отпускается здесь, до нового: две плоскости разом лимит
        // памяти инстанса не переживёт.
        state.parsed = None;
        veldsdk::log::debug!(target: "decode", "разбираем ресурс {} ({} байт)", resource_id, size);
        let info = adapters::describe(resource_id, size, bytes)?;
        state.parsed = Some(Parsed { resource: resource_id, info });
    } else {
        // Прочитанное засчитывается и на попадании: заказчик показывает долю
        // прочитанного, и ноль на готовом разборе читается как «ещё не
        // начали», хотя файл прочитан целиком.
        bytes.set(size);
        veldsdk::log::debug!(target: "decode", "разбор ресурса {} взят готовым", resource_id);
    }
    Ok(&state.parsed.as_ref().expect("разбор только что положен").info)
}

pub fn on_describe(state: &mut State, req: DescribeRequest) {
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let described = match describe(state, &req) {
        Ok(described) => described,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            Described { error, ..Default::default() }
        }
    };
    crate::emit::on_described(&described, &correlation);
}

fn describe(state: &mut State, req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;
    let fingerprint = fingerprint::fingerprint(resource.id, resource.size)?;
    // Привязка приезжает из соседнего файла и в memo не идёт: она свойство
    // пары «растр и его координаты», а memo знает только про растр.
    let mut ties = Vec::new();
    let info = parsed(state, resource.id, resource.size, &Rc::new(Cell::new(0)))?;

    // Координаты из соседнего файла — только когда в самом растре их нет: то,
    // что записано в нём, знает о своей раскладке точнее любого соседа.
    //
    // Неудача здесь описания не отменяет. Растр от этого не портится, он лишь
    // остаётся без привязки — и что с ним тогда делать, решает заказчик: у
    // него есть контур каталога, а у нас нет ничего.
    if info.ties.is_empty()
        && let Some(coordinates) = req.geolocation.as_ref()
    {
        match adapters::netcdf::geolocation(
            coordinates.id,
            coordinates.size,
            info.width,
            info.height,
        ) {
            Ok(found) => ties = found,
            Err(error) => veldsdk::log::warn!(target: "decode", "файл координат: {}", error),
        }
    }

    Ok(Described {
        fingerprint,
        width: info.width,
        height: info.height,
        tile: pyramid::TILE,
        levels: pyramid::level_count(info.width, info.height),
        reach: info.reach() as i32,
        finest: info.finest,
        ties: info
            .ties
            .iter()
            .chain(ties.iter())
            .map(|tie| GeoTie { px: tie.px, py: tie.py, lat: tie.lat, lon: tie.lon })
            .collect(),
        error: String::new(),
    })
}

pub fn on_produce(state: &mut State, req: ProduceRequest) {
    // Корреляция запроса — она же имя операции у платформы: ею заказчик её
    // и убьёт, если тайлы перестанут быть нужны.
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let error = match produce(state, &req, &correlation) {
        Ok(()) => String::new(),
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            error
        }
    };
    crate::emit::on_produce_done(&ProduceDone { error }, &correlation);
}

fn produce(state: &mut State, req: &ProduceRequest, correlation: &str) -> Result<(), String> {
    // Владелец будущих текстур — тот, кто прислал запрос: без имени
    // передавать их некому.
    let owner = veldsdk::resource::requester("image-tiler/on_produce")?;
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;

    let bytes = Rc::new(Cell::new(0u64));
    let fingerprint = fingerprint::fingerprint(resource.id, resource.size)?;
    let info = parsed(state, resource.id, resource.size, &bytes)?;

    let levels = pyramid::level_count(info.width, info.height);
    if req.level >= levels {
        return Err(format!("уровня {} нет: у растра их {}", req.level, levels));
    }
    let grid_w = pyramid::grid(pyramid::level_size(info.width, req.level));
    let grid_h = pyramid::grid(pyramid::level_size(info.height, req.level));
    let mut wants = BTreeSet::new();
    for tile in &req.tiles {
        if tile.x >= grid_w || tile.y >= grid_h {
            return Err(format!(
                "тайла {}:{} нет: сетка уровня {} — {}×{}",
                tile.x, tile.y, req.level, grid_w, grid_h
            ));
        }
        wants.insert((tile.x, tile.y));
    }
    let ordered: Vec<(u32, u32)> = wants.iter().copied().collect();
    let single_pass = matches!(info.reach(), crate::proto::image_tiler::Reach::Pyramid);

    let mut sink = Sink {
        correlation,
        owner,
        fingerprint,
        want_level: req.level,
        wants,
        want_total: ordered.len() as u32,
        done: 0,
        bytes: bytes.clone(),
        total_bytes: resource.size,
        reported: 0,
    };
    {
        let mut emit = |level: u32, tx: u32, ty: u32, w: u32, h: u32, rgba: &[u8]| {
            sink.emit(level, tx, ty, w, h, rgba)
        };
        adapters::produce(resource.id, resource.size, info, req.level, &ordered, &bytes, &mut emit)?;
    }

    // Разбор, из которого проход строит всю пирамиду разом, после него не
    // нужен: второго прохода по такому источнику не будет, пока не спросят
    // заново, — а держит он с собой отсчёты величины, то есть до полугигабайта
    // памяти инстанса (`netcdf::PLANE_BUDGET`). У прочих источников разбор —
    // это заголовки, он дёшев и остаётся: у них проход на каждую ступень.
    if single_pass {
        state.parsed = None;
    }

    // Проход кончился, а запрошенное не всё отдано — это ошибка адаптера,
    // и молчать о ней значит показать заказчику дыру без причины.
    if !sink.wants.is_empty() {
        return Err(format!("адаптер не произвёл {} из запрошенных тайлов", sink.wants.len()));
    }
    Ok(())
}

/// Приёмник произведённых тайлов: каждый уезжает в кэш, запрошенные — ещё и
/// заказчику текстурой. Прогресс едет отсюда же: у адаптеров нет ни
/// корреляции, ни знания, кто чего ждал.
struct Sink<'a> {
    correlation: &'a str,
    owner: String,
    fingerprint: String,
    want_level: u32,
    wants: BTreeSet<(u32, u32)>,
    want_total: u32,
    done: u32,
    bytes: Rc<Cell<u64>>,
    total_bytes: u64,
    reported: u64,
}

impl Sink<'_> {
    fn emit(&mut self, level: u32, tx: u32, ty: u32, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
        // Кромка прозрачного заливается цветом соседа здесь, а не в адаптерах:
        // сюда сходятся все тайлы всех рукавов, и ореол на границе `nodata`
        // одинаков у каждого (см. `adapters::bleed_alpha`). Копия делается
        // только там, где прозрачное вообще есть, — у обычного снимка её нет.
        let bled;
        let rgba = match rgba.chunks_exact(4).any(|pixel| pixel[3] == 0) {
            false => rgba,
            true => {
                let mut copy = rgba.to_vec();
                adapters::bleed_alpha(&mut copy, w, h);
                bled = copy;
                &bled
            }
        };

        // В кэш — всё произведённое: второй запрос по этому источнику должен
        // обслуживаться без прохода. Fire-and-forget, подтверждение не ждётся.
        let encoded = qoi::encode_to_vec(rgba, w, h).map_err(|e| format!("qoi: {}", e))?;
        crate::calls::tile_cache::on_store(&StoreTile {
            fingerprint: self.fingerprint.clone(),
            level,
            x: tx,
            y: ty,
            width: w,
            height: h,
            qoi: encoded,
        });

        if level == self.want_level && self.wants.remove(&(tx, ty)) {
            let texture = gfx::upload_texture("тайл", w, h, TILE_FORMAT, rgba, &self.owner)?;
            crate::emit::on_produced(
                &TileResult { level, x: tx, y: ty, texture: Some(texture), width: w, height: h },
                self.correlation,
            );
            self.done += 1;
            self.progress(true);
        } else {
            self.progress(false);
        }
        Ok(())
    }

    /// Прогресс: всегда на отданном тайле, между ними — по прочитанным
    /// мегабайтам источника (долгий проход виден и там, где запрошенных
    /// тайлов ещё не было).
    fn progress(&mut self, force: bool) {
        let read = self.bytes.get();
        if !force && read.saturating_sub(self.reported) < PROGRESS_STEP {
            return;
        }
        self.reported = read;
        crate::emit::on_produce_progress(
            &ProduceProgress {
                read_bytes: read,
                total_bytes: self.total_bytes,
                done_tiles: self.done,
                want_tiles: self.want_total,
            },
            self.correlation,
        );
    }
}
