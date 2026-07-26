//! image-loader: файл с диска → GPU-текстура (RGBA8).
//!
//! У wasm нет доступа к диску, поэтому чтение идёт через платформенный
//! fs-сервис по шине (fs/on_read → fs/on_read_result, байты — в memory
//! ABI, регион принадлежит нам как запросчику). Декод — крейтом image,
//! формат определяется по содержимому. Готовая текстура передаётся
//! заказчику (arena_transfer): он грантит чтение (например, ui-service
//! для виджета image) и освобождает её — image-loader состояния уже
//! отданных текстур не хранит.

use veldsdk::proto::core::ResourceHandle;
use veldsdk::proto::fs::{FsReadRequest, FsReadResult};
use veldsdk::graphics::{texture_usage, TextureFormat};

use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};

/// Потолок пикселей на одну картинку: RGBA-буфер в пике живёт дважды
/// (декод + заливка) в памяти wasm-инстанса, чей лимит — 1 GiB.
const MAX_PIXELS: u64 = 64_000_000; // ~256 МБ RGBA

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
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { pending: veldsdk::Correlator::new() })
}

pub fn on_load(state: &mut State, req: LoadImageRequest) {
    // Без имени заказчика текстуру некому передать — отвечаем ошибкой сразу,
    // не тратя вызов fs.
    let owner = veldsdk::abi::event_publisher();
    if owner.is_empty() {
        crate::emit::on_load_result(&LoadImageResult {
            handle: None,
            width: 0,
            height: 0,
            error: "on_load пришёл от хоста: владение текстурой передать некому".to_string(),
            correlation_id: req.correlation_id,
        });
        return;
    }

    let correlation_id = state.pending.begin(PendingLoad {
        path: req.path.clone(),
        correlation_id: req.correlation_id,
        owner,
    });
    crate::calls::fs::on_read(&FsReadRequest { path: req.path, correlation_id });
}

/// Ответ fs на наше чтение. Broadcast — чужие ответы отбрасываем по
/// correlation_id, конвенция шины (см. veldsdk::Correlator).
pub fn on_read_result(state: &mut State, resp: FsReadResult) {
    let Some(pending) = state.pending.take(&resp.correlation_id) else { return };

    let (handle, width, height, error) = match make_texture(resp, &pending.path, &pending.owner) {
        Ok((id, w, h)) => (Some(ResourceHandle { id, ..Default::default() }), w, h, String::new()),
        Err(e) => {
            veldsdk::log::warn!(target: "handlers", "[image-loader] {}: {}", pending.path, e);
            (None, 0, 0, e)
        }
    };

    crate::emit::on_load_result(&LoadImageResult {
        handle,
        width,
        height,
        error,
        correlation_id: pending.correlation_id,
    });
}

/// Байты из fs-региона → декод → текстура, переданная `owner`'у.
/// Возвращает (region id текстуры, width, height).
fn make_texture(resp: FsReadResult, path: &str, owner: &str) -> Result<(u64, u32, u32), String> {
    if !resp.error.is_empty() {
        return Err(format!("fs: {}", resp.error));
    }
    let handle = resp.handle.ok_or_else(|| "fs: пустой handle в ответе".to_string())?;
    let bytes = veldsdk::abi::arena_read(handle.id, 0, handle.size)
        .ok_or_else(|| "fs: не удалось прочитать регион с файлом".to_string())?;
    veldsdk::abi::arena_free(handle.id);

    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("decode: {}", e))?;
    if u64::from(img.width()) * u64::from(img.height()) > MAX_PIXELS {
        return Err(format!("изображение {}x{} превышает лимит {} Мпикс",
            img.width(), img.height(), MAX_PIXELS / 1_000_000));
    }
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());

    // sRGB: сэмплер ui-service отдаст линейные значения, которые рендер в
    // sRGB-таргет переведёт обратно — картинка без искажения яркости.
    let texture_id = veldsdk::abi::arena_alloc_texture(
        w, h,
        TextureFormat::TexRgba8UnormSrgb as i32,
        texture_usage::TEXTURE_BINDING | texture_usage::COPY_DST,
    ).ok_or_else(|| "не удалось выделить текстуру".to_string())?;
    veldsdk::abi::arena_write(texture_id, 0, &rgba);

    // Владение — заказчику (паблишеру on_load): именно он знает, кому
    // грантить чтение текстуры и когда её освободить.
    if !veldsdk::abi::arena_transfer(texture_id, owner) {
        veldsdk::abi::arena_free(texture_id);
        return Err(format!("не удалось передать владение текстурой сервису '{}'", owner));
    }

    Ok((texture_id, w, h))
}
