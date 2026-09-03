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
//! Элементы сценарий называет не пикселями, а тем же, чем их называет
//! разметка: обработчиком нажимаемой коробки или частью видимой надписи. Где
//! это на экране, знает рендерер — он и отвечает (см. `app/on_locate_widget`).
//! Пиксельные шаги остались для того, у чего имени нет вовсе: шара, канвы
//! просмотра, границы между панелями.
//!
//! ```text
//! # <мс от старта окна> <действие> [аргументы]
//! 1500 move 640 300
//! 1600 click
//! 1700 press          # нажать и держать — дальше move тащит
//! 1800 release
//! 1900 shot browse
//! 2000 wait text:на диске     # дождаться надписи; часы сценария стоят
//! 2200 tap preview_product:eodata/Sentinel-1/S1A_IW.SAFE
//! 2400 expect text:открывается
//! 2600 exit
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
    /// Дождаться элемента и нажать в середину его видимой части.
    Tap { address: Address },
    /// Дождаться, пока элемент появится (или пропадёт). Часы сценария на это
    /// время стоят: ожидание не должно съедать время у следующих шагов.
    Await { address: Address, gone: bool },
    /// Элемент обязан быть (или не быть) прямо сейчас. Тем и отличается от
    /// ожидания: спрашивает один раз и не даёт второго шанса — часы сценария
    /// при этом всё равно стоят, пока идёт этот единственный вопрос.
    Assert { address: Address, gone: bool },
    /// Предел ожидания для последующих шагов.
    Patience { limit: Duration },
    /// Набрать текст туда, где сейчас каретка. Раскладка уже применена: шаг
    /// шлёт готовые знаки, а не коды клавиш.
    Type { text: String },
    /// Служебная клавиша целиком — нажать и отпустить.
    Key { code: u32, name: String },
    /// Ручательство сценария за провод: ни один удалённый ресурс не привёз
    /// больше этой доли своей длины. Окну проверять нечего — байты считает
    /// сеть и пишет в trace.log, — поэтому шаг только записывается в лог, а
    /// сверяет его прогон (`buildgen/run-uitests.py`).
    Delivered { share: u32 },
    /// Закрыть окно — конец прогона.
    Exit,
}

impl Action {
    /// Ждёт ли шаг ответа рендерера. Такой шаг останавливает часы сценария, и
    /// выдавать шаги дальше него нельзя: их время ещё не наступило.
    fn waits(&self) -> bool {
        matches!(self, Action::Tap { .. } | Action::Await { .. } | Action::Assert { .. })
    }
}

/// Чем сценарий называет элемент, когда не называет пикселями.
///
/// Ключ обработчика в адрес не входит: им разметка называет вкладку-адресата,
/// а не элемент, и от запуска к запуску он свой. Различает соседей нагрузка —
/// ключ снимка, имя записи, значение пункта меню.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Address {
    /// Имя метода обработчика; пусто — ищем по надписи.
    pub method: String,
    /// Нагрузка обработчика.
    pub value: String,
    /// Часть видимой надписи.
    pub text: String,
    /// Который по счёту из подошедших, считая с единицы, в порядке обхода
    /// разметки. 0 — «подойти должен ровно один».
    pub ordinal: u32,
}

impl std::fmt::Display for Address {
    /// Тем же видом, каким его написали в сценарии: строка едет в лог и в
    /// отказ, и читать её будет тот, кто сценарий писал.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.method.is_empty() {
            true => write!(f, "text:{}", self.text)?,
            false => {
                write!(f, "{}", self.method)?;
                if !self.value.is_empty() {
                    write!(f, ":{}", self.value)?;
                }
            }
        }
        match self.ordinal {
            0 => Ok(()),
            n => write!(f, "#{n}"),
        }
    }
}

