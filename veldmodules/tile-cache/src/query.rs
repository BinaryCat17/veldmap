//! Обслуживание запроса: названные тайлы — из файлов кэша в текстуры.
//!
//! Открытие файла — событие fs, чтение открытого — синхронный ABI-вызов,
//! поэтому запрос живёт в два такта: on_query рассылает чтения, ответы
//! приходят в on_read_result, и последний из них закрывает запрос
//! терминальным QueryDone со списком промахов. Промах — не ошибка: за ним
//! заказчик идёт к производителю (image-tiler).

use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;

use crate::module::{layout, store, tile, State};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileAddr, TileResult};


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
    if req.tiles.len() > tile::MAX_QUERY_TILES {
        return fail(format!("{} тайлов за раз — больше потолка {}", req.tiles.len(), tile::MAX_QUERY_TILES));
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
            // пойдёт к производителю, — но в журнале их разводить надо:
            // «файла нет» это обычный ход дела, а «файл есть, а тайла из него
            // не вышло» производитель не лечит. Он перепишет годный файл,
            // заказчик спросит снова, и отказ повторится — с той разницей,
            // что каждый круг стои́т полного прохода по источнику.
            //
            // Спрашивается это у самого хода дела, а не у текста ошибки:
            // открытие делает файловая система, и текст у неё свой на каждой
            // операционной системе (то же правило, что в `fs::delete`).
            match miss {
                Miss::NoFile(why) => veldsdk::log::debug!(target: "handlers",
                    "{}: тайл {}:{}:{} — промах: {}", query.label, query.level, read.x, read.y, why),
                Miss::Broken(why) => veldsdk::log::warn!(target: "handlers",
                    "{}: тайл {}:{}:{} есть, а тайла из него не вышло: {}",
                    query.label, query.level, read.x, read.y, why),
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

/// Почему тайл не отдался. Различаются здесь не тексты, а места: открытие —
/// не наше дело и обычно означает «такого тайла ещё нет», всё остальное делаем
/// мы сами, и там отказ — это отказ.
enum Miss {
    /// Файл не открылся: чаще всего его просто нет.
    NoFile(String),
    /// Файл есть, а тайла из него не вышло: не разобрался, не выделилась
    /// текстура, не залилась.
    Broken(String),
}

/// Файл открыт → байты → RGBA → текстура с передачей владения заказчику.
fn serve(owner: &str, level: u32, x: u32, y: u32, opened: &ResourceOpened) -> Result<TileResult, Miss> {
    let handle = veldsdk::resource::accept(opened).map_err(Miss::NoFile)?;
    // Потолок тот же, что у записи: тело тайла сжато, и сжатое крупнее
    // развёрнутого не бывает.
    let bytes = veldsdk::resource::read_whole(handle, layout::MAX_TILE_BYTES as u64)
        .map_err(Miss::Broken)?;

    let (width, height, rgba) = decode(&bytes).map_err(Miss::Broken)?;
    let texture = veldsdk::graphics::upload_texture(
        "тайл", width, height, tile::TILE_FORMAT, &rgba, owner,
    )
        .map_err(Miss::Broken)?;
    Ok(TileResult { level, x, y, texture: Some(texture), width, height })
}

/// Кодированный тайл → RGBA8. Трёхканальные разворачиваются: кодек хранит
/// столько каналов, сколько было при записи, а текстуре нужны четыре.
fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    // Размеры спрашиваются у заголовка, а не у декодированного. Декодер
    // выделяет по ним, и проверка после него отвергала бы уже занятое — а на
    // размерах, ради которых она написана, не исполнялась бы вовсе: свой
    // потолок у кодека — четыреста мегапикселей, то есть полтора гигабайта
    // RGBA, и столько инстансу не дают.
    let header = qoi::decode_header(bytes).map_err(|e| format!("qoi: {}", e))?;
    let (width, height) = (header.width, header.height);
    if width == 0 || height == 0 || width > layout::MAX_TILE_SIDE || height > layout::MAX_TILE_SIDE {
        return Err(format!("qoi: неправдоподобные размеры {}×{}", width, height));
    }
    let (_, pixels) = qoi::decode_to_vec(bytes).map_err(|e| format!("qoi: {}", e))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Заголовок QOI без единого байта тела: размеры объявлены, декодировать
    /// нечего.
    fn header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"qoif".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[4, 0]);
        bytes
    }

    /// Размеры проверяются ДО декодирования, а не после него.
    ///
    /// Порядок здесь и есть вся защита: декодер выделяет по объявленным
    /// размерам, и его собственный потолок — четыреста мегапикселей, то есть
    /// полтора гигабайта RGBA. Столько инстансу не дают, и проверка после
    /// декода не исполнилась бы вовсе.
    ///
    /// Видно это по тому, чем кончается отказ: сработай проверка позже, речь
    /// шла бы о нехватке байтов, а не о размерах.
    #[test]
    fn размеры_проверяются_до_декодирования() {
        let why = decode(&header(20_000, 20_000)).expect_err("тайл 20000×20000 обязан быть отвергнут");
        assert!(why.contains("неправдоподобные размеры"), "отказ не про размеры: {why}");
    }

    /// А правдоподобные размеры проверку проходят, и дело доходит до декодера —
    /// по отказу это и видно. Без этой половины первый тест доказывал бы лишь,
    /// что `decode` всегда отказывает.
    #[test]
    fn правдоподобные_размеры_до_потолка_не_доходят() {
        let why = decode(&header(512, 512)).expect_err("тела в заголовке нет, декод обязан сорваться");
        assert!(!why.contains("неправдоподобные"), "законный тайл отвергнут потолком: {why}");
    }

    /// Пустой и битый заголовок — отказ, а не паника: файл в каталоге кэша
    /// переживает и оборванную запись.
    #[test]
    fn битый_заголовок_не_роняет() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"not a qoi file at all").is_err());
        assert!(decode(&header(0, 512)).is_err(), "нулевая сторона");
    }
}
