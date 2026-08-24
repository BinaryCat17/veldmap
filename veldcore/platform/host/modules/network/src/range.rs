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

/// Через сколько проводных байт ресурс отчитывается о чтении.
///
/// Отчёт по набранному объёму, а не при закрытии: ресурс наложения живёт,
/// пока слой на шаре, и `Drop` у него не наступает весь сеанс — то есть
/// именно у крупного и долгого спросить сегодня нечего. Порог, а не часы,
/// потому что мерится здесь провод, а он идёт байтами.
const REPORT_STEP: u64 = 4 * 1024 * 1024;

/// Сколько раз пробовать один и тот же запрос, прежде чем признать чтение
/// сорвавшимся.
///
/// Оконное чтение живёт весь показ снимка и уходит в сеть сотнями запросов, а
/// соединение рвётся: оборванное тело ответа, сброс, 502 у шлюза. Без повтора
/// одна такая осечка стоит целого прохода производителя, а сорвавшийся проход
/// — ячеек, которые потребитель больше не просит (см. `tiles::Fetch::failed`):
/// снимок остаётся с мутным пятном до переоткрытия.
const ATTEMPTS: u32 = 3;

/// Пауза перед повтором; удваивается с каждым. Сброшенное соединение
/// переустанавливается сразу, а перегруженный шлюз — нет.
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

/// Потолок блочных кэшей — общий на процесс, а не на ресурс. Открытых
/// ресурсов столько, сколько попросил сценарий: у наложения это по растру на
/// слой, включая скрытые, — и потолок «на каждого» умножался бы на их число,
/// оставаясь при этом невидимым (ни один бюджет его не считает, потому что
/// байты лежат в куче хоста). Проход по гигабайтному снимку по-прежнему не
/// превращается в его копию в памяти: что не влезло, перечитается.
const POOL_LIMIT: u64 = 256 * 1024 * 1024;

