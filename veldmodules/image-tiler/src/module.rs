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
pub mod budget;
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
    Described, DescribeRequest, GeoTie, Level, Placement, ProduceDone, ProduceProgress,
    ProduceRequest, Serve, TileResult, Variable,
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
/// ресурс, а за одним описанием идёт столько проходов, сколько у источника
/// ступеней; разбор — это заголовки и каталоги, у NetCDF ещё и выборка окон
/// величины (`netcdf::describe`), и по сети каждый стои́т походов к блочному
/// пулу. Отсчётов разбор не держит ни у одного формата: они читаются окнами
/// по заказу, и держать в memo нечего, кроме раскладки.
struct Parsed {
    resource: u64,
    info: adapters::Info,
    /// Ключ этого источника в кэше тайлов. Лежит рядом с разбором затем, что
    /// считается он тем же чтением файла: голова и хвост по 64 КиБ, то есть у
    /// удалённого ресурса — два похода к блочному пулу. Разбор переживает
    /// десятки заказов (ступень лестницы, обнажившийся край), и отпечаток
    /// обязан пережить столько же: он свойство файла, а не заказа.
    fingerprint: String,
}

/// Сколько разборов лежит в memo. Два, потому что растры приходят парами
/// вперемежку — описали квиклук, описали гранулу, произвели квиклук,
/// произвели гранулу, — и одним слотом каждый вытеснял бы каждого: разбор
/// квиклука не пережил бы даже своего собственного прохода. Больше незачем:
/// заказчиков у тайлера двое, и у каждого на виду одна пара.
const MEMO_SLOTS: usize = 2;

pub struct State {
    /// Разборы от свежего к старому; лишний выпадает с хвоста.
    kept: Vec<Parsed>,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { kept: Vec::new() })
}

impl State {
    /// Положить разбор свежим; старейший сверх [`MEMO_SLOTS`] выпадает.
    fn keep(&mut self, kept: Parsed) {
        self.kept.retain(|other| other.resource != kept.resource);
        self.kept.insert(0, kept);
        self.kept.truncate(MEMO_SLOTS);
    }

    /// Отметить разбор ресурса свежим; `false` — его в memo нет.
    fn touch(&mut self, resource_id: u64) -> bool {
        match self.kept.iter().position(|kept| kept.resource == resource_id) {
            Some(at) => {
                let kept = self.kept.remove(at);
                self.kept.insert(0, kept);
                true
            }
            None => false,
        }
    }

    /// Сколько памяти держат разборы чужих источников, лежащие в memo: работа
    /// над своим идёт рядом с ними, и её пик обязан учесть их слагаемым.
    fn neighbour_footprint(&self, resource_id: u64) -> u64 {
        self.kept
            .iter()
            .filter(|kept| kept.resource != resource_id)
            .map(|kept| kept.info.footprint())
            .sum()
    }

    /// Разбор этого ресурса, если он лежит в memo.
    fn kept(&self, resource_id: u64) -> Option<&Parsed> {
        self.kept.iter().find(|kept| kept.resource == resource_id)
    }
}