/// Адрес: `метод[:нагрузка][#номер]` либо `text:<часть надписи>[#номер]`.
fn parse_address(rest: &str) -> Option<Address> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    // Номер отделяется, только когда за решёткой одни цифры: нагрузкой бывает
    // и путь, и ключ меню, и решётка в них не запрещена.
    let (body, ordinal) = match rest.rsplit_once('#') {
        // Считают с единицы: `#0` — это не «нулевой», а описка, и молча
        // принимать её за «должен быть один» значит скрывать её от писавшего.
        Some((body, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => {
            match tail.parse().ok()? {
                0 => return None,
                n => (body, n),
            }
        }
        _ => (rest, 0),
    };
    let sought = match body.split_once(':') {
        // Надпись — весь остаток вместе с двоеточиями: она и есть то, что
        // человек видит на экране.
        Some(("text", said)) => Address { text: said.trim().to_string(), ordinal, ..Address::default() },
        // Нагрузка тоже забирается целиком: в ключе снимка двоеточий хватает.
        Some((method, value)) => Address {
            method: method.to_string(),
            value: value.to_string(),
            ordinal,
            ..Address::default()
        },
        None => Address { method: body.to_string(), ordinal, ..Address::default() },
    };
    let named = !sought.method.is_empty() || !sought.text.is_empty();
    named.then_some(sought)
}

/// Разобранный сценарий: очередь шагов и отсчёт времени от первого кадра.
pub struct Script {
    /// Шаги по возрастанию времени; отыгранное снимается с головы.
    steps: VecDeque<(Duration, Action)>,
    /// Ноль отсчёта. Ставится по первому обращению, а не при разборе файла:
    /// между чтением конфига и первым кадром лежит вся инициализация GPU и
    /// загрузка плагинов, и время сценария уехало бы на неё целиком.
    started: Option<Instant>,
    /// С какой минуты сценарий чего-то ждёт; `None` — не ждёт.
    held: Option<Instant>,
    /// Сколько всего простояли в ожиданиях.
    paused: Duration,
}

impl Script {
    /// Сценарий из `VELDMAP_SCRIPT`. `Ok(None)` — переменной нет, то есть
    /// обычный запуск.
    ///
    /// Нечитаемый файл или непонятная строка — отказ, а не запуск без
    /// сценария: прогон затевали ради проверки, и молча отыграть вместо неё
    /// пустоту значит объявить непроверенное пройденным.
    pub fn from_env(logs: &Path) -> anyhow::Result<Option<Self>> {
        let Ok(path) = std::env::var("VELDMAP_SCRIPT") else { return Ok(None) };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("сценарий '{}' не читается: {}", path, e))?;

        // Каталог снимков заводится здесь: логов могло ещё не быть, а имена
        // файлов сценарий уже назвал.
        std::fs::create_dir_all(logs)
            .map_err(|e| anyhow::anyhow!("каталог снимков '{}' не создан: {}", logs.display(), e))?;

        let mut steps = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = strip_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            let step = parse_step(line, logs).ok_or_else(|| anyhow::anyhow!(
                "сценарий '{}', строка {}: не разобрана — '{}'", path, number + 1, line))?;
            steps.push(step);
        }

        steps.sort_by_key(|(at, _)| *at);
        log::info!(target: "render", "Сценарий '{}': {} шагов, снимки в {}", path, steps.len(), logs.display());
        Ok(Some(Self { steps: steps.into(), started: None, held: None, paused: Duration::ZERO }))
    }

    /// Остались ли неотыгранные шаги. Сценарий кончается шагом `exit`, и
    /// непустая очередь значит, что прогон оборвали снаружи.
    pub fn unfinished(&self) -> bool {
        !self.steps.is_empty()
    }

    /// Сценарий чего-то ждёт: часы стоят, пока не дождётся.
    ///
    /// Времена в файле значат «столько после предыдущего шага», и ожидание,
    /// съевшее их у следующих, отыграло бы остаток сценария разом.
    pub fn hold(&mut self) {
        self.held.get_or_insert_with(Instant::now);
    }

    /// Дождались — часы пошли дальше с того же места.
    pub fn resume(&mut self) {
        if let Some(since) = self.held.take() {
            self.paused += since.elapsed();
        }
    }

    /// Шаги, чей срок настал к этому кадру, в порядке сценария.
    pub fn due(&mut self) -> Vec<Action> {
        let started = *self.started.get_or_insert_with(Instant::now);
        if self.held.is_some() {
            return Vec::new();
        }
        let elapsed = started.elapsed().saturating_sub(self.paused);
        let mut ready = Vec::new();
        while self.steps.front().is_some_and(|(at, _)| *at <= elapsed) {
            let (_, action) = self.steps.pop_front().expect("front проверен условием");
            // Шаг, ждущий ответа, обрывает выдачу: часы после него встанут, и
            // созревшее вместе с ним — это созревшее «до», а не «вместо».
            let waits = action.waits();
            ready.push(action);
            if waits {
                break;
            }
        }
        ready
    }
}

