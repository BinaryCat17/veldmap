//! capture.rs — воспроизводимый прогон окна: синтетический ввод и снимки кадра.
//!
//! Интерфейс юнит-тестами не проверяется — «как это выглядит» видно только
//! запуском. Сценарий делает эту проверку повторяемой: те же движения
//! курсора, те же нажатия и снимок в тот же момент — снимок ложится рядом с
//! логами, то есть тоже относится к последнему запуску.
//!
//! Включается переменной `VELDMAP_SCRIPT` с путём к файлу сценария; без неё
//! ничего отсюда не работает и обычный запуск об этом модуле не знает.
//!
//! ```text
//! # <мс от старта окна> <действие> [аргументы]
//! 1500 move 640 300
//! 1600 click
//! 1700 press          # нажать и держать — дальше move тащит
//! 1800 release
//! 1900 shot browse
//! 2000 exit
//! ```

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::compositor::Compositor;

/// Шаг сценария. Своего времени не несёт — его держит очередь.
pub enum Action {
    /// Курсор в физических пикселях окна: точка, от которой отсчитываются
    /// и наведение, и следующий клик.
    Move { x: f32, y: f32 },
    /// Нажать и отпустить левую кнопку там, где стоит курсор.
    Click,
    /// Только нажать или только отпустить. Порознь они нужны затем, для чего
    /// щелчка мало: перетаскивание — это нажатие, движения и отпускание, и
    /// проверяется оно только так.
    Button { pressed: bool },
    /// Колесо, в тех же единицах, в которых его шлёт окно. Своим шагом, а не
    /// через `Move`: приближение и прокрутка списка иначе не проверяются.
    Scroll { dx: f32, dy: f32 },
    /// Снимок кадра. Путь собран при разборе сценария: каталог знает он, а не
    /// кадровый цикл.
    Shot { path: PathBuf },
    /// Закрыть окно — конец прогона.
    Exit,
}

/// Разобранный сценарий: очередь шагов и отсчёт времени от первого кадра.
pub struct Script {
    /// Шаги по возрастанию времени; отыгранное снимается с головы.
    steps: VecDeque<(Duration, Action)>,
    /// Ноль отсчёта. Ставится по первому обращению, а не при разборе файла:
    /// между чтением конфига и первым кадром лежит вся инициализация GPU и
    /// загрузка плагинов, и время сценария уехало бы на неё целиком.
    started: Option<Instant>,
}

impl Script {
    /// Сценарий из `VELDMAP_SCRIPT`. `None` — переменной нет, то есть обычный
    /// запуск. Нечитаемый файл или непонятная строка — предупреждение в лог и
    /// тоже `None`: прогон без сценария лучше, чем прогон по половине его.
    pub fn from_env(logs: &Path) -> Option<Self> {
        let path = std::env::var("VELDMAP_SCRIPT").ok()?;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                log::error!(target: "render", "Сценарий '{}' не читается: {}", path, e);
                return None;
            }
        };

        // Каталог снимков заводится здесь: логов могло ещё не быть, а имена
        // файлов сценарий уже назвал.
        if let Err(e) = std::fs::create_dir_all(logs) {
            log::error!(target: "render", "Каталог снимков '{}' не создан: {}", logs.display(), e);
            return None;
        }

        let mut steps = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match parse_step(line, logs) {
                Some(step) => steps.push(step),
                None => {
                    log::error!(target: "render", "Сценарий '{}', строка {}: не разобрана — '{}'", path, number + 1, line);
                    return None;
                }
            }
        }

        steps.sort_by_key(|(at, _)| *at);
        log::info!(target: "render", "Сценарий '{}': {} шагов, снимки в {}", path, steps.len(), logs.display());
        Some(Self { steps: steps.into(), started: None })
    }

    /// Шаги, чей срок настал к этому кадру, в порядке сценария.
    pub fn due(&mut self) -> Vec<Action> {
        let elapsed = self.started.get_or_insert_with(Instant::now).elapsed();
        let mut ready = Vec::new();
        while self.steps.front().is_some_and(|(at, _)| *at <= elapsed) {
            let (_, action) = self.steps.pop_front().expect("front проверен условием");
            ready.push(action);
        }
        ready
    }
}

/// `<мс> <действие> [аргументы]`.
fn parse_step(line: &str, logs: &Path) -> Option<(Duration, Action)> {
    let mut words = line.split_whitespace();
    let at = Duration::from_millis(words.next()?.parse().ok()?);
    let action = match words.next()? {
        "move" => Action::Move { x: words.next()?.parse().ok()?, y: words.next()?.parse().ok()? },
        "click" => Action::Click,
        "press" => Action::Button { pressed: true },
        "release" => Action::Button { pressed: false },
        "scroll" => Action::Scroll {
            dx: words.next()?.parse().ok()?,
            dy: words.next()?.parse().ok()?,
        },
        "shot" => Action::Shot { path: logs.join(format!("{}.png", words.next()?)) },
        "exit" => Action::Exit,
        _ => return None,
    };
    // Лишние слова — почти наверняка опечатка в аргументах, а не мусор.
    words.next().is_none().then_some((at, action))
}

/// Всё, чем кадр пересобирается в свою текстуру: тот же блит, что и в окно.
pub struct FrameSource<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub compositor: &'a Compositor,
    /// Bind group приаттаченной поверхности; `None` — окно рисует один фон,
    /// и снимок честно покажет именно его.
    pub surface: Option<&'a wgpu::BindGroup>,
    pub size: (u32, u32),
    /// Формат свопчейна: пайплайн блита собран под него, и цель снимка обязана
    /// быть такой же.
    pub format: wgpu::TextureFormat,
}

/// Снимок кадра в PNG.
///
/// Кадр рисуется заново в свою текстуру, а не читается из свопчейна: его
/// текстуры создаёт драйвер, и `COPY_SRC` у них нет. Заодно снимок не зависит
/// от того, показан ли уже кадр на экране.
pub fn shoot(frame: FrameSource<'_>, path: &Path) -> anyhow::Result<()> {
    let (width, height) = frame.size;
    if width == 0 || height == 0 {
        anyhow::bail!("окно нулевого размера");
    }

    let target = frame.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Capture Target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: frame.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Выравнивание строк — требование copy_texture_to_buffer, а не картинки:
    // лишние байты в конце каждой строки срезаются уже при чтении.
    let row_bytes = width * 4;
    let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = frame.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Capture Readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = frame.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Capture") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Capture Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        if let Some(bind_group) = frame.surface {
            frame.compositor.blit_ui(&mut pass, bind_group);
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    frame.queue.submit(Some(encoder.finish()));

    // Ожидание синхронное: снимок делается изредка и вне кадрового бюджета,
    // а асинхронный путь потребовал бы держать буфер между кадрами.
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    frame.device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;

    let mapped = readback.get_mapped_range(..)?;
    let mut rgba = Vec::with_capacity((row_bytes * height) as usize);
    for row in mapped.chunks_exact(padded_row as usize) {
        rgba.extend_from_slice(&row[..row_bytes as usize]);
    }
    drop(mapped);
    readback.unmap();

    if is_bgra(frame.format) {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    } else if !is_rgba(frame.format) {
        anyhow::bail!("формат поверхности {:?} не восьмибитный RGBA/BGRA", frame.format);
    }

    write_png(path, width, height, &rgba)
}

fn is_bgra(format: wgpu::TextureFormat) -> bool {
    matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb)
}

fn is_rgba(format: wgpu::TextureFormat) -> bool {
    matches!(format, wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb)
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}
