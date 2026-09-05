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
//! ресурс живёт ровно столько, сколько они действительны. Для подписи SigV4 в
//! заголовках это четверть часа — достаточно, чтобы открыть и декодировать, но
//! не для слоя, который держат на шаре часами; отказ 401/403 посреди чтения
//! не переспрашивается, и переподписи пока нет (ADR 0005).

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

/// Сколько серий блоков prefetch тянет разом. Запрос стоит около полусекунды
/// задержки при любой длине, а точечное чтение заказывает десятки серий и
/// платило бы её за каждую по очереди; четыре соединения делят задержку, а
/// пул соединений клиента (`http::client`) держит их между вызовами.
const IN_FLIGHT: usize = 4;

/// Сколько блоков prefetch привозит за один заказ. Привезённое ложится в
/// общий пул, и больше половины его значило бы вытеснять своё же до того, как
/// его прочтут; лишнее отбрасывается с хвоста и приедет обычным чтением.
const PREFETCH_CAP: u64 = POOL_LIMIT / 2;

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
                log::info!(target: "network", "Открыт удалённый ресурс {} ({} байт): {}", id, len, without_query(&req.url));
                opened_handle(id, len)
            }
            Err(e) => {
                // Ошибка уходит событием заказчику, но на экране её увидит
                // только тот, кто в этот момент смотрит на превью — в логе она
                // нужна независимо от этого.
                log::warn!(target: "network", "Удалённый ресурс {} не открылся: {}", without_query(&req.url), e);
                opened(Err(e.to_string()))
            }
        };
        bus::emit::on_open_result(&*ctx.publisher, &result, &correlation);
    });
}

