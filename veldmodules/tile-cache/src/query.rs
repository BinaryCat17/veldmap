//! Обслуживание запроса: названные тайлы — из файлов кэша в текстуры.
//!
//! Открытие файла — событие fs, чтение открытого — синхронный ABI-вызов,
//! поэтому запрос живёт в два такта: on_query рассылает чтения, ответы
//! приходят в on_read_result, и последний из них закрывает запрос
//! терминальным QueryDone со списком промахов. Промах — не ошибка: за ним
//! заказчик идёт к производителю (image-tiler).

use veldsdk::graphics::TextureFormat;
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;

use crate::module::{layout, store, State};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileAddr, TileResult};

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

/// Потолок числа тайлов в одном запросе. Экран — это десятки тайлов;
/// тысячи означают ошибку в расчёте видимого у заказчика, и честнее отказать,
/// чем молча открыть тысячи файлов.
///
/// Заказчик своё желаемое режет по бюджету видеопамяти
/// (`tiles::Store::cap_tiles` — при умолчании в 256 МиБ это 128 ячеек), и пока
/// его доля бюджета меньше этого числа тайлов, потолок не срабатывает вовсе.
/// Больше 2 ГиБ на одну пирамиду — и он начнёт отказывать законным запросам, а
/// выхода из такого отказа у заказчика нет: он пришлёт тот же список.
const MAX_QUERY_TILES: usize = 1024;

/// Запрос в обслуживании.
pub struct Query {
    /// Кому передавать владение текстурами — паблишер запроса.
    pub owner: String,
    pub level: u32,
    /// Сколько чтений ещё в полёте; ноль закрывает запрос.
    pub remaining: u32,
    pub misses: Vec<(u32, u32)>,
    pub label: String,
}

/// Контекст одного чтения: чей это тайл и какой.
pub struct TileRead {
    pub query: String,
    pub x: u32,
    pub y: u32,
}

pub fn on_query(state: &mut State, req: QueryRequest) {
    let correlation = veldsdk::correlation();
    let fail = |error: String| {
        veldsdk::log::warn!(target: "handlers", "запрос кэша: {}", error);
        crate::emit::on_query_done(&QueryDone { misses: Vec::new(), error }, &correlation);
    };

    let owner = match veldsdk::resource::requester("tile-cache/on_query") {
        Ok(owner) => owner,
        Err(e) => return fail(e),
    };
    if !layout::valid_key(&req.fingerprint) {
        return fail(format!("негодный ключ кэша: '{}'", req.fingerprint));
    }
    if req.tiles.len() > MAX_QUERY_TILES {
        return fail(format!("{} тайлов за раз — больше потолка {}", req.tiles.len(), MAX_QUERY_TILES));
    }
    if req.tiles.is_empty() {
        crate::emit::on_query_done(&QueryDone { misses: Vec::new(), error: String::new() }, &correlation);
        return;
    }

    // Использование кэша — повод обновить свежесть источника: по mtime
    // маркера вытеснение отличает живое от заброшенного.
    store::touch(state, &req.fingerprint);

    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };
    state.queries.insert(correlation.clone(), Query {
        owner,
        level: req.level,
        remaining: req.tiles.len() as u32,
        misses: Vec::new(),
        label,
    });

    for tile in &req.tiles {
        let read = state
            .pending_reads
            .begin(TileRead { query: correlation.clone(), x: tile.x, y: tile.y });
        crate::calls::fs::on_read(&FsReadRequest {
            path: layout::tile_path(&req.fingerprint, req.level, tile.x, tile.y),
        }, &read);
    }
}

/// Ответ fs на чтение тайла. Нет файла — промах; битый файл — тоже промах:
/// производитель перезапишет его свежим, и кэш выправится сам.
pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let Some(read) = state.pending_reads.take(&veldsdk::correlation()) else {
        return veldsdk::resource::discard("fs/on_read_result", opened);
    };
    let Some(query) = state.queries.get_mut(&read.query) else {
        // Запрос не снимается до последнего ответа, так что сюда не попасть,
        // — но ресурс в ответе всё равно наш.
        if let Some(handle) = opened.handle {
            veldsdk::resource::release(handle);
        }
        return;
    };

    match serve(&query.owner, query.level, read.x, read.y, &opened) {
        Ok(tile) => crate::emit::on_tile(&tile, &read.query),
        Err(miss) => {
            // Промах и отказ выглядят одинаково для заказчика — он просто
            // пойдёт к производителю, — но в журнале их разводить надо.
            // «Файла нет» — обычный ход дела; всё прочее (не разобрался,
            // текстура не выделилась) производитель не лечит: он перепишет
            // годный файл, заказчик спросит снова, и отказ повторится — с той
            // разницей, что каждый круг стои́т полного прохода по источнику.
            let missing = miss.contains("не открыт") || miss.contains("не найден");
            match missing {
                true => veldsdk::log::debug!(target: "handlers",
                    "{}: тайл {}:{}:{} — промах: {}", query.label, query.level, read.x, read.y, miss),
                false => veldsdk::log::warn!(target: "handlers",
                    "{}: тайл {}:{}:{} есть, но не отдался: {}",
                    query.label, query.level, read.x, read.y, miss),
            }
            query.misses.push((read.x, read.y));
        }
    }

    query.remaining -= 1;
    if query.remaining == 0 {
        let query = state.queries.remove(&read.query).expect("запрос только что был");
        crate::emit::on_query_done(&QueryDone {
            misses: query.misses.into_iter().map(|(x, y)| TileAddr { x, y }).collect(),
            error: String::new(),
        }, &read.query);
    }
}

/// Файл открыт → байты → RGBA → текстура с передачей владения заказчику.
fn serve(owner: &str, level: u32, x: u32, y: u32, opened: &ResourceOpened) -> Result<TileResult, String> {
    let handle = veldsdk::resource::accept(opened)?;
    let file = veldsdk::OwnedResource::new(handle.clone());
    let bytes = veldsdk::abi::resource_read(file.id(), 0, handle.size).map_err(|e| e.to_string())?;
    drop(file);

    let (width, height, rgba) = decode(&bytes)?;
    let texture =
        veldsdk::graphics::upload_texture("тайл", width, height, TILE_FORMAT, &rgba, owner)?;
    Ok(TileResult { level, x, y, texture: Some(texture), width, height })
}

/// Кодированный тайл → RGBA8. Трёхканальные разворачиваются: кодек хранит
/// столько каналов, сколько было при записи, а текстуре нужны четыре.
fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let (header, pixels) = qoi::decode_to_vec(bytes).map_err(|e| format!("qoi: {}", e))?;
    let (width, height) = (header.width, header.height);
    if width == 0 || height == 0 || width > layout::MAX_TILE_SIDE || height > layout::MAX_TILE_SIDE {
        return Err(format!("qoi: неправдоподобные размеры {}×{}", width, height));
    }
    let pixel_count = (width as usize) * (height as usize);
    match pixels.len() / pixel_count.max(1) {
        4 => Ok((width, height, pixels)),
        3 => {
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for px in pixels.chunks_exact(3) {
                rgba.extend_from_slice(px);
                rgba.push(255);
            }
            Ok((width, height, rgba))
        }
        other => Err(format!("qoi: {} каналов", other)),
    }
}
