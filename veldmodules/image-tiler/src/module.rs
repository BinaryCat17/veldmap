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
use std::time::{Duration, Instant};

use veldsdk::graphics as gfx;
// Формат тайла — не наш: он общий с кэшем и объявлен там, где его видят оба
// (см. `tile.rs` в tile-cache). Своя копия здесь однажды разошлась бы с той.
use veldmap_tile_cache_wrap::tile::TILE_FORMAT;

use crate::proto::image_tiler::{
    Described, DescribeRequest, GeoTie, Placement, ProduceDone, ProduceProgress, ProduceRequest,
    TileResult,
};
use crate::proto::tile_cache::StoreTile;

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
/// Слота два, по весу разбора. Тяжёлый — тот, что держит при себе отсчёты
/// величины: у NetCDF это вся плоскость, до полугигабайта. Держать два таких
/// разом лимит памяти инстанса не переживёт, поэтому тяжёлый один и
/// отпускается ДО того, как начнётся новый разбор.
///
/// Лёгкий — заголовки и каталоги (TIFF, JPEG, PNG), и он тяжёлому не мешает.
/// Порознь они затем, что у наложения растров два и приходят они парами
/// вперемежку: описали квиклук, описали гранулу, произвели квиклук, произвели
/// гранулу. Одним слотом квиклук вытеснял бы разбор гранулы ровно между её
/// описанием и производством — и она разбиралась бы дважды, второй раз уже
/// после того, как заказчик решил, что снимок вот-вот появится.
struct Parsed {
    resource: u64,
    /// Лежит ли источник на диске. В ключе memo, а не рядом с ним: разбор
    /// зависит от этого довода (по сети у NetCDF свой потолок терпения), и
    /// ключ, знающий только про ресурс, однажды отдаст разбор, сделанный при
    /// другом ответе.
    near: bool,
    info: adapters::Info,
    /// Ключ этого источника в кэше тайлов. Лежит рядом с разбором затем, что
    /// считается он тем же чтением файла: голова и хвост по 64 КиБ, то есть у
    /// удалённого ресурса — два похода к блочному пулу. Разбор переживает
    /// десятки заказов (ступень лестницы, обнажившийся край), и отпечаток
    /// обязан пережить столько же: он свойство файла, а не заказа.
    fingerprint: String,
}

pub struct State {
    heavy: Option<Parsed>,
    light: Option<Parsed>,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { heavy: None, light: None })
}

impl State {
    /// Положить разбор в слот по его весу. Тяжёлый — тот, что держит при себе
    /// отсчёты величины; лёгкий — заголовки. Своим слотом каждому затем, что
    /// иначе дешёвый вытеснял бы дорогой.
    fn keep(&mut self, kept: Parsed) {
        match kept.info.holds_samples() {
            true => self.heavy = Some(kept),
            false => self.light = Some(kept),
        }
    }

    /// Отпустить то, чего нельзя держать вдвоём: отсчёты величины.
    ///
    /// Зовётся перед всяким новым разбором и после прохода, которому разбор
    /// больше не понадобится. Лёгкий при этом остаётся — он стоит заголовков,
    /// и вытеснять его незачем.
    fn release_heavy(&mut self) {
        self.heavy = None;
    }

    /// Разбор этого ресурса, если он лежит в каком-нибудь из слотов.
    fn kept(&self, resource_id: u64, near: bool) -> Option<&Parsed> {
        [self.heavy.as_ref(), self.light.as_ref()]
            .into_iter()
            .flatten()
            .find(|kept| kept.resource == resource_id && kept.near == near)
    }
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
    near: bool,
) -> Result<(&'a Parsed, Duration), String> {
    let mut stamped = Duration::ZERO;
    if state.kept(resource_id, near).is_none() {
        // Отпускается здесь, до нового разбора: две плоскости разом лимит
        // памяти инстанса не переживёт.
        state.release_heavy();
        veldsdk::log::debug!(target: "decode", "разбираем ресурс {} ({} байт)", resource_id, size);
        let began = Instant::now();
        let fingerprint = fingerprint::fingerprint(resource_id, size)?;
        stamped = began.elapsed();
        let info = adapters::describe(resource_id, size, bytes, near)?;
        state.keep(Parsed { resource: resource_id, near, info, fingerprint });
    } else {
        veldsdk::log::debug!(target: "decode", "разбор ресурса {} взят готовым", resource_id);
    }
    Ok((state.kept(resource_id, near).expect("разбор только что положен"), stamped))
}

