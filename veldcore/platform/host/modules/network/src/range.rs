//! Удалённый файл как ресурс (топик network/open): читается Range-запросами,
//! целиком не скачивается.
//!
//! Для читателя такой ресурс неотличим от файла — тот же `resource_read(id,
//! offset, size)`. Поэтому декодер, умеющий работать окнами, снимает превью
//! со снимка на гигабайты, вытянув заголовок и несколько тайлов: остальное
//! по проводу не идёт. Условия — сервер отвечает на Range (иначе открытие
//! завершится ошибкой сразу, а не посреди чтения) и формат допускает
//! произвольный доступ.
//!
//! Заголовки авторизации — снимок на момент открытия (см. RemoteOpenRequest):
//! ресурс живёт ровно столько, сколько они действительны. Для подписи вида
//! AWS SigV4 это порядка четверти часа — достаточно, чтобы открыть и
//! декодировать, но не для ресурса, который держат открытым часами.

use super::State;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::RemoteOpenRequest;
use veldmap_host_util::{blocking, opened, opened_handle, Caller, RangeSource};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Размер блока кэша — наименьший кусок, которым ходят в сеть. Вдвое крупнее
/// окна читателя (256 КиБ), и не больше: за 64 килобайтами заголовка не должны
/// ехать мегабайты. У eodata запрос стоит около полусекунды, а мегабайт — около
/// секунды, поэтому случайному чтению дешевле сходить ещё раз, чем взять
/// вчетверо больше нужного.
const BLOCK: u64 = 512 * 1024;
/// Потолок упреждающего чтения: последовательный проход разгоняется до него,
/// чтобы не платить задержкой запроса за каждый блок (см. [`Readahead`]).
const READAHEAD: u64 = 8 * 1024 * 1024;

/// Потолок кэша на один ресурс. Проход по гигабайтному снимку не должен
/// превращаться в его копию в памяти — что не влезло, перечитается.
const CACHE_LIMIT: u64 = 64 * 1024 * 1024;

/// Задачи здесь нет намеренно, в отличие от download и http: открытие — это
/// один пробный запрос, ограниченный таймаутами клиента (см. http::client),
/// и отменять в нём нечего. Долгая часть — чтение, а оно идёт через ABI
/// памяти, вне системы задач; корреляция запроса достаётся задаче того,
/// кто ресурс потом читает (например, декодирования в image-loader).
pub fn on_open(state: &State, req: RemoteOpenRequest, caller: Caller) {
    let Caller { instance, correlation, .. } = caller;

    // Пробный запрос уходит в сеть, поэтому не в async-обработчике.
    blocking(&state.ctx, move |ctx| {
        let result = match HttpRange::open(&req.url, req.headers) {
            Ok(source) => {
                let len = source.len();
                let id = ctx.memory.alloc_range(Arc::new(source), instance);
                log::info!(target: "network", "Opened remote resource {} ({} bytes): {}", id, len, req.url);
                opened_handle(id, len)
            }
            Err(e) => {
                // Ошибка уходит событием заказчику, но на экране её увидит
                // только тот, кто в этот момент смотрит на превью — в логе она
                // нужна независимо от этого.
                log::warn!(target: "network", "Failed to open remote resource {}: {}", req.url, e);
                opened(Err(e.to_string()))
            }
        };
        bus::emit::on_open_result(&*ctx.publisher, &result, &correlation);
    });
}

/// Носитель поверх HTTP: блоки тянутся по требованию и кэшируются.
struct HttpRange {
    url: String,
    headers: HashMap<String, String>,
    len: u64,
    /// Хендл рантайма: read_at вызывается хостом с blocking-пула, где
    /// асинхронный запрос надо кому-то отдать.
    runtime: tokio::runtime::Handle,
    cache: Mutex<Cache>,
    /// Сколько байт реально ушло по проводу. Смысл оконного чтения в том,
    /// чтобы это была доля файла, а не он весь, — но доля зависит от формата
    /// (тайловый TIFF с пирамидой читается кусками, PNG приходится прочесть
    /// целиком). Поэтому не утверждение в комментарии, а счётчик: итог
    /// пишется в лог при закрытии ресурса.
    fetched: std::sync::atomic::AtomicU64,
}

