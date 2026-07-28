//! image-loader: ресурс с изображением → GPU-текстура превью (RGBA8).
//!
//! Откуда байты — не его забота: заказчик открывает ресурс сам (файл через
//! fs, удалённый файл через network) и даёт read-грант, а читаются они
//! одинаково — окнами через `ResourceReader`. Поэтому здесь нет ни
//! зависимостей на шине, ни состояния между вызовами: пришёл ресурс —
//! ответили текстурой.
//!
//! Декод потоковый, с даунсемплом сразу в запрошенный бокс: память не зависит
//! от размера исходника, а у удалённого ресурса по проводу идут только те
//! фрагменты, которые декодер действительно прочитал.
//!
//! module.rs — фасад: State, init и обработчик топика. Декодирование —
//! в decode.rs, усреднение в превью — в downsample.rs.

pub mod decode;
pub mod downsample;

use veldsdk::proto::core::ResourceHandle;
use veldsdk::graphics::{texture_usage, TextureFormat};

use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};

/// Потолок стороны превью, если заказчик не назвал свой бокс. Текстуру больше
/// GPU может и не принять, а для просмотра этого с запасом достаточно.
const MAX_PREVIEW_SIDE: u32 = 4096;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

/// Состояния между вызовами нет: запрос обслуживается целиком в обработчике.
pub struct State;

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State)
}

pub fn on_load(_state: &mut State, req: LoadImageRequest) {
    // Корреляция запроса — она же идентификатор задачи декодирования, которую
    // завёл заказчик: он её владелец, он же её и отменяет.
    let correlation_id = veldsdk::correlation();
    // Чем назвать источник в логах и в списке задач, знает только заказчик:
    // сюда приходит ресурс, у которого ни имени, ни пути уже нет.
    let label = if req.label.is_empty() { correlation_id.clone() } else { req.label.clone() };

    // Декодирование занимает наш обработчик целиком, а с ним и очередь: узнать
    // об отмене событием невозможно, поэтому декодер опрашивает её между
    // порциями. Саму задачу завёл заказчик — он её владелец и он же её
    // отменяет; нам остаётся только смотреть, живая ли она.
    let cancelled = veldsdk::Cancellation::watch(&correlation_id);

    let (handle, width, height, source, error) = match make_texture(req, &|| cancelled.cancelled()) {
        Ok(t) => (Some(ResourceHandle { id: t.id, size: t.size }), t.width, t.height, t.source, String::new()),
        Err(e) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, e);
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
    }, &correlation_id);
}

struct Texture {
    id: u64,
    size: u64,
    width: u32,
    height: u32,
    source: (u32, u32),
}

fn make_texture(req: LoadImageRequest, cancelled: decode::Cancelled) -> Result<Texture, String> {
    // Владелец ресурса и будущий владелец текстуры — тот, кто прислал запрос.
    // Без имени передать текстуру некому.
    let owner = veldsdk::resource::requester("image-loader/on_load")?;
    let resource = req.resource.ok_or_else(|| "в запросе нет ресурса".to_string())?;

    let preview = decode::preview(
        resource.id, resource.size,
        preview_side(req.max_width), preview_side(req.max_height),
        cancelled,
    )?;

    // sRGB: сэмплер ui-service отдаст линейные значения, которые рендер в
    // sRGB-таргет переведёт обратно — картинка без искажения яркости.
    let texture_id = veldsdk::abi::arena_alloc_texture(
        preview.width, preview.height,
        TextureFormat::TexRgba8UnormSrgb as i32,
        texture_usage::TEXTURE_BINDING | texture_usage::COPY_DST,
    ).ok_or_else(|| format!("не удалось выделить текстуру {}×{}", preview.width, preview.height))?;
    veldsdk::abi::arena_write(texture_id, 0, &preview.rgba);

    let texture = veldsdk::resource::hand_off(
        ResourceHandle { id: texture_id, size: preview.rgba.len() as u64 },
        &owner,
    )?;

    Ok(Texture {
        id: texture.id,
        size: texture.size,
        width: preview.width,
        height: preview.height,
        source: preview.source,
    })
}

fn preview_side(requested: u32) -> u32 {
    if requested == 0 { MAX_PREVIEW_SIDE } else { requested.min(MAX_PREVIEW_SIDE) }
}