pub fn on_describe(state: &mut State, req: DescribeRequest) {
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    // Сорвавшееся описание раскладки по шагам не оставляет — она печатается в
    // конце удавшегося. А сорваться оно может дорого: истёкшая подпись
    // отказывает после трёх попыток с паузами, и молчаливым это ожидание
    // выглядит так же, как удачное.
    let began = Instant::now();
    let described = match describe(state, &req) {
        Ok(described) => described,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {} (за {:.2} с)",
                                label, error, began.elapsed().as_secs_f32());
            Described { error, ..Default::default() }
        }
    };
    crate::emit::on_described(&described, &correlation);
}

fn describe(state: &mut State, req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;
    let began = Instant::now();
    // Привязка приезжает из соседнего файла и в memo не идёт: она свойство
    // пары «растр и его координаты», а memo знает только про растр.
    let mut ties = Vec::new();
    let (kept, stamped) =
        parsed(state, resource.id, resource.size, &Rc::new(Cell::new(0)), req.near)?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());
    let read = began.elapsed();

    // Координаты из соседнего файла — только когда в самом растре их нет: то,
    // что записано в нём, знает о своей раскладке точнее любого соседа.
    //
    // Неудача здесь описания не отменяет. Растр от этого не портится, он лишь
    // остаётся без привязки — и что с ним тогда делать, решает заказчик: у
    // него есть контур каталога, а у нас нет ничего. Но и молчать о ней
    // нельзя: по пустой привязке заказчику не отличить «в файлах не сказано»
    // от «сказано, да не прочиталось».
    let mut sidecar = None;
    if info.ties.is_empty()
        && info.placement.is_none()
        && let Some(coordinates) = req.geolocation.as_ref()
    {
        match adapters::netcdf::geolocation(
            coordinates.id,
            coordinates.size,
            info.frame,
            info.width,
            info.height,
        ) {
            Ok(found) => ties = found,
            Err(error) => {
                veldsdk::log::warn!(target: "decode", "файл координат: {}", error);
                sidecar = Some(format!("файл координат: {}", error));
            }
        }
    }

    // Описание — самая долгая и самая молчаливая часть показа: прогресса у
    // него нет вовсе (см. schema.yaml), а внутри лежит и отпечаток двумя
    // концами файла, и разбор, и соседний файл координат.
    //
    // Секунды у отпечатка — это сеть, и разбор ими же и оплачен: голова файла
    // нужна обоим, и после отпечатка она лежит в блочном кэше хоста. Читать
    // «отпечаток 2,6 — разбор 0,0» как «разбор дёшев» нельзя.
    //
    // Только время: сколько уехало по проводу, отсюда не видно — счётчик
    // `Metered` считает дальнюю достигнутую позицию, а не объём, и провод
    // мерит хост (`network::perf`).
    // Привязка в ответе есть — своя у растра либо приехавшая из соседнего файла.
    let placed = !info.ties.is_empty() || info.placement.is_some() || !ties.is_empty();

    let total = began.elapsed();
    veldsdk::log::debug!(target: "perf",
        "описание ресурса {}: {:.2} с — отпечаток {:.2}, разбор {:.2}, координаты {:.2}",
        resource.id, total.as_secs_f32(), stamped.as_secs_f32(),
        (read - stamped).as_secs_f32(), (total - read).as_secs_f32());

    Ok(Described {
        fingerprint,
        width: info.width,
        height: info.height,
        tile: pyramid::TILE,
        levels: pyramid::level_count(info.width, info.height),
        reach: info.reach() as i32,
        finest: info.finest,
        windowed: info.windowed(),
        ties: info
            .ties
            .iter()
            .chain(ties.iter())
            .map(|tie| GeoTie { px: tie.px, py: tie.py, lat: tie.lat, lon: tie.lon })
            .collect(),
        placement: info.placement.as_ref().map(|found| Placement {
            epsg: found.epsg,
            x_per_i: found.affine[0],
            x_per_j: found.affine[1],
            x0: found.affine[2],
            y_per_i: found.affine[3],
            y_per_j: found.affine[4],
            y0: found.affine[5],
        }),
        error: String::new(),
        // Оговорка едет ровно тогда, когда привязки в ответе нет: обещано полем
        // «привязку взять не удалось», и присланная вместе со взятой привязкой
        // она называла бы беду там, где её нет. Ловится это здесь, а не у
        // потребителя: адаптер про соседний файл координат не знает, а тот его
        // жалобу как раз и отменяет, когда сам справился.
        //
        // Обе половины, а не старшая: они про разные файлы, и вторая первую не
        // отменяет. Каждая называет свой — иначе склеенное читается повтором,
        // потому что беда у них бывает одна и та же.
        binding_trouble: match placed {
            true => String::new(),
            false => [
                info.binding_trouble.as_ref().map(|said| format!("растр: {}", said)),
                sidecar,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join("; "),
        },
    })
}