/// Ресурс закрыт (гость освободил его через veld_resource_free) — подводим
/// итог по трафику.
impl Drop for HttpRange {
    fn drop(&mut self) {
        let fetched = self.fetched.load(std::sync::atomic::Ordering::Relaxed);
        let share = if self.len > 0 { fetched * 100 / self.len } else { 0 };
        log::info!(target: "network", "Closed remote resource: fetched {} of {} bytes ({}%): {}",
                   fetched, self.len, share, self.url);
    }
}

/// Блоки хранятся под Arc: читатель ходит окнами по 256 КБ (ResourceReader),
/// и копировать ради каждого окна весь блок незачем.
#[derive(Default)]
struct Cache {
    blocks: HashMap<u64, Arc<[u8]>>,
    /// Порядок появления — им же и вытесняем: у последовательного прохода
    /// (а это основной сценарий) самый старый блок и есть самый ненужный.
    order: std::collections::VecDeque<u64>,
    bytes: u64,
    readahead: Readahead,
}

/// Разгон последовательного чтения: сколько блоков брать одним запросом.
///
/// Мелкий блок хорош случайному чтению и плох проходу по файлу — на каждый
/// блок пришлась бы задержка запроса. Поэтому блок остаётся мелким, а проход
/// узнаётся по тому, что промах пришёлся ровно туда, где кончился прошлый
/// запрос: тогда кусок удваивается, и уже через несколько промахов запросы
/// идут мегабайтами. Скачок в сторону сбрасывает разгон — там снова дешевле
/// взять один блок, чем тянуть упреждением то, чего никто не спросит.
#[derive(Default)]
struct Readahead {
    /// Блок сразу за концом прошлого запроса — на нём проход и опознаётся.
    next: u64,
    /// Сколько блоков взял прошлый запрос.
    run: u64,
}

impl Readahead {
    /// Сколько блоков брать, начиная с промаха на `index`. `total` — всего
    /// блоков у ресурса: за конец файла упреждать нечего.
    fn plan(&mut self, index: u64, total: u64) -> u64 {
        let run = match index == self.next {
            true => (self.run * 2).min(READAHEAD / BLOCK),
            false => 1,
        };
        let run = run.clamp(1, total.saturating_sub(index).max(1));
        self.next = index + run;
        self.run = run;
        run
    }
}

impl Cache {
    /// Кладёт блок, вытесняя старые. Если блок уже есть (два читателя
    /// запросили его одновременно), возвращается лежащий: учёт байт должен
    /// совпадать с содержимым, иначе потолок кэша поплывёт.
    fn insert(&mut self, index: u64, data: Arc<[u8]>) -> Arc<[u8]> {
        if let Some(present) = self.blocks.get(&index) {
            return present.clone();
        }
        while self.bytes + data.len() as u64 > CACHE_LIMIT {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(dropped) = self.blocks.remove(&oldest) {
                self.bytes -= dropped.len() as u64;
            }
        }
        self.bytes += data.len() as u64;
        self.order.push_back(index);
        self.blocks.insert(index, data.clone());
        data
    }
}

