//! image-loader: файл → GPU-текстура превью (RGBA8).
//!
//! У wasm нет доступа к диску, поэтому файл приходит ресурсом от платформенного
//! fs (fs/on_read → fs/on_read_result). Ресурс не поднимается в память целиком:
//! декодер тянет из него окна через `ResourceReader`, а пиксели сразу уходят в
//! даунсемпл — так открываются и гигабайтные снимки. Готовая текстура
//! передаётся заказчику (arena_transfer): он грантит чтение (например,
//! ui-service для виджета image) и освобождает её, а image-loader состояния
//! уже отданных текстур не хранит.
//!
//! module.rs — фасад: State, init и обработчики топиков. Декодирование —
//! в decode.rs, усреднение в превью — в downsample.rs.

pub mod decode;
pub mod downsample;

use veldsdk::proto::core::ResourceHandle;
use veldsdk::proto::fs::{FsReadRequest, FsReadResult};
use veldsdk::graphics::{texture_usage, TextureFormat};
use veldsdk::OwnedResource;

use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};

/// Потолок стороны превью, если заказчик не назвал свой бокс. Текстуру больше
/// GPU может и не принять, а для просмотра этого с запасом достаточно.
const MAX_PREVIEW_SIDE: u32 = 4096;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    /// Запрошенные у fs файлы: correlation_id → контекст внешнего запроса.
    pending: veldsdk::Correlator<PendingLoad>,
}

pub struct PendingLoad {
    path: String,
    correlation_id: String,
    /// Паблишер on_load — будущий владелец текстуры. Читается именно здесь:
    /// в on_read_result event_publisher() уже «хост» (ответ fs публикует
    /// нативный сервис, publisher = 0).
    owner: String,
    box_w: u32,
    box_h: u32,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { pending: veldsdk::Correlator::new() })
}

pub fn on_load(state: &mut State, req: LoadImageRequest) {
    // Без имени заказчика текстуру некому передать — отвечаем ошибкой сразу,
    // не тратя вызов fs.
    let owner = veldsdk::abi::event_publisher();
    if owner.is_empty() {
        emit_error(req.correlation_id, "on_load пришёл от хоста: владение текстурой передать некому".to_string());
        return;
    }

    let correlation_id = state.pending.begin(PendingLoad {
        path: req.path.clone(),
        correlation_id: req.correlation_id,
        owner,
        box_w: preview_side(req.max_width),
        box_h: preview_side(req.max_height),
    });
    crate::calls::fs::on_read(&FsReadRequest { path: req.path, correlation_id });
}

/// Ответ fs на наше чтение. Broadcast — чужие ответы отбрасываем по
/// correlation_id, конвенция шины (см. veldsdk::Correlator).
pub fn on_read_result(state: &mut State, resp: FsReadResult) {
    let Some(pending) = state.pending.take(&resp.correlation_id) else { return };

    let (handle, width, height, source, error) = match make_texture(resp, &pending) {
        Ok(t) => (Some(ResourceHandle { id: t.id, size: t.size }), t.width, t.height, t.source, String::new()),
        Err(e) => {
            veldsdk::log::warn!(target: "handlers", "[image-loader] {}: {}", pending.path, e);
            (None, 0, 0, (0, 0), e)
        }
    };

    crate::emit::on_load_result(&LoadImageResult {
        handle,
        width,
        height,
        source_width: source.0,
        source_height: source.1,
        error,
        correlation_id: pending.correlation_id,
    });
}

struct Texture {
    id: u64,
    size: u64,
    width: u32,
    height: u32,
    source: (u32, u32),
}

/// Ресурс файла → превью → текстура, переданная владельцу запроса.
fn make_texture(resp: FsReadResult, pending: &PendingLoad) -> Result<Texture, String> {
    if !resp.error.is_empty() {
        return Err(format!("fs: {}", resp.error));
    }
    let handle = resp.handle.ok_or_else(|| "fs: пустой handle в ответе".to_string())?;
    let size = handle.size;
    // Ресурс файла наш и нужен только на время декодирования: освободится на
    // выходе из функции любым путём, включая ошибку.
    let file = OwnedResource::new(handle);

    let preview = decode::preview(file.id(), size, pending.box_w, pending.box_h)?;

    // sRGB: сэмплер ui-service отдаст линейные значения, которые рендер в
    // sRGB-таргет переведёт обратно — картинка без искажения яркости.
    let texture_id = veldsdk::abi::arena_alloc_texture(
        preview.width, preview.height,
        TextureFormat::TexRgba8UnormSrgb as i32,
        texture_usage::TEXTURE_BINDING | texture_usage::COPY_DST,
    ).ok_or_else(|| format!("не удалось выделить текстуру {}×{}", preview.width, preview.height))?;
    veldsdk::abi::arena_write(texture_id, 0, &preview.rgba);

    // Владение — заказчику (паблишеру on_load): именно он знает, кому
    // грантить чтение текстуры и когда её освободить.
    if !veldsdk::abi::arena_transfer(texture_id, &pending.owner) {
        veldsdk::abi::arena_free(texture_id);
        return Err(format!("не удалось передать владение текстурой сервису '{}'", pending.owner));
    }

    Ok(Texture {
        id: texture_id,
        size: preview.rgba.len() as u64,
        width: preview.width,
        height: preview.height,
        source: preview.source,
    })
}

fn preview_side(requested: u32) -> u32 {
    if requested == 0 { MAX_PREVIEW_SIDE } else { requested.min(MAX_PREVIEW_SIDE) }
}

fn emit_error(correlation_id: String, error: String) {
    crate::emit::on_load_result(&LoadImageResult {
        handle: None,
        width: 0,
        height: 0,
        source_width: 0,
        source_height: 0,
        error,
        correlation_id,
    });
}