/// Физический код служебной клавиши по её имени в сценарии.
///
/// Именами, а не числами: код — дискриминант `winit::keyboard::KeyCode`, и
/// написанное числом в сценарии рассохлось бы с ним молча. Печатные клавиши
/// сюда не входят вовсе — их набирают шагом `type`, где раскладка уже
/// применена.
fn named_key(name: &str) -> Option<u32> {
    use winit::keyboard::KeyCode;
    let code = match name {
        "enter" => KeyCode::Enter,
        "escape" => KeyCode::Escape,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "tab" => KeyCode::Tab,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "left" => KeyCode::ArrowLeft,
        "right" => KeyCode::ArrowRight,
        "up" => KeyCode::ArrowUp,
        "down" => KeyCode::ArrowDown,
        _ => return None,
    };
    Some(code as u32)
}

/// Комментарий — решётка, начинающая слово. Решётка внутри слова остаётся
/// адресу: ею он называет, который из подошедших нужен (`tab_select#2`).
fn strip_comment(line: &str) -> &str {
    let comment = line.char_indices().find(|(at, ch)| {
        *ch == '#' && (*at == 0 || line[..*at].ends_with(char::is_whitespace))
    });
    match comment {
        Some((at, _)) => &line[..at],
        None => line,
    }
}