impl HttpRange {
    /// Пробный запрос первого байта: заодно проверяет, что сервер понимает
    /// Range, и узнаёт полный размер из Content-Range.
    fn open(url: &str, headers: HashMap<String, String>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        let response = runtime.block_on(super::http::get(url, &headers, Some((0, 1))).send())?;

        // Range здесь ни при чём, если ответ вообще не про содержимое: 404 —
        // это неверный адрес, 401/403 — просроченная или чужая подпись. Валить
        // всё в «сервер не поддерживает Range» значило бы уводить от причины;
        // отсутствие поддержки — это именно 200 вместо 206.
        let status = response.status();
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            if status == reqwest::StatusCode::OK {
                anyhow::bail!("сервер не поддерживает Range: на запрос диапазона ответил целым файлом (HTTP 200)");
            }
            anyhow::bail!("удалённый ресурс не открыт: HTTP {} на {}", status, url);
        }
        let len = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next()?.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("сервер не сообщил размер файла (Content-Range)"))?;

        Ok(Self {
            url: url.to_string(),
            headers,
            len,
            runtime,
            cache: Mutex::new(Cache::default()),
            fetched: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Блок из кэша или из сети. Промах тянет не один блок, а столько, сколько
    /// назначил разгон, — и одним запросом: цена запроса не зависит от того,
    /// сколько в нём байт (см. [`Readahead`]).
    fn block(&self, index: u64) -> anyhow::Result<Arc<[u8]>> {
        let run = {
            let mut cache = self.cache.lock().unwrap();
            if let Some(data) = cache.blocks.get(&index) {
                return Ok(data.clone());
            }
            cache.readahead.plan(index, self.len.div_ceil(BLOCK))
        };

        let from = index * BLOCK;
        let to = (from + run * BLOCK).min(self.len);
        let expected = to - from;
        let response = self.runtime.block_on(
            super::http::get(&self.url, &self.headers, Some((from, to))).send(),
        )?;

        // Только 206: ответ 200 означал бы, что Range проигнорирован и пришёл
        // весь файл — принять его за блок значило бы сдвинуть все смещения.
        let status = response.status();
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!("доступ к удалённому ресурсу больше не действителен (HTTP {}): \
                               заголовки авторизации выданы при открытии и могли истечь", status);
            }
            anyhow::bail!("чтение диапазона {}..{}: HTTP {}", from, to, status);
        }

        let data = self.runtime.block_on(response.bytes())?;
        // Короткий ответ — это обрыв, а не конец файла: длину мы знаем из
        // Content-Range и запросили ровно столько, сколько есть.
        if data.len() as u64 != expected {
            anyhow::bail!("чтение диапазона {}..{}: получено {} байт вместо {}",
                          from, to, data.len(), expected);
        }
        self.fetched.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);

        // Пришедшее раскладывается по блокам целиком: упреждающая часть за это
        // и заплачена, а выбросить её значило бы перечитать её же следующим
        // окном читателя.
        let mut cache = self.cache.lock().unwrap();
        let mut wanted = None;
        for (step, chunk) in data.chunks(BLOCK as usize).enumerate() {
            let stored = cache.insert(index + step as u64, Arc::from(chunk));
            if step == 0 {
                wanted = Some(stored);
            }
        }
        wanted.ok_or_else(|| anyhow::anyhow!("чтение диапазона {}..{}: пустой ответ", from, to))
    }
}

impl RangeSource for HttpRange {
    fn len(&self) -> u64 {
        self.len
    }

    /// Диапазон приходит уже проверенным (см. `RangeSource`), поэтому здесь
    /// только сборка ответа из блоков.
    fn read_at(&self, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(size as usize);

        let mut position = offset;
        while (out.len() as u64) < size {
            let block = self.block(position / BLOCK)?;
            let start = (position % BLOCK) as usize;
            let take = ((size - out.len() as u64) as usize).min(block.len() - start);
            out.extend_from_slice(&block[start..start + take]);
            position += take as u64;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проход разгоняется, скачок в сторону — нет. Цена ошибки несимметрична:
    /// не разогнаться на проходе значит платить задержкой за каждые полмегабайта,
    /// а разогнаться на случайном чтении — тянуть мегабайты ради заголовка.
    #[test]
    fn sequential_reads_speed_up_random_ones_do_not() {
        let total = 1024;
        let mut readahead = Readahead::default();

        // Первый промах — один блок: о проходе ещё ничего не известно.
        assert_eq!(readahead.plan(0, total), 1);
        // Дальше подряд — удвоение до потолка.
        let cap = READAHEAD / BLOCK;
        assert_eq!(readahead.plan(1, total), 2);
        assert_eq!(readahead.plan(3, total), 4);
        assert_eq!(readahead.plan(7, total), 8);
        assert_eq!(readahead.plan(15, total), cap);
        assert_eq!(readahead.plan(15 + cap, total), cap, "выше потолка не растёт");

        // Скачок в сторону сбрасывает разгон.
        assert_eq!(readahead.plan(500, total), 1);
    }

    /// За концом файла упреждать нечего: запрошенный диапазон обрежется по
    /// длине, а короткий ответ здесь читается как обрыв связи.
    #[test]
    fn readahead_stops_at_the_last_block() {
        let mut readahead = Readahead::default();
        assert_eq!(readahead.plan(8, 10), 1);
        assert_eq!(readahead.plan(9, 10), 1, "остался один блок");
    }
}