pub fn on_produce(state: &mut State, req: ProduceRequest) {
    // Корреляция запроса — она же имя операции у платформы: ею заказчик её
    // и убьёт, если тайлы перестанут быть нужны.
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let began = Instant::now();
    let error = match produce(state, &req, &correlation) {
        Ok(()) => String::new(),
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            error
        }
    };
    // Проход — самая долгая работа тайлера, и мерить её больше нечем: описание
    // печатает свою раскладку, а во что обошлось само декодирование, из логов
    // не следовало никак. Числами взяты те же, какими проход отчитывается о
    // ходе: без уровня и числа тайлов секунды не с чем сравнить — у грубой
    // ступени и у подробной работа разная.
    //
    // Сорвавшийся мерится наравне с удавшимся: его секунды человек прождал так
    // же, а по молчанию долгий отказ неотличим от быстрого.
    veldsdk::log::debug!(target: "perf",
        "проход по ресурсу {}: {:.2} с — уровень {}, тайлов {}{}",
        req.resource.as_ref().map_or(0, |handle| handle.id),
        began.elapsed().as_secs_f32(),
        req.level,
        req.tiles.len(),
        match error.is_empty() {
            true => String::new(),
            false => format!(", сорвался: {}", error),
        });
    crate::emit::on_produce_done(&ProduceDone { error }, &correlation);
}

fn produce(state: &mut State, req: &ProduceRequest, correlation: &str) -> Result<(), String> {
    // Владелец будущих текстур — тот, кто прислал запрос: без имени
    // передавать их некому.
    let owner = veldsdk::resource::requester("image-tiler/on_produce")?;
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;

    let bytes = Rc::new(Cell::new(0u64));
    let (kept, _) = parsed(state, resource.id, resource.size, &bytes, req.near)?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());

    let single_pass = matches!(info.reach(), crate::proto::image_tiler::Reach::Pyramid);
    // Заказ проверяется отдельной функцией, а не по месту: у проверок свои
    // выходы, и уйти по любому из них, не отпустив разбор, значит оставить
    // висеть отсчёты величины — ровно то, от чего memo и освобождают ниже.
    let planned = plan(info, req);

    let mut sink = None;
    let outcome = match planned {
        Err(why) => Err(why),
        Ok(ordered) => {
            let wants: BTreeSet<(u32, u32)> = ordered.iter().copied().collect();
            let mut ready = Sink {
                correlation,
                owner,
                fingerprint,
                want_level: req.level,
                wants,
                want_total: ordered.len() as u32,
                done: 0,
                bytes: bytes.clone(),
                // У источника, чей разбор уже держит отсчёты величины, байты
                // прохода о работе впереди не говорят ничего: читать больше
                // нечего, а стои́т время разворот плоскости в пирамиду.
                // Знаменатель нулём — это и есть «мерить нечем» (см.
                // `tiles::readable`), и подпись тогда идёт ступенями и
                // ячейками, а не мегабайтами.
                total_bytes: match info.holds_samples() {
                    true => 0,
                    false => resource.size,
                },
                reported: 0,
            };
            let outcome = {
                let mut emit = |level: u32, tx: u32, ty: u32, w: u32, h: u32, rgba: &[u8]| {
                    ready.emit(level, tx, ty, w, h, rgba)
                };
                adapters::produce(
                    resource.id, resource.size, info, req.level, &ordered, &bytes, &mut emit,
                )
            };
            sink = Some(ready);
            outcome
        }
    };
    // Разбор, из которого проход строит всю пирамиду разом, после него не
    // нужен: второго прохода по такому источнику не будет, пока не спросят
    // заново, — а держит он с собой отсчёты величины, то есть до восьмисот
    // мегабайт
    // памяти инстанса (`netcdf::PLANE_BUDGET`). Остаётся он у тех, к кому
    // приходят за каждой ступенью отдельно (тайловый TIFF, JPEG 2000), — там
    // разбор это заголовки, и он дёшев.
    //
    // Отпускается до разбора исхода, а не после: сорвавшийся проход — самый
    // обычный конец (оборвалось чтение по сети), и уйти по `?`, оставив
    // полгигабайта висеть, значит не сделать ровно того, ради чего это здесь.
    if single_pass {
        state.release_heavy();
    }
    outcome?;

    // Проход кончился, а запрошенное не всё отдано — это ошибка адаптера,
    // и молчать о ней значит показать заказчику дыру без причины.
    if let Some(sink) = sink
        && !sink.wants.is_empty()
    {
        return Err(format!("адаптер не произвёл {} из запрошенных тайлов", sink.wants.len()));
    }
    Ok(())
}