/// Адрес без строки запроса — для журнала и для ключа владения: строка запроса
/// не часть тождества объекта, а у чужого сервера в ней может лежать ключ или
/// подпись, и журналу такое не принадлежит.
fn without_query(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Поход в сеть за диапазоном `[from, to)`: тело ответа либо исход попытки.
///
/// Замыканием, а не методом, — это названное исключение из «никаких трейтов
/// для тестов»: у хоста нет фальшивки SDK, а сборка ответа из блоков, разгон
/// и пул обязаны проверяться без сети. Всё, что знает про HTTP, — статусы,
/// заголовки, рантайм — живёт внутри замыкания, которое собирает [`HttpRange::open`].
type Fetch = Box<dyn Fn(u64, u64) -> Result<bytes::Bytes, Attempt> + Send + Sync>;

/// Носитель поверх HTTP: блоки тянутся по требованию и кэшируются.
struct HttpRange {
    /// Адрес — только для журнала: ходит в сеть [`HttpRange::fetch`].
    url: String,
    len: u64,
    fetch: Fetch,
    /// Общий на все ресурсы пул блоков и ключ владения в нём; там же и
    /// разгон — он принадлежит объекту, а не открытию (см. [`Readahead`]).
    blocks: Arc<Blocks>,
    owner: u64,
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
                   fetched, self.len, share, without_query(&self.url));
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

/// Чем один и тот же объект хранилища узнаётся в двух разных открытиях.
///
/// Адрес без запроса: объект — это путь, а строка запроса у чужого сервера
/// бывает подписью или ключом, то есть разной у двух открытий одних и тех же
/// байт. Длина и валидатор — чтобы перевыложенный объект не
/// достался в наследство прежнему: путь у него тот же самый, и по одному пути
/// новые байты легли бы под старый ключ.
///
/// Валидатора у сервера может и не быть; тогда общего ключа не заводят вовсе
/// (см. [`Blocks::claim_for`]) — угадывать тождество по пути и длине нельзя,
/// а ошибка здесь стоит подменённой середины файла.
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Identity {
    path: String,
    len: u64,
    validator: String,
}

#[derive(Default)]
struct Pool {
    blocks: HashMap<(u64, u64), Arc<[u8]>>,
    /// Объект → его ключ владения и сколько ресурсов сейчас его читают.
    ///
    /// Счётчик обязателен: блоки не должны переживать последнего читателя,
    /// иначе вытеснение начнёт выбрасывать чужое живое ради мёртвого.
    shared: HashMap<Identity, (u64, u32)>,
    /// Разгон чтения по ключу владения: превью и слой шара читают один файл,
    /// и проход, начатый одним, продолжает другой. Уходит с блоками.
    readahead: HashMap<u64, Readahead>,
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

    /// Ключ владения для этого объекта: у второго открытия того же объекта он
    /// тот же, и привезённые блоки достаются ему готовыми.
    ///
    /// Ради этого всё и заведено: одно открытие снимка живёт минутами и тянет
    /// сотни блоков, а открытий у него бывает несколько — превью, наложение на
    /// шар, второй слой того же файла, — и каждое ходило бы в сеть за тем же
    /// самым.
    ///
    /// `None` — тождества не установить (сервер не прислал ни ETag, ни
    /// Last-Modified): тогда ресурс читает сам за себя, как и раньше. Угадывать
    /// тождество нельзя — по одному пути и длине два разных объекта склеились
    /// бы молча, серединой файла.
    fn claim_for(&self, identity: Option<Identity>) -> u64 {
        let Some(identity) = identity else { return self.claim() };
        let mut pool = self.pool.lock().unwrap();
        if let Some((owner, readers)) = pool.shared.get_mut(&identity) {
            *readers += 1;
            return *owner;
        }
        let owner = self.next_owner.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        pool.shared.insert(identity, (owner, 1));
        owner
    }

    fn get(&self, owner: u64, index: u64) -> Option<Arc<[u8]>> {
        self.pool.lock().unwrap().blocks.get(&(owner, index)).cloned()
    }

    fn has(&self, owner: u64, index: u64) -> bool {
        self.pool.lock().unwrap().blocks.contains_key(&(owner, index))
    }

    /// Сколько блоков брать на промахе `index` объекта `owner` (см.
    /// [`Readahead::plan`]).
    fn plan(&self, owner: u64, index: u64, total: u64) -> u64 {
        self.pool.lock().unwrap().readahead.entry(owner).or_default().plan(index, total)
    }

    /// Читатель объекта `owner` дочитал блок `index` до конца (см.
    /// [`Readahead::consumed`]).
    fn consumed(&self, owner: u64, index: u64) {
        self.pool.lock().unwrap().readahead.entry(owner).or_default().consumed(index);
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
    ///
    /// Кроме одного случая: тот же объект читает кто-то ещё. Ключ у них общий,
    /// и унесённые блоки пришлось бы везти по проводу заново — ровно тому
    /// читателю, который никуда не уходил.
    fn release(&self, owner: u64) {
        let mut pool = self.pool.lock().unwrap();
        // Ключ ищется перебором: объектов в карте столько, сколько ресурсов
        // открыто, то есть единицы, а держать вторую карту «ключ → объект»
        // значило бы завести две правды об одном.
        if let Some((identity, (_, readers))) =
            pool.shared.iter_mut().find(|(_, (who, _))| *who == owner)
        {
            *readers -= 1;
            if *readers > 0 {
                return;
            }
            let identity = identity.clone();
            pool.shared.remove(&identity);
        }
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
        pool.readahead.remove(&owner);
    }
}

/// Разгон последовательного чтения: сколько блоков брать одним запросом.
///
/// Мелкий блок хорош случайному чтению и плох проходу по файлу — на каждый
/// блок пришлась бы задержка запроса. Поэтому блок остаётся мелким, а проход
/// узнаётся по двум приметам разом: промах пришёлся ровно туда, где кончился
/// прошлый запрос, и читатель дочитал прошлый запрос до конца. Одной первой
/// мало: заголовки тайл-партов JPEG 2000 лежат через два блока, цепочка проб
/// по 64 КиБ то и дело попадает ровно за конец прошлого запроса, а
/// разогнавшись, везёт мегабайты того, чего никто не спросит — 84 % файла
/// ради самого грубого уровня (замер в ADR 0004). Проход же дочитывает каждый
/// блок: тогда кусок удваивается, и уже через несколько промахов запросы идут
/// мегабайтами. Скачок в сторону и недочитанный запрос сбрасывают разгон —
/// там снова дешевле взять один блок.
///
/// Состояние — у объекта, а не у открытия (см. [`Pool::readahead`]): превью
/// и слой шара читают один файл, и проход, начатый одним, продолжает другой.
#[derive(Default)]
struct Readahead {
    /// Блок сразу за концом прошлого запроса — на нём проход и опознаётся.
    next: u64,
    /// Сколько блоков взял прошлый запрос.
    run: u64,
    /// Читатель дочитал последний блок прошлого запроса до конца.
    deep: bool,
}

impl Readahead {
    /// Сколько блоков брать, начиная с промаха на `index`. `total` — всего
    /// блоков у ресурса: за конец файла упреждать нечего.
    fn plan(&mut self, index: u64, total: u64) -> u64 {
        let run = match index == self.next && self.deep {
            true => (self.run * 2).min(READAHEAD / BLOCK),
            false => 1,
        };
        let run = run.clamp(1, total.saturating_sub(index).max(1));
        self.next = index + run;
        self.run = run;
        self.deep = false;
        run
    }

    /// Читатель дошёл до конца блока `index`. Последний блок прошлого запроса,
    /// дочитанный до конца, и есть примета прохода: следующий промах за ним —
    /// его продолжение, а не очередная проба.
    fn consumed(&mut self, index: u64) {
        if index + 1 == self.next {
            self.deep = true;
        }
    }
}

/// Серии подряд идущих блоков из отсортированных без повторов индексов:
/// (первый, сколько). Серия не длиннее `max_run` — один запрос на мегабайты
/// держит соединение дольше, чем стоит, и это тот же предел, что у разгона;
/// всего не больше `cap` блоков — лишнее отбрасывается с хвоста.
fn runs_of(indices: &[u64], max_run: u64, cap: u64) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    let mut taken = 0u64;
    for &index in indices {
        if taken >= cap {
            break;
        }
        match runs.last_mut() {
            Some((first, count)) if *first + *count == index && *count < max_run => *count += 1,
            _ => runs.push((index, 1)),
        }
        taken += 1;
    }
    runs
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

/// Исход пробного запроса по статусу. Range здесь ни при чём, если ответ
/// вообще не про содержимое: 404 — неверный адрес, 401/403 — просроченная или
/// чужая подпись, и валить всё в «не поддерживает Range» значило бы уводить от
/// причины; отсутствие поддержки — это именно 200 вместо 206.
fn probed(status: reqwest::StatusCode, url: &str) -> Result<(), Attempt> {
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        return Ok(());
    }
    if status == reqwest::StatusCode::OK {
        return Err(Attempt::Refused(anyhow::anyhow!(
            "сервер не поддерживает Range: на запрос диапазона ответил целым файлом (HTTP 200)"
        )));
    }
    Err(refusal_or_hiccup(status, anyhow::anyhow!("удалённый ресурс не открыт: HTTP {} на {}", status, url)))
}

/// Исход чтения диапазона по статусу: только 206 — ответ 200 означал бы, что
/// Range проигнорирован и пришёл весь файл, а принять его за блок значило бы
/// сдвинуть все смещения. 401/403 посреди чтения — истёкшая подпись.
fn ranged(status: reqwest::StatusCode, from: u64, to: u64) -> Result<(), Attempt> {
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        return Ok(());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(Attempt::Refused(anyhow::anyhow!(
            "доступ к удалённому ресурсу больше не действителен (HTTP {}): \
             подпись выдана при открытии и могла истечь", status)));
    }
    Err(refusal_or_hiccup(status, anyhow::anyhow!("чтение диапазона {}..{}: HTTP {}", from, to, status)))
}

