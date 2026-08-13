//! Обслуживание запроса: тайлы прямоугольника — из файлов кэша в текстуры.
//!
//! Открытие файла — событие fs, чтение открытого — синхронный ABI-вызов,
//! поэтому запрос живёт в два такта: on_query рассылает чтения, ответы
//! приходят в on_read_result, и последний из них закрывает запрос
//! терминальным QueryDone со списком промахов. Промах — не ошибка: за ним
//! заказчик идёт к производителю (image-tiler).

use veldsdk::graphics::{texture_usage, TextureFormat};
use veldsdk::proto::core::{ResourceHandle, ResourceOpened};
use veldsdk::proto::fs::FsReadRequest;

use crate::module::{layout, store, State};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileAddr, TileResult};

/// Потолок числа тайлов в одном запросе. Экран — это десятки тайлов;
/// тысячи означают ошибку в расчёте видимого прямоугольника у заказчика,
/// и честнее отказать, чем молча открыть тысячи файлов.
const MAX_QUERY_TILES: u64 = 1024;

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
    let (w, h) = (req.x1.saturating_sub(req.x0), req.y1.saturating_sub(req.y0));
    let count = u64::from(w) * u64::from(h);
    if count > MAX_QUERY_TILES {
        return fail(format!("{} тайлов за раз — больше потолка {}", count, MAX_QUERY_TILES));
    }
    if count == 0 {
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
        remaining: count as u32,
        misses: Vec::new(),
        label,
    });

    for y in req.y0..req.y1 {
        for x in req.x0..req.x1 {
            let read = state.pending_reads.begin(TileRead { query: correlation.clone(), x, y });
            crate::calls::fs::on_read(&FsReadRequest {
                path: layout::tile_path(&req.fingerprint, req.level, x, y),
            }, &read);
        }
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
            veldsdk::log::debug!(target: "handlers",
                "{}: тайл {}:{}:{} — промах: {}", query.label, query.level, read.x, read.y, miss);
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
    let texture = texture(width, height, &rgba, owner)?;
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

/// Текстура тайла — тот же обряд, что у производителя: sRGB-формат, заливка,
/// владение заказчику (см. image-tiler::tile_texture; свой экземпляр, потому
/// что модули делят код только через крейты, а пятнадцать строк обряда не
/// стоят собственного крейта).
fn texture(width: u32, height: u32, rgba: &[u8], owner: &str) -> Result<ResourceHandle, String> {
    let id = veldsdk::abi::resource_alloc_texture(
        width,
        height,
        TextureFormat::TexRgba8UnormSrgb as i32,
        texture_usage::TEXTURE_BINDING | texture_usage::COPY_DST,
    )
    .ok_or_else(|| format!("не выделилась текстура {}×{}", width, height))?;
    let texture = veldsdk::OwnedResource::from_raw_id(id);
    veldsdk::abi::resource_upload_image(texture.id(), rgba)
        .map_err(|e| format!("тайл не залит в текстуру: {}", e))?;
    veldsdk::resource::hand_off(
        ResourceHandle { id: texture.into_handle().id, size: rgba.len() as u64 },
        owner,
    )
}