/// Какие ячейки заказаны — с проверкой, что такие вообще бывают.
///
/// Чистой функцией над описанием и запросом: правило «уровень есть, ячейка в
/// его сетке» не зависит ни от источника, ни от состояния модуля.
fn plan(info: &adapters::Info, req: &ProduceRequest) -> Result<Vec<(u32, u32)>, String> {
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

    // Рядами, а не столбцами. Чанки тайлового TIFF адресуются
    // `cy * across + cx`, то есть лежат рядами, а упреждающее чтение хоста
    // узнаёт последовательный проход по тому, что промах пришёлся ровно за
    // концом прошлого запроса. Столбцовый обход прыгает через всю ширину
    // чанковой сетки на каждом тайле, и разгон сбрасывается в один блок.
    //
    // Порядок отдаётся отсюда, а не наводится над готовым множеством: там его
    // можно забыть навести, и заказ снова поехал бы столбцами молча.
    //
    // Скачки он убирает не все, а только между тайлами. Внутри тайла область
    // собирается циклом по рядам чанков, и когда чанк ниже этой области —
    // у файла с внутренними тайлами 256×256 против наших 512 — следующий тайл
    // ряда возвращается к её первому ряду чанков. Там выигрыш падает с
    // семидесяти процентов до единиц, и лечится это уже не порядком заказа.
    // По той же причине выигрыш стои́т на том, что уровень читается из своей
    // копии: из копии вдвое крупнее один тайл накрывает 2×2 чанка, и возврат
    // случается на каждом.
    //
    // Выигрыш этот — в числе запросов, а не в байтах. Разогнавшееся чтение
    // берёт кусок наперёд, и на разрыве ряда хвост уезжает за правый край
    // окна, где его никто не спросит: у широкого окна запросов вдвое меньше
    // при тех же байтах, у узкого и высокого байт до полутора раз больше при
    // том же числе запросов. Пул при этом общий на процесс, и лишнее в нём
    // вытесняет чужое оплаченное — видно это строкой `network::perf`.
    //
    // И само правило — про индекс чанка, а не про его место в файле:
    // физический порядок задаёт `TileOffsets`, и монотонным он обязан быть
    // только по обычаю писателя. Пересобранный чужим инструментом файл вернёт
    // те же скачки, и заметить это можно лишь тем же счётчиком.
    let mut ordered: Vec<(u32, u32)> = wants.into_iter().collect();
    ordered.sort_by_key(|&(x, y)| (y, x));
    Ok(ordered)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::image_tiler::TileAddr;

    fn keep(state: &mut State, resource: u64, info: adapters::Info) {
        state.keep(Parsed { resource, near: true, info, fingerprint: String::new() });
    }

    /// Лёгкий разбор не вытесняет тяжёлый. У наложения растров два, и приходят
    /// они парами вперемежку: описали квиклук, описали гранулу, произвели
    /// квиклук, произвели гранулу. Одним слотом квиклук выбрасывал бы разбор
    /// гранулы ровно между её описанием и производством — и вся плоскость
    /// разворачивалась бы второй раз, уже после того, как заказчик решил, что
    /// снимок вот-вот появится.
    #[test]
    fn лёгкий_разбор_не_вытесняет_тяжёлый() {
        let mut state = State { heavy: None, light: None };

        keep(&mut state, 1, adapters::Info::heavy(1500, 1202));
        keep(&mut state, 2, adapters::Info::plain(2422, 1940, adapters::Kind::Jpeg));

        assert!(state.kept(1, true).is_some(), "гранула на месте");
        assert!(state.kept(2, true).is_some(), "и квиклук рядом");
    }

    /// Перед новым разбором отпускается тяжёлый, а лёгкий остаётся. Отпусти
    /// мы не тот — квиклук пережил бы гранулу, а гранула разбиралась бы
    /// заново, то есть ровно то, ради чего слоты и разведены.
    #[test]
    fn перед_разбором_отпускается_тяжёлый_а_лёгкий_остаётся() {
        let mut state = State { heavy: None, light: None };
        keep(&mut state, 1, adapters::Info::heavy(1500, 1202));
        keep(&mut state, 2, adapters::Info::plain(2422, 1940, adapters::Kind::Jpeg));

        state.release_heavy();

        assert!(state.kept(1, true).is_none(), "плоскость отпущена");
        assert!(state.kept(2, true).is_some(), "заголовки остались");
    }

    /// А тяжёлый тяжёлый вытесняет: две плоскости разом лимит памяти инстанса
    /// не переживёт, и это единственное, ради чего слот вообще ограничен.
    #[test]
    fn тяжёлый_разбор_вытесняет_тяжёлый() {
        let mut state = State { heavy: None, light: None };

        keep(&mut state, 1, adapters::Info::heavy(1500, 1202));
        keep(&mut state, 2, adapters::Info::heavy(3000, 2404));

        assert!(state.kept(1, true).is_none(), "первая плоскость отпущена");
        assert!(state.kept(2, true).is_some());
    }

    /// И лёгкий лёгкий вытесняет тоже: держать их без счёта незачем, а разбор
    /// заголовков стоит одного чтения головы файла.
    #[test]
    fn лёгкий_разбор_вытесняет_лёгкий() {
        let mut state = State { heavy: None, light: None };

        keep(&mut state, 1, adapters::Info::plain(64, 64, adapters::Kind::Png));
        keep(&mut state, 2, adapters::Info::plain(64, 64, adapters::Kind::Png));

        assert!(state.kept(1, true).is_none());
        assert!(state.kept(2, true).is_some());
    }

    fn asked(level: u32, tiles: &[(u32, u32)]) -> ProduceRequest {
        ProduceRequest {
            level,
            tiles: tiles.iter().map(|&(x, y)| TileAddr { x, y }).collect(),
            ..Default::default()
        }
    }

    /// Заказ отдаётся рядами. Это не вкус: чанки тайлового TIFF лежат рядами,
    /// и столбцовый обход прыгает через всю ширину чанковой сетки на каждом
    /// тайле — упреждающее чтение читает такой скачок как случайный доступ и
    /// сбрасывает разгон, так что весь проход едет блоками по одному.
    #[test]
    fn заказ_отдаётся_рядами_а_не_столбцами() {
        let info = adapters::Info::plain(2048, 2048, adapters::Kind::Png);
        let req = asked(0, &[(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)]);

        let ordered = plan(&info, &req).expect("сетка вмещает");

        assert_eq!(ordered, vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1)]);
    }

    /// Повторы в заказе не удваивают работу: тайл производят один раз.
    #[test]
    fn повторённый_тайл_заказан_однажды() {
        let info = adapters::Info::plain(2048, 2048, adapters::Kind::Png);
        let req = asked(0, &[(1, 1), (0, 0), (1, 1)]);

        assert_eq!(plan(&info, &req).expect("сетка вмещает"), vec![(0, 0), (1, 1)]);
    }

    /// Тайла за краем сетки не бывает, и молча пропустить его нельзя: заказчик
    /// ждал бы его до закрытия вида.
    #[test]
    fn тайл_за_краем_сетки_отвергается() {
        let info = adapters::Info::plain(1024, 1024, adapters::Kind::Png);

        assert!(plan(&info, &asked(0, &[(2, 0)])).is_err(), "сетка уровня 0 — 2×2");
        assert!(plan(&info, &asked(1, &[(1, 0)])).is_err(), "у первого уровня она 1×1");
        assert!(plan(&info, &asked(9, &[(0, 0)])).is_err(), "девятого уровня нет вовсе");
    }
}