/// Задачи здесь нет намеренно, в отличие от download и http: открытие — это
/// один пробный запрос, ограниченный таймаутами клиента (см. http::client),
/// и отменять в нём нечего. Долгая часть — чтение, а оно идёт через ABI
/// памяти, вне системы задач; корреляция запроса достаётся задаче того,
/// кто ресурс потом читает (например, разбора заголовка в image-tiler).
pub fn on_open(state: &State, req: RemoteOpenRequest, caller: Caller) {
    let Caller { instance, correlation, .. } = caller;
    let blocks = state.blocks.clone();

    // Пробный запрос уходит в сеть, поэтому не в async-обработчике.
    blocking(&state.ctx, move |ctx| {
        let result = match HttpRange::open(&req.url, req.headers, blocks) {
            Ok(source) => {
                let len = source.len();
                let source = Arc::new(source);
                let id = ctx.memory.alloc_range(source.clone(), instance);
                // Носитель узнаёт свой номер только здесь: заводит его реестр
                // ресурсов, а до него носитель уже существует. Без номера его
                // строки не сшить со строками тех, кто его читает, — те знают
                // ресурс только по нему.
                source.answers_to(id);
                log::info!(target: "network", "Открыт удалённый ресурс {} ({} байт): {}", id, len, req.url);
                opened_handle(id, len)
            }
            Err(e) => {
                // Ошибка уходит событием заказчику, но на экране её увидит
                // только тот, кто в этот момент смотрит на превью — в логе она
                // нужна независимо от этого.
                log::warn!(target: "network", "Удалённый ресурс {} не открылся: {}", req.url, e);
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
    /// Общий на все ресурсы пул блоков и ключ владения в нём.
    blocks: Arc<Blocks>,
    owner: u64,
    /// Разгон — наоборот, свой: это состояние потока чтения, а не хранилище.
    readahead: Mutex<Readahead>,
    /// Сколько байт реально ушло по проводу. Смысл оконного чтения в том,
    /// чтобы это была доля файла, а не он весь, — но доля зависит от формата
    /// (тайловый TIFF с пирамидой читается кусками, PNG приходится прочесть
    /// целиком). Поэтому не утверждение в комментарии, а счётчик.
    ///
    /// Считает доставленное: байты оборвавшейся попытки и повторов сюда не
    /// идут, так что на рвущемся канале провод дороже этого числа.
    fetched: std::sync::atomic::AtomicU64,
    /// Сколько диапазонов доставлено. Вместе с `fetched` это средняя длина
    /// запроса, а она и есть ответ на то, включился разгон или нет: без него
    /// всякий запрос ровно в блок (см. [`Readahead`]).
    ///
    /// Походов в сеть было не меньше: сорвавшийся диапазон переспрашивается до
    /// `ATTEMPTS` раз, и все попытки, кроме последней, сюда не идут.
    requests: std::sync::atomic::AtomicU64,
    /// Номер ресурса в реестре — тот же, каким его зовут читатели. Заводится
    /// реестром уже после носителя, поэтому не поле, а ячейка (см.
    /// [`HttpRange::answers_to`]).
    id: std::sync::atomic::AtomicU64,
    /// Попаданий в пул: чтений, которым блок нашёлся готовым. Считает не
    /// блоки, а обращения — окно читателя (256 КиБ) вдвое мельче блока,
    /// поэтому один блок отдаётся дважды.
    hits: std::sync::atomic::AtomicU64,
    /// Блоков этого ресурса привезено — вместе с упреждающими, которых никто
    /// не спрашивал. Больше, чем блоков у файла, значит одно: пул вытеснил
    /// уже привезённое, и оно приехало снова.
    blocks_in: std::sync::atomic::AtomicU64,
    /// Сколько порогов `REPORT_STEP` уже отчитано, считая нулевой. Ноль
    /// значит «ни одного», поэтому первый же поход в сеть отчитывается — у
    /// ресурса мельче порога это единственная строка, которая о нём будет до
    /// закрытия.
    reported: std::sync::atomic::AtomicU64,
}

/// Ресурс закрыт (гость освободил его через veld_resource_free) — блоки прочь,
/// итог по трафику в лог.
impl Drop for HttpRange {
    fn drop(&mut self) {
        let fetched = self.fetched.load(std::sync::atomic::Ordering::Relaxed);
        let share = if self.len > 0 { fetched * 100 / self.len } else { 0 };
        log::info!(target: "network", "Закрыт удалённый ресурс: прочитано {} из {} байт ({}%): {}",
                   fetched, self.len, share, self.url);
        self.report(true);
        self.blocks.release(self.owner);
    }
}

/// Блочные кэши всех открытых удалённых ресурсов: одна карта, один счётчик,
/// один порядок вытеснения.
///
/// Порядок общий не ради стройности, а потому что вытеснять своё — неверно:
/// под давлением активный читатель выбрасывал бы блоки, которые сейчас же и
/// перечитает, пока простаивающий сосед держит свои нетронутыми. Старейший
/// блок пула и есть самый ненужный, чей бы он ни был.
///
/// Блоки хранятся под Arc: читатель ходит окнами по 256 КБ (ResourceReader),
/// и копировать ради каждого окна весь блок незачем.
#[derive(Default)]
pub struct Blocks {
    pool: Mutex<Pool>,
    /// Раздатчик ключей владения. Адрес блока — пара «чей, какой», и ключ
    /// нельзя переиспользовать: закрытый ресурс и открытый следом за ним —
    /// разные файлы с разными смещениями.
    next_owner: std::sync::atomic::AtomicU64,
    /// Сколько блоков вытеснено потолком за сеанс. Свойство пула, а не
    /// ресурса: вытесняют друг друга все открытые вместе, и разделить это по
    /// ресурсам нечем. Ноль здесь значит, что потолка хватает всем.
    evicted: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct Pool {
    blocks: HashMap<(u64, u64), Arc<[u8]>>,
    /// Порядок появления — им же и вытесняем: у последовательного прохода
    /// (а это основной сценарий) самый старый блок и есть самый ненужный.
    /// Ключи закрытого ресурса снимаются отсюда вместе с его блоками: они
    /// живут парой, и очередь, из которой убрали только блоки, растёт весь
    /// сеанс — у долгого просмотра это десятки тысяч мёртвых ключей.
    order: std::collections::VecDeque<(u64, u64)>,
    bytes: u64,
}

impl Blocks {
    fn claim(&self) -> u64 {
        self.next_owner.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    }

    fn get(&self, owner: u64, index: u64) -> Option<Arc<[u8]>> {
        self.pool.lock().unwrap().blocks.get(&(owner, index)).cloned()
    }

    /// Сколько блоков пул вытеснил за сеанс.
    fn evicted(&self) -> u64 {
        self.evicted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Кладёт блок, вытесняя старые. Если блок уже есть (два читателя
    /// запросили его одновременно), возвращается лежащий: учёт байт должен
    /// совпадать с содержимым, иначе потолок поплывёт.
    ///
    /// Вторым ответом — лёг ли блок или уже лежал. Различать это нужно счёту:
    /// иначе «привезено больше, чем блоков у файла» значило бы и
    /// перечитывание вытесненного, и наложившиеся разгоны двух чтений, а
    /// читают эту строку как первое.
    fn insert(&self, owner: u64, index: u64, data: Arc<[u8]>) -> (Arc<[u8]>, bool) {
        let mut pool = self.pool.lock().unwrap();
        if let Some(present) = pool.blocks.get(&(owner, index)) {
            return (present.clone(), false);
        }
        while pool.bytes + data.len() as u64 > POOL_LIMIT {
            let Some(oldest) = pool.order.pop_front() else { break };
            if let Some(dropped) = pool.blocks.remove(&oldest) {
                pool.bytes -= dropped.len() as u64;
                self.evicted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        pool.bytes += data.len() as u64;
        pool.order.push_back((owner, index));
        pool.blocks.insert((owner, index), data.clone());
        (data, true)
    }

    /// Ресурс закрыт: его блоки не переживут его — читать их больше некому,
    /// а место они держат общее.
    fn release(&self, owner: u64) {
        let mut pool = self.pool.lock().unwrap();
        let mut freed = 0;
        pool.blocks.retain(|&(who, _), data| {
            let mine = who == owner;
            if mine {
                freed += data.len() as u64;
            }
            !mine
        });
        pool.bytes -= freed;
        // Очередь вытеснения — тем же проходом. Расходуется она только под
        // давлением на потолок, а после освобождения места пул заведомо не
        // полон: ключи закрытых ресурсов лежали бы в ней до конца сеанса, и
        // сеанс просмотра, открывающий сотни удалённых растров, накопил бы
        // сотни тысяч ключей, которых ни один бюджет не считает.
        pool.order.retain(|&(who, _)| who != owner);
    }
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

/// Пора ли отчитываться о чтении: пороги `REPORT_STEP` считаются от нуля, и
/// нулевой — тоже порог, поэтому первый же доставленный диапазон даёт строку.
///
/// Отдельной функцией, а не условием внутри отчёта: всё правило — это она, и
/// проверяется оно без сети. `fetch_max`, а не обмен: читателей у ресурса
/// бывает несколько (см. `Blocks::insert`), и отставший вернул бы счёт назад,
/// заставив отчитаться о том же пороге снова.
fn due(fetched: u64, reported: &std::sync::atomic::AtomicU64) -> bool {
    let passed = fetched / REPORT_STEP + 1;
    passed > reported.fetch_max(passed, std::sync::atomic::Ordering::Relaxed)
}

/// Чем кончилась одна попытка сходить в сеть.
///
/// Названными исходами, а не одной ошибкой: повторять имеет смысл ровно
/// оборвавшееся. На отказ сервера ответ будет тот же самый, сколько ни
/// спрашивай, и три попытки с паузами стоили бы секунды на каждом блоке —
/// а истёкшая подпись сделала бы такими все блоки до единого.
enum Attempt {
    /// Оборвалось: соединение, тело ответа, шлюз. Проходит само.
    Broken(anyhow::Error),
    /// Отказано: не тот статус, чужой адрес, истёкшая подпись.
    Refused(anyhow::Error),
}

/// Отказ идти в сеть, когда хост гасится: таймеры рантайма разбирают первыми, и
/// запрос, начатый после этого, паникует внутри реквеста. Отказом, а не
/// обрывом, — повторять тут нечего и некому: результата этого чтения уже никто
/// не ждёт (см. `veldmap_host_core::shutting_down`).
fn shutting_down() -> Result<(), Attempt> {
    match veldmap_host_util::shutting_down() {
        true => Err(Attempt::Refused(anyhow::anyhow!("хост завершается"))),
        false => Ok(()),
    }
}

/// Как читать неудачный статус: перегруженный шлюз и «слишком часто» проходят
/// сами, всё прочее — отказ. Различает их только код: тело у них одинаково
/// пустое.
fn refusal_or_hiccup(status: reqwest::StatusCode, error: anyhow::Error) -> Attempt {
    match status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        true => Attempt::Broken(error),
        false => Attempt::Refused(error),
    }
}

/// Сколько ждать перед следующей попыткой. `None` — следующей не будет.
///
/// Отдельной функцией, а не условием внутри цикла: всё правило повторов —
/// это она, и проверяется оно без сети и без ожидания.
fn again(attempt: u32, outcome: &Attempt) -> Option<std::time::Duration> {
    match outcome {
        Attempt::Refused(_) => None,
        Attempt::Broken(_) if attempt >= ATTEMPTS => None,
        Attempt::Broken(_) => Some(RETRY_PAUSE * 2u32.pow(attempt - 1)),
    }
}

/// Повторять, пока сбой из тех, что проходят сами.
///
/// `what` — что именно повторяется, для лога: без него в нём остаются
/// одинаковые строки, по которым не видно, один блок переспрашивают или все
/// подряд.
///
/// Пауза — сон самого потока, а не таймер рантайма, хотя запросы вокруг неё
/// асинхронные. Чтение идёт с blocking-пула, поток на нём для того и заведён,
/// чтобы его занимать, — а таймер к моменту паузы может быть уже разобран:
/// выход из приложения не ждёт чтений, и `sleep` на гасящемся рантайме
/// паникует.
fn with_retries<T>(what: &str, mut once: impl FnMut() -> Result<T, Attempt>) -> anyhow::Result<T> {
    let mut attempt = 1;
    loop {
        let outcome = match once() {
            Ok(value) => return Ok(value),
            Err(outcome) => outcome,
        };
        let Some(pause) = again(attempt, &outcome) else {
            let (Attempt::Broken(error) | Attempt::Refused(error)) = outcome;
            return Err(error);
        };
        log::warn!(target: "network", "{}: попытка {} из {} сорвалась, повтор через {} мс: {}",
                   what, attempt, ATTEMPTS, pause.as_millis(),
                   match &outcome { Attempt::Broken(e) | Attempt::Refused(e) => e });
        std::thread::sleep(pause);
        attempt += 1;
    }
}

impl HttpRange {
    /// Пробный запрос первого байта: заодно проверяет, что сервер понимает
    /// Range, и узнаёт полный размер из Content-Range.
    fn open(url: &str, headers: HashMap<String, String>, blocks: Arc<Blocks>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        let len = with_retries(&format!("открытие {}", url), || {
            shutting_down()?;
            let response = runtime
                .block_on(super::http::get(url, &headers, Some((0, 1))).send())
                .map_err(|e| Attempt::Broken(e.into()))?;

            // Range здесь ни при чём, если ответ вообще не про содержимое: 404 —
            // это неверный адрес, 401/403 — просроченная или чужая подпись. Валить
            // всё в «сервер не поддерживает Range» значило бы уводить от причины;
            // отсутствие поддержки — это именно 200 вместо 206.
            let status = response.status();
            if status != reqwest::StatusCode::PARTIAL_CONTENT {
                if status == reqwest::StatusCode::OK {
                    return Err(Attempt::Refused(anyhow::anyhow!(
                        "сервер не поддерживает Range: на запрос диапазона ответил целым файлом (HTTP 200)"
                    )));
                }
                return Err(refusal_or_hiccup(status, anyhow::anyhow!(
                    "удалённый ресурс не открыт: HTTP {} на {}", status, url
                )));
            }
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next()?.parse::<u64>().ok())
                .ok_or_else(|| {
                    Attempt::Refused(anyhow::anyhow!("сервер не сообщил размер файла (Content-Range)"))
                })
        })?;

        Ok(Self {
            url: url.to_string(),
            headers,
            len,
            runtime,
            owner: blocks.claim(),
            blocks,
            readahead: Mutex::new(Readahead::default()),
            fetched: std::sync::atomic::AtomicU64::new(0),
            requests: std::sync::atomic::AtomicU64::new(0),
            hits: std::sync::atomic::AtomicU64::new(0),
            blocks_in: std::sync::atomic::AtomicU64::new(0),
            id: std::sync::atomic::AtomicU64::new(0),
            reported: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Носителю сообщают его номер в реестре: до этого он о себе знает только
    /// адрес, а читатели зовут его номером.
    fn answers_to(&self, id: u64) {
        self.id.store(id, std::sync::atomic::Ordering::Relaxed);
    }

    /// Как ресурс зовётся в логе: номер в реестре и хвост адреса. Номер —
    /// потому что имя не различает (квиклук у каждого продукта CDSE зовётся
    /// `quick-look.png`) и потому что этим же номером ресурс зовут открытие и
    /// читатели; хвост — потому что по одному номеру не догадаться. Целиком
    /// адрес длиной со строку подписи и в строку отчёта не лезет.
    fn name(&self) -> String {
        let path = self.url.split('?').next().unwrap_or(&self.url);
        let tail = path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path);
        format!("ресурс {} ({})", self.id.load(std::sync::atomic::Ordering::Relaxed), tail)
    }

    /// Строка о чтении ресурса: по порогу `REPORT_STEP` и обязательно на
    /// закрытии, иначе у долгого чтения не было бы итога.
    ///
    /// Отвечает на три вопроса: сколько байт ресурса доставлено, какой длины
    /// запросы (то есть разогналось ли упреждающее чтение) и не перечитывает
    /// ли ресурс сам себя из-за потолка пула — последнее видно по тому, что
    /// привезённых блоков больше, чем их у файла.
    ///
    /// Первая строка ресурса приходит после первого же запроса, и длина
    /// запроса в ней всегда одна: разгон начинается с блока (см.
    /// [`Readahead`]). Читать её надо как «чтение началось», а не как итог.
    fn report(&self, closing: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        let fetched = self.fetched.load(Relaxed);
        if !closing && !due(fetched, &self.reported) {
            return;
        }
        let (requests, hits) = (self.requests.load(Relaxed), self.hits.load(Relaxed));
        let (blocks_in, blocks) = (self.blocks_in.load(Relaxed), self.len.div_ceil(BLOCK));
        let share = if self.len > 0 { fetched * 100 / self.len } else { 0 };
        let per_request = match requests {
            0 => 0,
            _ => fetched / requests / 1024,
        };
        // Мегабайты дробью, а не целыми: у квиклука на 270 КБ целое давало бы
        // «0 из 0», а он и есть тот случай, ради которого отчёт идёт с первого
        // же запроса.
        let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        log::debug!(target: "network::perf",
                    "{}{}: доставлено {:.1} из {:.1} МиБ ({}%), запросов {} по {} КиБ, \
                     попаданий {}; блоков привезено {} из {}, вытеснено пулом {}",
                    self.name(), if closing { ", закрыт" } else { "" },
                    mib(fetched), mib(self.len), share, requests, per_request, hits,
                    blocks_in, blocks, self.blocks.evicted());
    }

    /// Блок из кэша или из сети. Промах тянет не один блок, а столько, сколько
    /// назначил разгон, — и одним запросом: цена запроса не зависит от того,
    /// сколько в нём байт (см. [`Readahead`]).
    fn block(&self, index: u64) -> anyhow::Result<Arc<[u8]>> {
        if let Some(data) = self.blocks.get(self.owner, index) {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(data);
        }
        let run = self.readahead.lock().unwrap().plan(index, self.len.div_ceil(BLOCK));

        let from = index * BLOCK;
        let to = (from + run * BLOCK).min(self.len);
        let expected = to - from;

        // Повторяется запрос вместе с чтением тела: рвётся и то, и другое, а
        // снаружи обрыв одинаково выглядит как «диапазон не прочитан».
        let data = with_retries(
            &format!("чтение диапазона {}..{} ({})", from, to, self.url),
            || {
                shutting_down()?;
                let response = self
                    .runtime
                    .block_on(super::http::get(&self.url, &self.headers, Some((from, to))).send())
                    .map_err(|e| Attempt::Broken(e.into()))?;

                // Только 206: ответ 200 означал бы, что Range проигнорирован и
                // пришёл весь файл — принять его за блок значило бы сдвинуть все
                // смещения.
                let status = response.status();
                if status != reqwest::StatusCode::PARTIAL_CONTENT {
                    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                        return Err(Attempt::Refused(anyhow::anyhow!(
                            "доступ к удалённому ресурсу больше не действителен (HTTP {}): \
                             заголовки авторизации выданы при открытии и могли истечь", status)));
                    }
                    return Err(refusal_or_hiccup(status, anyhow::anyhow!(
                        "чтение диапазона {}..{}: HTTP {}", from, to, status)));
                }

                let data = self
                    .runtime
                    .block_on(response.bytes())
                    .map_err(|e| Attempt::Broken(e.into()))?;
                // Короткий ответ — это обрыв, а не конец файла: длину мы знаем из
                // Content-Range и запросили ровно столько, сколько есть.
                if data.len() as u64 != expected {
                    return Err(Attempt::Broken(anyhow::anyhow!(
                        "получено {} байт вместо {}", data.len(), expected)));
                }
                Ok(data)
            },
        )?;
        self.fetched.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Пришедшее раскладывается по блокам целиком: упреждающая часть за это
        // и заплачена, а выбросить её значило бы перечитать её же следующим
        // окном читателя.
        let mut wanted = None;
        for (step, chunk) in data.chunks(BLOCK as usize).enumerate() {
            let (stored, fresh) = self.blocks.insert(self.owner, index + step as u64, Arc::from(chunk));
            if fresh {
                self.blocks_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if step == 0 {
                wanted = Some(stored);
            }
        }
        // После раскладки, а не до неё: иначе счёт блоков отстаёт от того
        // самого запроса, о котором отчитываются.
        self.report(false);
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

    /// Переспрашивается только то, что проходит само. Отказ повторять нельзя
    /// не из экономии: истёкшая подпись отказывает одинаково всем блокам, и
    /// пауза перед каждым растянула бы одно сообщение на минуты.
    #[test]
    fn обрыв_переспрашивают_отказ_нет() {
        let broken = Attempt::Broken(anyhow::anyhow!("тело ответа оборвалось"));
        let refused = Attempt::Refused(anyhow::anyhow!("HTTP 403"));

        assert_eq!(again(1, &broken), Some(RETRY_PAUSE));
        assert_eq!(again(2, &broken), Some(RETRY_PAUSE * 2), "пауза растёт");
        assert_eq!(again(ATTEMPTS, &broken), None, "попытки кончились");
        assert_eq!(again(1, &refused), None, "от повтора подпись не оживёт");
    }

    /// Осечку шлюза от отказа по существу отличает только код: тело у них
    /// одинаково пустое, а повторять надо ровно первое.
    #[test]
    fn чужой_статус_разбирается_на_осечку_и_отказ() {
        let hiccup = |code: u16| {
            let status = reqwest::StatusCode::from_u16(code).expect("код статуса");
            matches!(refusal_or_hiccup(status, anyhow::anyhow!("")), Attempt::Broken(_))
        };
        assert!(hiccup(502), "шлюз перегружен — пройдёт само");
        assert!(hiccup(429), "слишком часто — тем более");
        assert!(!hiccup(403), "подпись не станет действительной от повтора");
        assert!(!hiccup(404), "и адрес не появится");
    }

    fn block() -> Arc<[u8]> {
        Arc::from(vec![0u8; BLOCK as usize])
    }

    /// Первый доставленный диапазон отчитывается всегда, дальше — раз на
    /// порог. Первая строка нужна отдельным правилом: у ресурса мельче порога
    /// других не будет до самого закрытия, а закрытия у наложения не бывает.
    #[test]
    fn первый_диапазон_отчитывается_дальше_раз_на_порог() {
        let reported = std::sync::atomic::AtomicU64::new(0);

        assert!(due(0, &reported), "первый диапазон, даже пустой");
        assert!(!due(1, &reported), "тот же порог молчит");
        assert!(!due(REPORT_STEP - 1, &reported), "и до самого края");
        assert!(due(REPORT_STEP, &reported), "порог перейдён");
        assert!(!due(REPORT_STEP + 1, &reported));
        assert!(due(REPORT_STEP * 3, &reported), "через два порога — одна строка, не две");

        // Отставший читатель порог не откатывает: иначе следующий же
        // догнавший отчитался бы о том, о чём уже отчитались.
        assert!(!due(REPORT_STEP * 2, &reported), "отставший молчит");
        assert!(!due(REPORT_STEP * 3, &reported), "и порог за собой не сбросил");
    }

    /// Потолок общий: сколько бы ресурсов ни было открыто, вместе они держат
    /// не больше, чем один. Ради этого пул и заведён — прежний потолок «на
    /// ресурс» умножался на их число.
    #[test]
    fn pool_is_capped_across_resources_not_per_resource() {
        let pool = Blocks::default();
        let per_resource = POOL_LIMIT / BLOCK / 4;

        let owners: Vec<u64> = (0..8).map(|_| pool.claim()).collect();
        for &owner in &owners {
            for index in 0..per_resource {
                pool.insert(owner, index, block());
            }
        }

        assert!(pool.pool.lock().unwrap().bytes <= POOL_LIMIT, "восемь читателей вместе не выше потолка");
    }

    /// Вытесняется старейшее в пуле, а не старейшее своё: иначе активный
    /// читатель выбрасывал бы то, что сейчас же и перечитает, пока сосед
    /// держит нетронутое.
    #[test]
    fn oldest_in_the_pool_goes_first_whoever_owns_it() {
        let pool = Blocks::default();
        let (old, fresh) = (pool.claim(), pool.claim());
        let fits = POOL_LIMIT / BLOCK;

        pool.insert(old, 0, block());
        for index in 0..fits {
            pool.insert(fresh, index, block());
        }

        assert!(pool.get(old, 0).is_none(), "первым вышел старейший, хоть он и чужой");
        assert!(pool.get(fresh, fits - 1).is_some(), "только что положенное на месте");
    }

    /// Закрытый ресурс уносит свои блоки и своё место: читать их больше
    /// некому, а место они держат общее.
    #[test]
    fn closing_a_resource_returns_its_bytes() {
        let pool = Blocks::default();
        let (leaving, staying) = (pool.claim(), pool.claim());
        pool.insert(leaving, 0, block());
        pool.insert(leaving, 1, block());
        pool.insert(staying, 0, block());

        pool.release(leaving);

        assert_eq!(pool.pool.lock().unwrap().bytes, BLOCK, "осталось место одного блока");
        assert!(pool.get(leaving, 0).is_none());
        assert!(pool.get(staying, 0).is_some(), "чужие блоки не тронуты");
    }

    /// Вытеснения считаются, и считается только они. Число это единственное,
    /// по чему видно, что потолка не хватает: выброшенный блок перечитывается
    /// молча, а по одному объёму чтения перечитывание от чтения не отличить.
    /// Закрытие ресурса вытеснением не является — блоки уносит он сам, а не
    /// давление на потолок.
    #[test]
    fn evictions_are_counted_and_closing_is_not_one() {
        let pool = Blocks::default();
        let owner = pool.claim();
        let fits = POOL_LIMIT / BLOCK;

        for index in 0..fits {
            pool.insert(owner, index, block());
        }
        assert_eq!(pool.evicted(), 0, "пока помещается — вытеснять нечего");

        pool.insert(owner, fits, block());
        assert_eq!(pool.evicted(), 1, "лишний блок выбросил ровно один");

        assert!(!pool.insert(owner, fits, block()).1, "уже лежащий блок не кладётся заново");
        assert_eq!(pool.evicted(), 1, "и никого не выбрасывает");

        pool.release(owner);
        assert_eq!(pool.evicted(), 1, "закрытие ресурса — не вытеснение");
    }

    /// Ключ владения не переиспользуется: закрытый ресурс и открытый следом —
    /// разные файлы, и блок с тем же номером у них разный.
    #[test]
    fn owner_keys_are_not_reused() {
        let pool = Blocks::default();
        let first = pool.claim();
        pool.insert(first, 0, block());
        pool.release(first);

        let second = pool.claim();
        assert_ne!(first, second);
        assert!(pool.get(second, 0).is_none(), "новый ресурс начинает с пустого");
    }
}
