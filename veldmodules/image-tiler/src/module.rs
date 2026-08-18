//! image-tiler: ресурс с растром → тайлы пирамиды (RGBA8 sRGB).
//!
//! Откуда байты — не его забота: заказчик открывает ресурс сам и даёт
//! read-грант, а читаются они окнами одинаково — файл, сеть или память.
//! Состояния между вызовами нет вовсе: durable-состояние — тайловый кэш, им
//! владеет tile-cache, и сюда приходят только его промахи. Поэтому убийство
//! посреди самого долгого прохода ничего не разматывает: уже отданные тайлы
//! у заказчика, уже отправленные — в кэше, остальное честно пропало.
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

/// Состояния между вызовами нет: запрос обслуживается целиком в обработчике —
/// на этом стоит и `cancellable: true` в схеме (см. заголовок модуля).
pub struct State;

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State)
}

pub fn on_describe(_state: &mut State, req: DescribeRequest) {
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let described = match describe(&req) {
        Ok(described) => described,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            Described { error, ..Default::default() }
        }
    };
    crate::emit::on_described(&described, &correlation);
}

fn describe(req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;
    let fingerprint = fingerprint::fingerprint(resource.id, resource.size)?;
    let info = adapters::describe(resource.id, resource.size, &Rc::new(Cell::new(0)))?;
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
            .map(|tie| GeoTie { px: tie.px, py: tie.py, lat: tie.lat, lon: tie.lon })
            .collect(),
        error: String::new(),
    })
}

pub fn on_produce(_state: &mut State, req: ProduceRequest) {
    // Корреляция запроса — она же имя операции у платформы: ею заказчик её
    // и убьёт, если тайлы перестанут быть нужны.
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let error = match produce(&req, &correlation) {
        Ok(()) => String::new(),
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            error
        }
    };
    crate::emit::on_produce_done(&ProduceDone { error }, &correlation);
}

fn produce(req: &ProduceRequest, correlation: &str) -> Result<(), String> {
    // Владелец будущих текстур — тот, кто прислал запрос: без имени
    // передавать их некому.
    let owner = veldsdk::resource::requester("image-tiler/on_produce")?;
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;

    let bytes = Rc::new(Cell::new(0u64));
    let fingerprint = fingerprint::fingerprint(resource.id, resource.size)?;
    let info = adapters::describe(resource.id, resource.size, &bytes)?;

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
        adapters::produce(resource.id, resource.size, &info, req.level, &ordered, &bytes, &mut emit)?;
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