/// `<мс> <действие> [аргументы]`.
///
/// Адрес элемента забирает остаток строки целиком: в надписи бывают пробелы, и
/// по словам её не разобрать.
fn parse_step(line: &str, logs: &Path) -> Option<(Duration, Action)> {
    let mut words = line.split_whitespace();
    let at = Duration::from_millis(words.next()?.parse().ok()?);
    let verb = words.next()?;
    let tail = line.split_once(verb).map(|(_, tail)| tail.trim()).unwrap_or_default();

    let action = match verb {
        "move" => Action::Move { x: words.next()?.parse().ok()?, y: words.next()?.parse().ok()? },
        "click" => Action::Click,
        "press" => Action::Button { pressed: true },
        "release" => Action::Button { pressed: false },
        "scroll" => Action::Scroll {
            dx: words.next()?.parse().ok()?,
            dy: words.next()?.parse().ok()?,
        },
        // Снимок ложится рядом с логами и только туда: имя — это имя, а не
        // путь, и разделители в нём увели бы файл из каталога прогона.
        "shot" => {
            let name = words.next()?;
            let plain = !name.contains(['/', '\\']) && name != "." && name != "..";
            Action::Shot { path: logs.join(format!("{}.png", plain.then_some(name)?)) }
        }
        "tap" => return Some((at, Action::Tap { address: parse_address(tail)? })),
        "wait" => return Some((at, Action::Await { address: parse_address(tail)?, gone: false })),
        "gone" => return Some((at, Action::Await { address: parse_address(tail)?, gone: true })),
        "expect" => return Some((at, Action::Assert { address: parse_address(tail)?, gone: false })),
        "absent" => return Some((at, Action::Assert { address: parse_address(tail)?, gone: true })),
        "timeout" => Action::Patience { limit: Duration::from_millis(words.next()?.parse().ok()?) },
        // Набираемое забирает остаток строки: пробел в нём такой же знак, как
        // и прочие, и разбирать текст по словам нельзя.
        "type" => return (!tail.is_empty()).then(|| (at, Action::Type { text: tail.to_string() })),
        "key" => return Some((at, Action::Key { code: named_key(tail)?, name: tail.to_string() })),
        "delivered" => Action::Delivered { share: words.next()?.parse().ok().filter(|share| *share <= 100)? },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_keeps_the_colons_of_its_payload() {
        let address = parse_address("preview_product:eodata/Sentinel-1/S1A:IW.SAFE")
            .expect("адрес разобран");
        assert_eq!(address.method, "preview_product");
        assert_eq!(address.value, "eodata/Sentinel-1/S1A:IW.SAFE");
        assert_eq!(address.ordinal, 0);
    }

    #[test]
    fn a_method_alone_is_an_address() {
        let address = parse_address("run_search").expect("адрес разобран");
        assert_eq!(address.method, "run_search");
        assert!(address.value.is_empty());
    }

    #[test]
    fn digits_after_the_hash_say_which_one() {
        let address = parse_address("tab_select#2").expect("адрес разобран");
        assert_eq!(address.method, "tab_select");
        assert_eq!(address.ordinal, 2);
    }

    #[test]
    fn a_hash_inside_the_payload_is_not_a_number() {
        let address = parse_address("download:S1A_IW#GRDH").expect("адрес разобран");
        assert_eq!(address.value, "S1A_IW#GRDH");
        assert_eq!(address.ordinal, 0);
    }

    #[test]
    fn a_label_is_addressed_by_a_part_of_it() {
        let address = parse_address("text:Папка пуста").expect("адрес разобран");
        assert!(address.method.is_empty());
        assert_eq!(address.text, "Папка пуста");
    }

    #[test]
    fn an_address_without_a_name_is_no_address() {
        assert!(parse_address("").is_none());
        assert!(parse_address(":полтора").is_none());
        assert!(parse_address("text:").is_none());
    }

    #[test]
    fn an_address_reads_back_the_way_it_was_written() {
        for written in ["run_search", "tab_select#2", "text:Папка пуста", "download:S1A#3"] {
            let address = parse_address(written).expect("адрес разобран");
            assert_eq!(address.to_string(), written);
        }
    }

    /// Комментарий отделяется пробелом, а номер в адресе — нет: иначе решётка
    /// съедала бы у сценария половину адреса вместе с номером.
    #[test]
    fn a_comment_does_not_eat_the_ordinal() {
        assert_eq!(strip_comment("2000 tap tab_select#2 # вторая").trim(), "2000 tap tab_select#2");
        assert_eq!(strip_comment("# только заметка").trim(), "");
        assert_eq!(strip_comment("2000 tap tab_select#2").trim(), "2000 tap tab_select#2");
    }

    #[test]
    fn a_label_with_spaces_survives_the_step() {
        let logs = Path::new("/tmp");
        let (at, action) = parse_step("2500 wait text:Под отбор ничего не подошло", logs)
            .expect("шаг разобран");
        assert_eq!(at, Duration::from_millis(2500));
        match action {
            Action::Await { address, gone } => {
                assert!(!gone);
                assert_eq!(address.text, "Под отбор ничего не подошло");
            }
            _ => panic!("ожидался шаг ожидания"),
        }
    }

    #[test]
    fn typed_text_keeps_its_spaces() {
        let (_, action) = parse_step("2000 type Sentinel 1", Path::new("/tmp")).expect("шаг разобран");
        match action {
            Action::Type { text } => assert_eq!(text, "Sentinel 1"),
            _ => panic!("ожидался набор текста"),
        }
    }

    #[test]
    fn a_key_is_named_not_numbered() {
        assert!(parse_step("2000 key enter", Path::new("/tmp")).is_some());
        assert!(parse_step("2000 key 13", Path::new("/tmp")).is_none());
        assert!(parse_step("2000 type", Path::new("/tmp")).is_none());
    }

    /// Ручательство за провод — доля в процентах, и только она: больше ста
    /// или без числа — не шаг, а опечатка.
    #[test]
    fn a_delivery_promise_is_a_share() {
        match parse_step("9700 delivered 75", Path::new("/tmp")).expect("шаг разобран") {
            (_, Action::Delivered { share }) => assert_eq!(share, 75),
            _ => panic!("ожидалось ручательство"),
        }
        assert!(parse_step("9700 delivered 101", Path::new("/tmp")).is_none());
        assert!(parse_step("9700 delivered", Path::new("/tmp")).is_none());
        assert!(parse_step("9700 delivered 50 60", Path::new("/tmp")).is_none());
    }

    /// Созревшее вместе с ждущим шагом — это созревшее ДО него, а не вместо:
    /// после него часы встанут, и следующим шагам ещё не время.
    #[test]
    fn a_waiting_step_cuts_the_batch() {
        let mut script = Script {
            steps: VecDeque::from(vec![
                (Duration::ZERO, Action::Click),
                (Duration::ZERO, Action::Await { address: Address::default(), gone: false }),
                (Duration::ZERO, Action::Click),
            ]),
            started: None,
            held: None,
            paused: Duration::ZERO,
        };
        assert_eq!(script.due().len(), 2, "выдача обрывается на ждущем шаге");
        assert_eq!(script.due().len(), 1, "остаток дожидается следующего раза");
    }

    /// Часы сценария стоят, пока он ждёт: иначе первое же долгое ожидание
    /// сделало бы все последующие времена просроченными, и остаток сценария
    /// отыгрался бы одним кадром.
    #[test]
    fn the_clock_stops_while_the_scenario_waits() {
        let mut script = Script {
            steps: VecDeque::from(vec![
                (Duration::ZERO, Action::Click),
                (Duration::from_millis(120), Action::Exit),
            ]),
            started: None,
            held: None,
            paused: Duration::ZERO,
        };

        assert_eq!(script.due().len(), 1, "первый шаг созрел сразу");

        script.hold();
        std::thread::sleep(Duration::from_millis(200));
        assert!(script.due().is_empty(), "пока ждём, шаги не зреют");

        script.resume();
        assert!(script.due().is_empty(), "простой не засчитан сценарию");

        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(script.due().len(), 1, "после простоя время идёт своим ходом");
    }
}