/// Разбор источника — из memo, если это тот же ресурс, иначе заново; с ним —
/// сколько длился отпечаток и сколько памяти держат разборы соседей, оставшиеся
/// в memo ([`State::neighbour_footprint`]): пока разбор взят ссылкой, самого
/// `state` больше не спросить.
///
/// Отдаётся ссылкой, а не значением: за одним описанием идёт столько проходов,
/// сколько у источника ступеней (у тайлового TIFF это четыре-пять), и разбор,
/// расходуемый первым же из них, экономил бы ровно один.
fn parsed<'a>(
    state: &'a mut State,
    resource_id: u64,
    size: u64,
    bytes: &Rc<Cell<u64>>,
) -> Result<(&'a Parsed, Duration, u64), String> {
    let mut stamped = Duration::ZERO;
    if state.touch(resource_id) {
        veldsdk::log::debug!(target: "decode", "разбор ресурса {} взят готовым", resource_id);
    } else {
        veldsdk::log::debug!(target: "decode", "разбираем ресурс {} ({} байт)", resource_id, size);
        let began = Instant::now();
        let fingerprint = fingerprint::fingerprint(resource_id, size)?;
        stamped = began.elapsed();
        let info = adapters::describe(resource_id, size, bytes)?;
        state.keep(Parsed { resource: resource_id, info, fingerprint });
    }
    let neighbour = state.neighbour_footprint(resource_id);
    Ok((state.kept(resource_id).expect("разбор только что положен"), stamped, neighbour))
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

/// Величина на провод — как есть.
fn wire_variable(variable: &adapters::Variable) -> Variable {
    Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

fn describe(state: &mut State, req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "в запросе нет ресурса".to_string())?;
    let began = Instant::now();
    // Привязка приезжает из соседнего файла и в memo не идёт: она свойство
    // пары «растр и его координаты», а memo знает только про растр.
    let mut ties = Vec::new();
    let (kept, stamped, _) = parsed(state, resource.id, resource.size, &Rc::new(Cell::new(0)))?;
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
        // Таблица уровней — та же, по которой `produce` выберет рукав: строка
        // на уровень, как она посчитана, без пересказа скалярами.
        levels: info
            .levels()
            .iter()
            .map(|row| Level {
                serve: match row.serve {
                    adapters::table::Serve::Pointwise => Serve::Pointwise,
                    adapters::table::Serve::Pass { .. } => Serve::Pass,
                } as i32,
                from: match row.serve {
                    adapters::table::Serve::Pass { from } => from,
                    adapters::table::Serve::Pointwise => 0,
                },
                bytes: row.peak.total(),
                fits: row.fits,
            })
            .collect(),
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
        variable: info.variable.as_ref().map(wire_variable),
        variables: info.variables.iter().map(wire_variable).collect(),
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
    let (kept, _, neighbour) = parsed(state, resource.id, resource.size, &bytes)?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());

    // Пик работы сверяется со свободным до неё — вместе с разбором соседа,
    // который лежит в memo и памяти не отпускает. Строка таблицы та же, по
    // которой описание обещало «влезает»; отказ здесь называет слагаемые.
    let admitted = match info.level(req.level) {
        Some(row) => row.peak.with("разбор соседа", neighbour).admit(),
        None => Ok(()),
    };
    let ordered = plan(info, req)?;
    admitted?;

    let wants: BTreeSet<(u32, u32)> = ordered.iter().copied().collect();
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
        adapters::produce(resource.id, resource.size, info, req.level, &ordered, &bytes, &mut emit)?;
    }

    // Проход кончился, а запрошенное не всё отдано — это ошибка адаптера,
    // и молчать о ней значит показать заказчику дыру без причины.
    if !sink.wants.is_empty() {
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
        state.keep(Parsed { resource, info, fingerprint: String::new() });
    }

    fn quicklook() -> adapters::Info {
        adapters::Info::plain(2422, 1940, adapters::Kind::Jpeg)
    }

    /// Пара растров наложения переживает друг друга. Приходят они парами
    /// вперемежку: описали квиклук, описали гранулу, произвели квиклук,
    /// произвели гранулу. Одним слотом квиклук выбрасывал бы разбор гранулы
    /// ровно между её описанием и производством.
    #[test]
    fn пара_разборов_живёт_в_memo_вместе() {
        let mut state = State { kept: Vec::new() };
        keep(&mut state, 1, adapters::Info::plain(64, 64, adapters::Kind::Png { interlaced: false }));
        keep(&mut state, 2, quicklook());

        assert!(state.touch(1), "гранула на месте");
        assert!(state.touch(2), "и квиклук рядом");
        assert!(state.kept(1).is_some() && state.kept(2).is_some());
    }

    /// Третий разбор вытесняет тот, к которому дольше не обращались, а не
    /// тот, что положен раньше: разбор, взятый готовым, свеж так же, как
    /// новый.
    #[test]
    fn вытесняется_давно_не_спрошенный() {
        let mut state = State { kept: Vec::new() };
        keep(&mut state, 1, quicklook());
        keep(&mut state, 2, quicklook());
        assert!(state.touch(1), "первый спрошен снова");

        keep(&mut state, 3, quicklook());

        assert!(state.kept(1).is_some(), "спрошенный остался");
        assert!(state.kept(2).is_none(), "давно не спрошенный выпал");
        assert!(state.kept(3).is_some());
        assert!(!state.touch(2), "выпавшего готовым не взять");
        assert_eq!(state.kept.len(), MEMO_SLOTS);
    }

    /// Тот же ресурс, разобранный снова, не занимает второго слота.
    #[test]
    fn повторный_разбор_того_же_ресурса_занимает_тот_же_слот() {
        let mut state = State { kept: Vec::new() };
        keep(&mut state, 1, quicklook());
        keep(&mut state, 2, quicklook());
        keep(&mut state, 1, quicklook());

        assert_eq!(state.kept.len(), 2);
        assert!(state.kept(1).is_some() && state.kept(2).is_some());
    }

    /// Сосед в memo входит в пик работы над своим источником: чужой разбор
    /// считается, свой — нет.
    #[test]
    fn сосед_в_memo_входит_в_пик() {
        let mut state = State { kept: Vec::new() };
        let mut tiff = adapters::Info::plain(64, 64, adapters::Kind::Jpeg);
        tiff.ties = vec![adapters::Tie { px: 0.0, py: 0.0, lat: 0.0, lon: 0.0 }];
        keep(&mut state, 2, tiff);

        assert!(state.neighbour_footprint(1) > 0, "чужой разбор не посчитан");
        assert_eq!(state.neighbour_footprint(2), 0, "свой разбор — не сосед");
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
        let info = adapters::Info::plain(2048, 2048, adapters::Kind::Png { interlaced: false });
        let req = asked(0, &[(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)]);

        let ordered = plan(&info, &req).expect("сетка вмещает");

        assert_eq!(ordered, vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1)]);
    }

    /// Повторы в заказе не удваивают работу: тайл производят один раз.
    #[test]
    fn повторённый_тайл_заказан_однажды() {
        let info = adapters::Info::plain(2048, 2048, adapters::Kind::Png { interlaced: false });
        let req = asked(0, &[(1, 1), (0, 0), (1, 1)]);

        assert_eq!(plan(&info, &req).expect("сетка вмещает"), vec![(0, 0), (1, 1)]);
    }

    /// Тайла за краем сетки не бывает, и молча пропустить его нельзя: заказчик
    /// ждал бы его до закрытия вида.
    #[test]
    fn тайл_за_краем_сетки_отвергается() {
        let info = adapters::Info::plain(1024, 1024, adapters::Kind::Png { interlaced: false });

        assert!(plan(&info, &asked(0, &[(2, 0)])).is_err(), "сетка уровня 0 — 2×2");
        assert!(plan(&info, &asked(1, &[(1, 0)])).is_err(), "у первого уровня она 1×1");
        assert!(plan(&info, &asked(9, &[(0, 0)])).is_err(), "девятого уровня нет вовсе");
    }
}