/// Полная длина объекта из `Content-Range: bytes 0-1/12345`; `None` — сервер
/// её не назвал.
fn full_length(content_range: &str) -> Option<u64> {
    content_range.rsplit('/').next()?.trim().parse::<u64>().ok()
}

/// Короткий ответ — обрыв, а не конец файла: длина известна из Content-Range,
/// и запрошено ровно столько, сколько есть.
fn delivered(got: u64, expected: u64) -> Result<(), Attempt> {
    match got == expected {
        true => Ok(()),
        false => Err(Attempt::Broken(anyhow::anyhow!("получено {} байт вместо {}", got, expected))),
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

/// Поход в сеть за диапазоном адреса `url` с заголовками `headers` — то, чем
/// живёт [`HttpRange`].
///
/// Рантайм нужен только походу: `read_at` зовётся хостом с blocking-пула,
/// prefetch — со своих потоков, и асинхронный запрос надо кому-то отдать.
/// Запрос собирается ВНУТРИ `block_on`, а не доводом к нему: клиент заводит
/// таймер `read_timeout` (`http::client`) уже при сборке future, таймеру нужен
/// контекст рантайма, а у потока prefetch его нет — собранный снаружи запрос
/// падает «there is no reactor running», и с `panic = "abort"` это уносит весь
/// хост. Держит это правило тест `запрос_собирается_внутри_block_on`.
fn fetcher(runtime: tokio::runtime::Handle, url: String, headers: HashMap<String, String>) -> Fetch {
    Box::new(move |from, to| {
        runtime.block_on(async {
            let response = super::http::get(&url, &headers, Some((from, to)))
                .send()
                .await
                .map_err(|e| Attempt::Broken(e.into()))?;
            ranged(response.status(), from, to)?;
            response.bytes().await.map_err(|e| Attempt::Broken(e.into()))
        })
    })
}

impl HttpRange {
    /// Пробный запрос первого байта: заодно проверяет, что сервер понимает
    /// Range, и узнаёт полный размер из Content-Range.
    fn open(url: &str, headers: HashMap<String, String>, blocks: Arc<Blocks>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        let mut validator = String::new();
        let len = with_retries(&format!("открытие {}", without_query(url)), || {
            shutting_down()?;
            let response = runtime
                .block_on(async { super::http::get(url, &headers, Some((0, 1))).send().await })
                .map_err(|e| Attempt::Broken(e.into()))?;
            probed(response.status(), without_query(url))?;
            // Валидатор берётся тем же пробным запросом: второго повода
            // ходить за ним нет, а без него блоки этого объекта не разделить
            // с его же вторым открытием (см. [`Identity`]). ETag старше даты:
            // он про содержимое, а секундная дата у перевыложенного объекта
            // вполне совпадает с прежней.
            validator = response
                .headers()
                .get(reqwest::header::ETAG)
                .or_else(|| response.headers().get(reqwest::header::LAST_MODIFIED))
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(full_length)
                .ok_or_else(|| {
                    Attempt::Refused(anyhow::anyhow!("сервер не сообщил размер файла (Content-Range)"))
                })
        })?;

        let identity = (!validator.is_empty()).then(|| Identity {
            path: url.split('?').next().unwrap_or(url).to_string(),
            len,
            validator,
        });

        Ok(Self::over(url, len, blocks, identity, fetcher(runtime, url.to_string(), headers)))
    }

    /// Носитель над готовым походом в сеть — тем, что собрал [`fetcher`],
    /// либо тем, что подложил тест.
    fn over(url: &str, len: u64, blocks: Arc<Blocks>, identity: Option<Identity>, fetch: Fetch) -> Self {
        Self {
            url: url.to_string(),
            len,
            fetch,
            owner: blocks.claim_for(identity),
            blocks,
            fetched: std::sync::atomic::AtomicU64::new(0),
            requests: std::sync::atomic::AtomicU64::new(0),
            hits: std::sync::atomic::AtomicU64::new(0),
            blocks_in: std::sync::atomic::AtomicU64::new(0),
            id: std::sync::atomic::AtomicU64::new(0),
            reported: std::sync::atomic::AtomicU64::new(0),
        }
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
        let path = without_query(&self.url);
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
        let run = self.blocks.plan(self.owner, index, self.len.div_ceil(BLOCK));
        let data = self.fetch_run(index, run)?;
        let wanted = self.store(index, &data);
        // После раскладки, а не до неё: иначе счёт блоков отстаёт от того
        // самого запроса, о котором отчитываются.
        self.report(false);
        wanted.ok_or_else(|| anyhow::anyhow!("чтение диапазона с блока {}: пустой ответ", index))
    }

    /// Поход в сеть за `run` блоками с `index` — с повторами и счётом.
    fn fetch_run(&self, index: u64, run: u64) -> anyhow::Result<bytes::Bytes> {
        let from = index * BLOCK;
        let to = (from + run * BLOCK).min(self.len);
        let expected = to - from;

        // Повторяется запрос вместе с чтением тела: рвётся и то, и другое, а
        // снаружи обрыв одинаково выглядит как «диапазон не прочитан».
        let data = with_retries(
            &format!("чтение диапазона {}..{} ({})", from, to, without_query(&self.url)),
            || {
                shutting_down()?;
                let data = (self.fetch)(from, to)?;
                delivered(data.len() as u64, expected)?;
                Ok(data)
            },
        )?;
        self.fetched.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(data)
    }

    /// Пришедшее раскладывается по блокам целиком: упреждающая часть за это и
    /// заплачена, а выбросить её значило бы перечитать её же следующим окном
    /// читателя. Возвращает первый блок — тот, ради которого ходили.
    fn store(&self, index: u64, data: &[u8]) -> Option<Arc<[u8]>> {
        let mut first = None;
        for (step, chunk) in data.chunks(BLOCK as usize).enumerate() {
            let (stored, fresh) = self.blocks.insert(self.owner, index + step as u64, Arc::from(chunk));
            if fresh {
                self.blocks_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if step == 0 {
                first = Some(stored);
            }
        }
        first
    }

    /// Недостающие в пуле блоки под диапазонами — по порядку, без повторов.
    fn missing(&self, ranges: &[(u64, u64)]) -> Vec<u64> {
        let total = self.len.div_ceil(BLOCK);
        let mut indices: Vec<u64> = ranges
            .iter()
            .filter(|(_, len)| *len > 0)
            .flat_map(|&(offset, len)| {
                let last = (offset.saturating_add(len) - 1) / BLOCK;
                offset / BLOCK..=last.min(total.saturating_sub(1))
            })
            .collect();
        indices.sort_unstable();
        indices.dedup();
        indices.retain(|&index| !self.blocks.has(self.owner, index));
        indices
    }

    /// Привезти диапазоны наперёд: недостающие блоки — сериями подряд идущих
    /// ([`runs_of`]), по [`IN_FLIGHT`] запросов разом. Читатель назвал, что
    /// прочтёт, и угадывать проход по промахам незачем: разгон не трогается,
    /// а чтения вслед — попадания. Каждая серия — тот же поход, что и на
    /// промахе, со своими повторами; сорвавшаяся валит весь заказ, и читатель
    /// пойдёт за своим обычным чтением.
    ///
    /// Потоки, а не задачи рантайма: поход в сеть — синхронный `block_on`
    /// внутри [`Fetch`], и зовётся он с blocking-пула; вложить его в
    /// рантайм нельзя, а поток рядом — можно.
    fn fetch_ahead(&self, ranges: &[(u64, u64)]) -> anyhow::Result<()> {
        let runs = runs_of(&self.missing(ranges), READAHEAD / BLOCK, PREFETCH_CAP / BLOCK);
        if runs.is_empty() {
            return Ok(());
        }
        log::debug!(target: "network", "{}: prefetch {} диапазонов — {} серий, {} блоков",
                    self.name(), ranges.len(), runs.len(), runs.iter().map(|(_, count)| count).sum::<u64>());
        for batch in runs.chunks(IN_FLIGHT) {
            let fetched: Vec<anyhow::Result<(u64, bytes::Bytes)>> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|&(index, run)| scope.spawn(move || self.fetch_run(index, run).map(|data| (index, data))))
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or_else(|_| Err(anyhow::anyhow!("поток prefetch упал"))))
                    .collect()
            });
            // Привезённое ложится в пул раньше, чем разбирается отказ: серии
            // одной пачки независимы, и выброшенные ради соседней сорвавшейся
            // приехали бы снова — а счётчики их уже сосчитали.
            let mut failed = None;
            for outcome in fetched {
                match outcome {
                    Ok((index, data)) => {
                        self.store(index, &data);
                    }
                    Err(why) => {
                        failed.get_or_insert(why);
                    }
                }
            }
            if let Some(why) = failed {
                self.report(false);
                return Err(why);
            }
        }
        self.report(false);
        Ok(())
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
            let index = position / BLOCK;
            let block = self.block(index)?;
            let start = (position % BLOCK) as usize;
            let take = ((size - out.len() as u64) as usize).min(block.len() - start);
            out.extend_from_slice(&block[start..start + take]);
            position += take as u64;
            // Дочитанный до конца блок — примета прохода (см. [`Readahead`]).
            if start + take == block.len() {
                self.blocks.consumed(self.owner, index);
            }
        }
        Ok(out)
    }

    fn prefetch(&self, ranges: &[(u64, u64)]) -> anyhow::Result<()> {
        self.fetch_ahead(ranges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проход разгоняется, скачок в сторону — нет, и цепочка проб, попавших
    /// ровно за конец прошлого запроса, — тоже нет: проход дочитывает блоки
    /// до конца, проба — нет. Цена ошибки несимметрична: не разогнаться на
    /// проходе значит платить задержкой за каждые полмегабайта, а разогнаться
    /// на пробах — тянуть мегабайты ради заголовков.
    #[test]
    fn sequential_reads_speed_up_random_ones_do_not() {
        let total = 1024;
        let mut readahead = Readahead::default();

        // Первый промах — один блок: о проходе ещё ничего не известно.
        assert_eq!(readahead.plan(0, total), 1);
        // Дальше подряд и дочитывая — удвоение до потолка.
        let cap = READAHEAD / BLOCK;
        readahead.consumed(0);
        assert_eq!(readahead.plan(1, total), 2);
        readahead.consumed(2);
        assert_eq!(readahead.plan(3, total), 4);
        readahead.consumed(6);
        assert_eq!(readahead.plan(7, total), 8);
        readahead.consumed(14);
        assert_eq!(readahead.plan(15, total), cap);
        readahead.consumed(15 + cap - 1);
        assert_eq!(readahead.plan(15 + cap, total), cap, "выше потолка не растёт");

        // Скачок в сторону сбрасывает разгон.
        readahead.consumed(15 + 2 * cap - 1);
        assert_eq!(readahead.plan(500, total), 1);

        // Промах ровно за концом прошлого запроса, который не дочитали, — не
        // проход: так идут пробы тайл-партов JPEG 2000 через два блока.
        assert_eq!(readahead.plan(501, total), 1, "недочитанный запрос не разгоняет");
        readahead.consumed(500);
        assert_eq!(readahead.plan(502, total), 1, "дочитанным считается последний блок запроса, а не любой");
        readahead.consumed(502);
        assert_eq!(readahead.plan(503, total), 2);
    }

    /// Серии prefetch: подряд идущие блоки склеиваются, серия не длиннее
    /// предела, всего не больше потолка — лишнее отброшено с хвоста.
    #[test]
    fn серии_prefetch_склеиваются_и_упираются_в_пределы() {
        assert_eq!(runs_of(&[0, 1, 2, 5, 9, 10], 16, 100), vec![(0, 3), (5, 1), (9, 2)]);
        assert_eq!(runs_of(&[0, 1, 2, 3, 4], 2, 100), vec![(0, 2), (2, 2), (4, 1)], "серия не длиннее предела");
        assert_eq!(runs_of(&[0, 1, 2, 3, 4], 16, 3), vec![(0, 3)], "потолок отбрасывает хвост");
        assert_eq!(runs_of(&[7], 16, 0), Vec::<(u64, u64)>::new());
        assert!(runs_of(&[], 16, 100).is_empty());
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

    fn identity(path: &str, len: u64, validator: &str) -> Identity {
        Identity { path: path.to_string(), len, validator: validator.to_string() }
    }

    /// Два открытия одного и того же объекта читают один пул: превью снимка и
    /// его же наложение на шар открывают один путь порознь — а байты одни и те
    /// же.
    #[test]
    fn один_объект_читается_одним_ключом() {
        let pool = Blocks::default();
        let same = identity("s3://bucket/scene.tif", 4096, "\"abc\"");

        let first = pool.claim_for(Some(same.clone()));
        pool.insert(first, 0, block());

        let second = pool.claim_for(Some(same));
        assert_eq!(first, second, "второе открытие пошло бы в сеть за тем же самым");
        assert!(pool.get(second, 0).is_some(), "привезённое досталось не готовым");
    }

    /// А разные объекты — разными, чем бы они ни были похожи. Ошибка здесь
    /// стоит подменённой середины файла: длина и путь совпадают, а байты нет.
    #[test]
    fn разные_объекты_читаются_порознь() {
        let pool = Blocks::default();
        let path = "s3://bucket/scene.tif";

        // Перевыложенный объект: путь и длина те же, содержимое другое.
        let old = pool.claim_for(Some(identity(path, 4096, "\"abc\"")));
        let new = pool.claim_for(Some(identity(path, 4096, "\"def\"")));
        assert_ne!(old, new);

        // Другой объект по тому же пути другой длины.
        let longer = pool.claim_for(Some(identity(path, 8192, "\"abc\"")));
        assert_ne!(old, longer);

        // И объект, о котором сервер не сказал ничего: тождества не установить,
        // и угадывать его нельзя — каждое открытие читает само за себя.
        let (mute, mute_again) = (pool.claim_for(None), pool.claim_for(None));
        assert_ne!(mute, mute_again);
    }

    /// Уходит один из двух читателей — блоки остаются: они нужны тому, кто
    /// никуда не уходил, и увезённые пришлось бы везти по проводу заново.
    /// Уходит последний — уносит всё, как и всякий закрытый ресурс.
    #[test]
    fn блоки_живут_столько_же_сколько_последний_их_читатель() {
        let pool = Blocks::default();
        let same = identity("s3://bucket/scene.tif", 4096, "\"abc\"");
        let first = pool.claim_for(Some(same.clone()));
        let second = pool.claim_for(Some(same.clone()));
        pool.insert(first, 0, block());

        pool.release(first);
        assert!(pool.get(second, 0).is_some(), "блоки ушли с тем, кто их не держал один");
        assert_eq!(pool.pool.lock().unwrap().bytes, BLOCK);

        pool.release(second);
        assert_eq!(pool.pool.lock().unwrap().bytes, 0, "последний читатель не унёс своё");

        // И ключ объекта освободился вместе с ними: открытый следом начинает с
        // пустого, а не наследует чужую очередь вытеснения.
        let again = pool.claim_for(Some(same));
        assert!(pool.get(again, 0).is_none());
    }
}

#[cfg(test)]
mod reads {
    use super::*;

    /// Носитель над байтами в памяти: «сеть» — срез вектора, и каждый поход
    /// за диапазоном записывается.
    fn in_memory(bytes: Vec<u8>) -> (HttpRange, Arc<Mutex<Vec<(u64, u64)>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let journal = asked.clone();
        let len = bytes.len() as u64;
        let fetch: Fetch = Box::new(move |from, to| {
            journal.lock().unwrap().push((from, to));
            Ok(bytes::Bytes::copy_from_slice(&bytes[from as usize..to as usize]))
        });
        let source = HttpRange::over("http://host/path/file.tif?sig", len, Arc::new(Blocks::default()), None, fetch);
        (source, asked)
    }

    fn pattern(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Чтение через границу блоков собирается из двух блоков байт в байт, а
    /// хвост файла короче блока приезжает ровно такой длины, какая есть.
    #[test]
    fn read_at_crosses_block_boundaries_and_ends_short() {
        let bytes = pattern(2 * BLOCK + 1000);
        let (source, asked) = in_memory(bytes.clone());

        let across = source.read_at(BLOCK - 10, 20).unwrap();
        assert_eq!(across, bytes[(BLOCK - 10) as usize..(BLOCK + 10) as usize]);

        let tail = source.read_at(2 * BLOCK - 5, 1005).unwrap();
        assert_eq!(tail, bytes[(2 * BLOCK - 5) as usize..]);

        // Хвост приезжает разгоном второго запроса: тот обрезан ровно по
        // концу файла, за него не просят ни байта.
        let asked = asked.lock().unwrap();
        let len = 2 * BLOCK + 1000;
        assert!(asked.iter().any(|(_, to)| *to == len), "хвост просится ровно до конца: {asked:?}");
        assert!(asked.iter().all(|(from, to)| from % BLOCK == 0 && *to <= len), "запросы идут с границ блоков и не за конец: {asked:?}");
    }

    fn runs(asked: &Arc<Mutex<Vec<(u64, u64)>>>) -> Vec<u64> {
        asked.lock().unwrap().iter().map(|(from, to)| (to - from) / BLOCK).collect()
    }

    /// Последовательный проход окнами читателя разгоняется удвоением до
    /// потолка, прыжок в сторону сбрасывает разгон к одному блоку.
    #[test]
    fn a_sequential_pass_speeds_up_and_a_jump_resets() {
        let (source, asked) = in_memory(pattern(40 * BLOCK));
        let window = BLOCK / 2;
        for step in 0..(31 * BLOCK / window) {
            source.read_at(step * window, window).unwrap();
        }
        source.read_at(36 * BLOCK, 16).unwrap();
        assert_eq!(runs(&asked), vec![1, 2, 4, 8, 16, 1]);
    }

    /// Цепочка проб — по кусочку с начала каждого блока — не разгоняется,
    /// сколько бы промахов ни пришлось ровно за конец прошлого запроса: так
    /// читаются заголовки тайл-партов, и разгон на них вёз мегабайты впустую.
    #[test]
    fn a_chain_of_probes_stays_at_one_block() {
        let (source, asked) = in_memory(pattern(40 * BLOCK));
        for block in 0..12 {
            source.read_at(block * BLOCK, 16).unwrap();
        }
        assert_eq!(runs(&asked), vec![1; 12]);

        // И через блок — тоже.
        let (source, asked) = in_memory(pattern(40 * BLOCK));
        for block in (0..24).step_by(2) {
            source.read_at(block * BLOCK + 100, 64 * 1024).unwrap();
        }
        assert!(runs(&asked).iter().all(|run| *run == 1), "{:?}", runs(&asked));
    }

    /// Одно длинное чтение — тоже проход: блоки внутри него дочитываются до
    /// конца, и запросы растут, не дожидаясь следующего окна; последний
    /// запрос упреждает за край чтения — это и есть разгон.
    #[test]
    fn one_long_read_speeds_up_within_itself() {
        let (source, asked) = in_memory(pattern(40 * BLOCK));
        source.read_at(0, 12 * BLOCK).unwrap();
        assert_eq!(runs(&asked), vec![1, 2, 4, 8]);
    }

    /// Разгон принадлежит объекту: второе открытие того же файла продолжает
    /// проход первого, а не начинает разгон с одного блока.
    #[test]
    fn два_открытия_одного_объекта_делят_разгон() {
        let bytes = pattern(40 * BLOCK);
        let blocks = Arc::new(Blocks::default());
        let same = Identity { path: "http://host/f".to_string(), len: bytes.len() as u64, validator: "\"v\"".to_string() };
        let len = bytes.len() as u64;
        let opened = |journal: Arc<Mutex<Vec<(u64, u64)>>>| {
            let bytes = bytes.clone();
            let fetch: Fetch = Box::new(move |from, to| {
                journal.lock().unwrap().push((from, to));
                Ok(bytes::Bytes::copy_from_slice(&bytes[from as usize..to as usize]))
            });
            HttpRange::over("http://host/f?sig", len, blocks.clone(), Some(same.clone()), fetch)
        };
        let (first_asked, second_asked) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let first = opened(first_asked.clone());
        let second = opened(second_asked.clone());

        // Первое открытие дочитывает три блока — запросы 1, 2.
        first.read_at(0, 3 * BLOCK).unwrap();
        assert_eq!(runs(&first_asked), vec![1, 2]);
        // Второе продолжает с четвёртого — и получает удвоение, а не единицу.
        second.read_at(3 * BLOCK, BLOCK).unwrap();
        assert_eq!(runs(&second_asked), vec![4]);
    }

    /// Закрытие одного ресурса не сбивает разгон другого: у каждого объекта
    /// он свой, и уходит только со своими блоками.
    #[test]
    fn закрытие_чужого_ресурса_не_сбивает_разгон() {
        let blocks = Arc::new(Blocks::default());
        let bytes = pattern(40 * BLOCK);
        let len = bytes.len() as u64;
        let opened = |path: &str, journal: Arc<Mutex<Vec<(u64, u64)>>>| {
            let bytes = bytes.clone();
            let fetch: Fetch = Box::new(move |from, to| {
                journal.lock().unwrap().push((from, to));
                Ok(bytes::Bytes::copy_from_slice(&bytes[from as usize..to as usize]))
            });
            let identity = Identity { path: path.to_string(), len, validator: "\"v\"".to_string() };
            HttpRange::over(path, len, blocks.clone(), Some(identity), fetch)
        };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let staying = opened("http://host/a", asked.clone());
        let leaving = opened("http://host/b", Arc::new(Mutex::new(Vec::new())));

        staying.read_at(0, 3 * BLOCK).unwrap();
        assert_eq!(runs(&asked), vec![1, 2]);
        drop(leaving);

        staying.read_at(3 * BLOCK, BLOCK).unwrap();
        assert_eq!(runs(&asked), vec![1, 2, 4], "проход продолжился, хотя сосед закрылся");
    }

    /// prefetch привозит недостающие блоки сериями подряд идущих, ровно те,
    /// что под диапазонами, и чтения вслед — попадания без единого запроса;
    /// уже лежащее в пуле не спрашивается снова.
    #[test]
    fn prefetch_brings_the_named_blocks_in_runs() {
        let (source, asked) = in_memory(pattern(40 * BLOCK));
        source.read_at(5 * BLOCK, 16).unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1);

        source.prefetch(&[(10, 10), (BLOCK + 5, 10), (5 * BLOCK + 100, 10), (7 * BLOCK, BLOCK + 1)]).unwrap();

        let mut requested: Vec<(u64, u64)> = asked.lock().unwrap()[1..].to_vec();
        requested.sort_unstable();
        assert_eq!(requested, vec![(0, 2 * BLOCK), (7 * BLOCK, 9 * BLOCK)], "серии: блоки 0–1 и 7–8; пятый уже лежал");
        assert_eq!(source.requests.load(std::sync::atomic::Ordering::Relaxed), 3);
        assert_eq!(source.blocks_in.load(std::sync::atomic::Ordering::Relaxed), 5);

        let before = asked.lock().unwrap().len();
        source.read_at(BLOCK + 5, 10).unwrap();
        source.read_at(8 * BLOCK, 16).unwrap();
        assert_eq!(asked.lock().unwrap().len(), before, "привезённое читается без сети");
        assert_eq!(source.read_at(BLOCK + 5, 10).unwrap(), pattern(40 * BLOCK)[(BLOCK + 5) as usize..(BLOCK + 15) as usize]);
    }

    /// prefetch тянет серии по несколько разом — в полёте бывает больше одной —
    /// и не трогает разгон: промах сразу за привезённым — один блок, а не
    /// удвоение.
    #[test]
    fn prefetch_runs_in_parallel_and_leaves_the_readahead_alone() {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        let bytes = pattern(40 * BLOCK);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (inflight, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let fetch: Fetch = {
            let (bytes, journal, inflight, peak) = (bytes.clone(), asked.clone(), inflight.clone(), peak.clone());
            Box::new(move |from, to| {
                let now = inflight.fetch_add(1, Relaxed) + 1;
                peak.fetch_max(now, Relaxed);
                // Пауза длиннее старта потока: одновременные серии успевают
                // застать друг друга в полёте.
                std::thread::sleep(std::time::Duration::from_millis(20));
                journal.lock().unwrap().push((from, to));
                inflight.fetch_sub(1, Relaxed);
                Ok(bytes::Bytes::copy_from_slice(&bytes[from as usize..to as usize]))
            })
        };
        let source = HttpRange::over("http://host/f", bytes.len() as u64, Arc::new(Blocks::default()), None, fetch);
        let ranges: Vec<(u64, u64)> = (0..(IN_FLIGHT as u64 * 2 + 1)).map(|k| (2 * k * BLOCK, 16)).collect();
        source.prefetch(&ranges).unwrap();
        assert_eq!(asked.lock().unwrap().len(), ranges.len(), "каждая серия — один запрос");
        assert_eq!(source.requests.load(Relaxed), ranges.len() as u64);
        assert!(peak.load(Relaxed) >= 2, "серии не шли в полёте вместе: пик {}", peak.load(Relaxed));
        assert!(peak.load(Relaxed) <= IN_FLIGHT, "в полёте больше, чем разрешено: {}", peak.load(Relaxed));

        // Промах за концом последней серии — это не проход.
        source.read_at((2 * IN_FLIGHT as u64 * 2 + 1) * BLOCK, 16).unwrap();
        assert_eq!(runs(&asked).last(), Some(&1));

        // Пустой заказ и заказ за концом файла ничего не стоят.
        let before = asked.lock().unwrap().len();
        source.prefetch(&[]).unwrap();
        source.prefetch(&[(100 * BLOCK, 10), (5, 0)]).unwrap();
        assert_eq!(asked.lock().unwrap().len(), before);
    }

    /// Поход в сеть собирается внутри `block_on`, и потому идёт с любого
    /// потока — в том числе с потоков prefetch, где контекста рантайма нет.
    /// Сервер — петля на этой машине, отвечающая одним готовым ответом 206:
    /// это единственное место, где хост зовёт reqwest не из рантайма, и ничем,
    /// кроме живого запроса, жадный таймер клиента не держится.
    #[test]
    fn запрос_собирается_внутри_block_on() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("петля");
        let port = listener.local_addr().expect("адрес").port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("соединение");
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request);
            let body = b"0123456789abcdef";
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-15/32\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).expect("заголовки");
            socket.write_all(body).expect("тело");
        });

        let runtime = tokio::runtime::Runtime::new().expect("рантайм");
        let fetch = fetcher(runtime.handle().clone(), format!("http://127.0.0.1:{}/f", port), HashMap::new());
        // Чужой поток без контекста рантайма — как у prefetch.
        let fetched = std::thread::spawn(move || fetch(0, 16).map(|bytes| bytes.to_vec()))
            .join()
            .expect("поток без контекста рантайма не должен падать");
        assert_eq!(fetched.map_err(|e| match e { Attempt::Broken(e) | Attempt::Refused(e) => e.to_string() }), Ok(b"0123456789abcdef".to_vec()));
        server.join().expect("сервер");
    }

    /// Сорвавшаяся серия не выбрасывает соседних по пачке: привезённое ложится
    /// в пул, счётчики сходятся с ним, а заказ отвечает отказом — читатель
    /// пойдёт за недостающим обычным чтением.
    #[test]
    fn a_failed_run_keeps_the_others_in_the_pool() {
        let bytes = pattern(40 * BLOCK);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let fetch: Fetch = {
            let (bytes, journal) = (bytes.clone(), asked.clone());
            Box::new(move |from, to| {
                journal.lock().unwrap().push((from, to));
                match from == 2 * BLOCK {
                    true => Err(Attempt::Refused(anyhow::anyhow!("403"))),
                    false => Ok(bytes::Bytes::copy_from_slice(&bytes[from as usize..to as usize])),
                }
            })
        };
        let source = HttpRange::over("http://host/f", bytes.len() as u64, Arc::new(Blocks::default()), None, fetch);

        assert!(source.prefetch(&[(0, 16), (2 * BLOCK, 16), (4 * BLOCK, 16)]).is_err());

        assert_eq!(source.blocks_in.load(std::sync::atomic::Ordering::Relaxed), 2, "две серии из трёх легли");
        let before = asked.lock().unwrap().len();
        source.read_at(0, 16).unwrap();
        source.read_at(4 * BLOCK, 16).unwrap();
        assert_eq!(asked.lock().unwrap().len(), before, "привезённое читается без сети");
    }

    /// Пул отдаёт привезённое без сети: второе чтение того же блока — попадание.
    #[test]
    fn a_second_read_of_a_block_is_a_hit() {
        let (source, asked) = in_memory(pattern(3 * BLOCK));
        source.read_at(10, 10).unwrap();
        source.read_at(20, 10).unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(source.hits.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    /// Полная длина берётся из Content-Range; без неё открытие невозможно.
    #[test]
    fn content_range_names_the_full_length() {
        assert_eq!(full_length("bytes 0-1/12345"), Some(12345));
        assert_eq!(full_length("bytes */999"), Some(999));
        assert_eq!(full_length("bytes 0-1"), None);
    }

    /// Только 206 значит «понимает Range»: 200 — отказ по существу, шлюз —
    /// обрыв, чужой адрес — отказ; истёкшая подпись посреди чтения — отказ с
    /// названной причиной; короткое тело — обрыв.
    #[test]
    fn statuses_are_read_as_refusals_or_hiccups() {
        use reqwest::StatusCode;
        assert!(probed(StatusCode::PARTIAL_CONTENT, "u").is_ok());
        let no_range = probed(StatusCode::OK, "u").unwrap_err();
        assert!(matches!(&no_range, Attempt::Refused(e) if e.to_string().contains("не поддерживает Range")));
        assert!(matches!(probed(StatusCode::BAD_GATEWAY, "u"), Err(Attempt::Broken(_))));
        assert!(matches!(probed(StatusCode::NOT_FOUND, "u"), Err(Attempt::Refused(_))));

        assert!(ranged(StatusCode::PARTIAL_CONTENT, 0, 1).is_ok());
        let expired = ranged(StatusCode::FORBIDDEN, 0, 1).unwrap_err();
        assert!(matches!(&expired, Attempt::Refused(e) if e.to_string().contains("истечь")));
        assert!(matches!(ranged(StatusCode::TOO_MANY_REQUESTS, 0, 1), Err(Attempt::Broken(_))));

        assert!(delivered(10, 10).is_ok());
        assert!(matches!(delivered(5, 10), Err(Attempt::Broken(_))));
    }

    /// Оборвавшийся диапазон переспрашивается и доезжает; отказ по существу
    /// не переспрашивается вовсе.
    #[test]
    fn a_broken_range_is_asked_again_and_a_refusal_is_not() {
        let tries = Arc::new(Mutex::new(0u32));
        let counter = tries.clone();
        let fetch: Fetch = Box::new(move |from, to| {
            let mut tries = counter.lock().unwrap();
            *tries += 1;
            match *tries {
                1 => Err(Attempt::Broken(anyhow::anyhow!("сброс"))),
                _ => Ok(bytes::Bytes::from(vec![7; (to - from) as usize])),
            }
        });
        let source = HttpRange::over("http://h/f", 2 * BLOCK, Arc::new(Blocks::default()), None, fetch);
        assert_eq!(source.read_at(0, 4).unwrap(), vec![7, 7, 7, 7]);
        assert_eq!(*tries.lock().unwrap(), 2);

        let refused: Fetch = Box::new(|_, _| Err(Attempt::Refused(anyhow::anyhow!("403"))));
        let source = HttpRange::over("http://h/f", 2 * BLOCK, Arc::new(Blocks::default()), None, refused);
        assert!(source.read_at(0, 4).is_err());
    }
}
