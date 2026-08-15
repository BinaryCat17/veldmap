//! Тайлы пирамиды на стороне потребителя: что уже лежит в видеопамяти и что
//! ещё едет.
//!
//! Потребителей двое — канва просмотра и наложения на шаре, — и оба ведут один
//! и тот же танец: описать источник, спросить у кэша прямоугольник ячеек,
//! промахи отдать производителю, приехавшее сложить в видеопамять и вытеснить
//! оттуда давно не рисовавшееся. Различаются они только тем, **какие ячейки
//! нужны прямо сейчас**: канве — видимый прямоугольник её камеры, наложению —
//! то, что попало в кадр на шаре. Всё остальное — механика, и держать её в двух
//! экземплярах значит однажды починить промах в одном месте и не починить в
//! другом.
//!
//! Поэтому здесь ровно эта механика и ничего про протокол: [`Store`] — тайлы в
//! видеопамяти с вытеснением по бюджету, [`Fetch`] — учёт того, что спрошено,
//! что производится и что уже не спрашивать. Сообщения кэшу и производителю
//! шлёт потребитель: топики объявлены в его схеме, и публиковать за него нельзя
//! (см. заголовок сгенерированного lib.rs) — сюда приезжает уже готовый ответ.
//!
//! Место здесь по той же причине, что и у `pyramid`: правила пирамиды
//! принадлежат тому, кто её производит, и потребители работают с ней одним и
//! тем же кодом, а не каждый своим.

use std::collections::{HashMap, HashSet};

use veldsdk::graphics::{self as gfx, BindGroupId, TextureViewId};
use veldsdk::proto::core::ResourceHandle;

/// Адрес тайла в пирамиде: уровень, колонка, ряд. Тот же, каким его знают
/// дисковый кэш и производитель.
pub type Addr = (u32, u32, u32);

// ── Видеопамять ────────────────────────────────────────────────

pub struct Stored {
    /// Держит текстуру живой; bind group и view — производные от неё.
    _texture: veldsdk::OwnedResource,
    _view: TextureViewId,
    pub bind: BindGroupId,
    pub width: u32,
    pub height: u32,
    bytes: u64,
    touched: u64,
}

/// Тайлы в видеопамяти, с вытеснением по бюджету.
///
/// Хранилище одно на модуль, а не на зрителя: две вкладки с одним снимком —
/// это один набор тайлов, и бюджет видеопамяти тоже один на всех. Ключ —
/// отпечаток источника и адрес в пирамиде: тот же, что у дискового кэша, потому
/// что это тот же самый тайл ярусом выше.
///
/// Вытеснение — давно не рисовавшиеся: каждое обращение из кадра трогает
/// запись, и старее всех та, на которую никто не смотрит. Вытеснение — это
/// забывание: тайл остаётся на диске у tile-cache и вернётся оттуда за
/// миллисекунды.
pub struct Store {
    /// Отпечаток источника → тайлы. Двухэтажная карта, чтобы не клонировать
    /// отпечаток в ключ каждого тайла.
    sources: HashMap<String, HashMap<Addr, Stored>>,
    budget: u64,
    bytes: u64,
    /// Логические часы обращений — по ним выбирается жертва.
    tick: u64,
    /// Растёт на каждом добавлении и вытеснении: кадр, собранный при другом
    /// поколении, устарел.
    pub generation: u64,
}

impl Store {
    pub fn new(budget: u64) -> Self {
        Self { sources: HashMap::new(), budget, bytes: 0, tick: 0, generation: 0 }
    }

    /// Пустое хранилище с тем же бюджетом — на пересборку устройства: bind
    /// group'ы собраны под layout прежнего и с новым несовместимы.
    pub fn new_like(other: &Store) -> Self {
        Self::new(other.budget)
    }

    /// Сколько тайлов помещается в бюджет — потолок аппетита одного уровня.
    pub fn capacity_tiles(&self, tile_bytes: u64) -> u64 {
        self.budget / tile_bytes.max(1)
    }

    /// Принимает тайл во владение: view создаётся здесь, а bind group — тем,
    /// кто его потом рисует: layout привязки принадлежит его пайплайну. Отказ
    /// графики — тайл выброшен (Drop), промах останется промахом.
    pub fn insert(
        &mut self,
        fingerprint: &str,
        addr: Addr,
        texture: ResourceHandle,
        width: u32,
        height: u32,
        bind: impl FnOnce(&TextureViewId) -> veldsdk::anyhow::Result<BindGroupId>,
    ) -> Result<(), String> {
        let texture = veldsdk::OwnedResource::new(texture);
        let view = gfx::create_texture_view(texture.id()).map_err(|e| e.to_string())?;
        let bind = bind(&view).map_err(|e| e.to_string())?;

        let bytes = u64::from(width) * u64::from(height) * 4;
        self.tick += 1;
        let stored = Stored {
            _texture: texture,
            _view: view,
            bind,
            width,
            height,
            bytes,
            touched: self.tick,
        };

        let tiles = self.sources.entry(fingerprint.to_string()).or_default();
        // Повтор (кэш и производитель могли прислать один тайл наперегонки)
        // замещается: старая текстура освобождается своим Drop.
        if let Some(old) = tiles.insert(addr, stored) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.generation += 1;
        self.evict_to_budget();
        Ok(())
    }

    /// Тайл к рисованию: обращение продлевает ему жизнь.
    pub fn touch(&mut self, fingerprint: &str, addr: Addr) -> Option<&Stored> {
        self.tick += 1;
        let stored = self.sources.get_mut(fingerprint)?.get_mut(&addr)?;
        stored.touched = self.tick;
        Some(stored)
    }

    pub fn contains(&self, fingerprint: &str, addr: Addr) -> bool {
        self.sources.get(fingerprint).is_some_and(|tiles| tiles.contains_key(&addr))
    }

    /// Забыть источник целиком — его больше не показывают, и тайлам не с чего
    /// рисоваться.
    pub fn forget(&mut self, fingerprint: &str) {
        if let Some(tiles) = self.sources.remove(fingerprint) {
            for stored in tiles.values() {
                self.bytes -= stored.bytes;
            }
            self.generation += 1;
        }
    }

    /// Старейшие — вон, пока не влезем. Линейный поиск жертвы честен: бюджет
    /// в четверть гигабайта — это порядка двух сотен тайлов.
    fn evict_to_budget(&mut self) {
        while self.bytes > self.budget {
            let victim = self
                .sources
                .iter()
                .flat_map(|(key, tiles)| {
                    tiles.iter().map(move |(addr, stored)| (stored.touched, key, addr))
                })
                .min_by_key(|(touched, ..)| *touched)
                .map(|(_, key, addr)| (key.clone(), *addr));
            let Some((key, addr)) = victim else { return };

            if let Some(tiles) = self.sources.get_mut(&key) {
                if let Some(old) = tiles.remove(&addr) {
                    self.bytes -= old.bytes;
                }
                if tiles.is_empty() {
                    self.sources.remove(&key);
                }
            }
            self.generation += 1;
        }
    }
}

// ── Что спрошено и что производится ────────────────────────────

/// Ступень добычи: самый грубый уровень от вершины пирамиды к целевому, которого
/// ещё не хватает, и его ячейки.
///
/// Пирамида набирается сверху вниз, а не прыжком к целевому уровню. Вершина —
/// это один тайл из самой мелкой копии файла: он приезжает за секунду и
/// накрывает снимок целиком, тогда как целевой уровень из десятка тайлов по
/// мегабайту едет полминуты, и всё это время показывать было бы нечего. Каждая
/// добытая ступень к тому же становится предком для следующей (parent-fallback
/// у обоих потребителей), поэтому дыр между ступенями не бывает.
///
/// `ready` — «эта ячейка ступень не держит»: она либо уже есть, либо её не
/// будет вовсе (производитель отказал, см. [`Fetch::hopeless`]). Без второго
/// ступень встала бы на непроизводимом уровне навсегда.
pub fn rung(
    target: u32,
    top: u32,
    cells_at: impl Fn(u32) -> Vec<Addr>,
    ready: impl Fn(Addr) -> bool,
) -> (u32, Vec<Addr>) {
    for level in ((target + 1)..=top).rev() {
        let cells = cells_at(level);
        if cells.iter().any(|addr| !ready(*addr)) {
            return (level, cells);
        }
    }
    (target, cells_at(target))
}

/// Держит ли ячейка ступень. Не держит та, что уже в видеопамяти, и та, которой
/// не будет вовсе (производитель отказал).
///
/// Предикат один на выбор ступени и на счёт добытого: разойдясь, они дают
/// полосу, которая не доходит до конца никогда, — ступень уже закрыта отказом,
/// а счёт всё ещё ждёт непроизводимых ячеек.
pub fn settled(store: &Store, fetch: &Fetch, fingerprint: &str, addr: Addr) -> bool {
    store.contains(fingerprint, addr) || fetch.hopeless(addr)
}

/// Прямоугольник ячеек, который просят у кэша: кэш отвечает про область целиком
/// (см. его `QueryRequest`), а список — то, что из неё нужно на самом деле.
pub struct Ask {
    pub level: u32,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// Ячейки, ради которых запрос и сделан, — ими же он потом опознаётся.
    pub cells: Vec<Addr>,
}

/// Учёт по одному источнику: что уже спрошено, что не спрашивать и идёт ли по
/// нему проход производителя.
///
/// Производство одно на источник: тайлер читает файл проходом, и второй проход
/// по тому же файлу мешал бы первому. Пока оно идёт, новые запросы
/// откладываются до его конца — там желаемое пересчитывается заново
/// (см. `Fetch::stale`).
#[derive(Default)]
pub struct Fetch {
    /// Спрошенные и ещё не приехавшие: второй раз их не просят.
    inflight: HashSet<Addr>,
    /// Не произведённые: переспрашивать их — долбить производителя тем же
    /// отказом на каждый сдвиг камеры.
    failed: HashSet<Addr>,
    /// Ячейки последнего оформленного заказа — того, что уехал в кэш. Ими
    /// меряется ход добычи, а не тем, что видно прямо сейчас: видимое есть
    /// функция камеры, и доля, выведенная из него, ездит взад-вперёд на каждом
    /// кадре жеста. Заказ переписывается, только когда действительно уходит
    /// новый запрос.
    ordered: Vec<Addr>,
    /// Идущий проход: корреляция и уровень, ради которого он затеян.
    produce: Option<(String, u32)>,
}

impl Fetch {
    /// Проход, ставший бесполезным: он производит уровень **подробнее** того,
    /// который смотрят сейчас, — такими тайлами грубую ячейку не нарисовать.
    /// Отдаёт его корреляцию; убивает её вызывающий, у него для этого свой
    /// `cancel`.
    ///
    /// Проход грубее нужного не убивается, и это не поблажка: ячейка рисуется
    /// ближайшим имеющимся предком, то есть тайлом любого уровня выше, — а
    /// один щелчок колеса растянут инерцией на секунду и меняет уровень по
    /// нескольку раз. Убивая проход на каждой смене, мы выбрасывали бы
    /// наполовину прочитанный файл столько же раз за одно приближение.
    pub fn stale(&mut self, level: u32) -> Option<String> {
        let stale = self.produce.as_ref().is_some_and(|(_, going)| *going < level);
        match stale {
            true => self.produce.take().map(|(correlation, _)| correlation),
            false => None,
        }
    }

    /// Заказ и сколько из него уже дошло — то, чем меряется ход добычи.
    /// Дошедшим считается всё, что перестало быть ожидаемым: приехавшее и то,
    /// в чём отказали.
    pub fn ordered(&self) -> (u32, u32) {
        let done = self.ordered.iter().filter(|addr| !self.inflight.contains(addr)).count();
        (done as u32, self.ordered.len() as u32)
    }

    pub fn producing(&self) -> bool {
        self.produce.is_some()
    }

    /// Спрошено и ещё не приехало: кэш ищет, производитель читает — работа идёт.
    /// Пусто — либо всё нужное уже есть, либо просить нечего.
    pub fn waiting(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// Ячейка, которой больше не будет: производитель уже отказал. Спрашивают
    /// об этом те, кто решает, чего не хватает: без неё ступень добычи (самый
    /// грубый недостающий уровень) встала бы на непроизводимом уровне навсегда
    /// и до подробных никогда не дошла.
    pub fn hopeless(&self, addr: Addr) -> bool {
        self.failed.contains(&addr)
    }

    /// Источник сменился или кончился: всё, что помнилось, было про прежний.
    /// Отдаёт корреляцию идущего прохода — убивает её вызывающий, у него для
    /// этого свой `cancel`.
    pub fn reset(&mut self) -> Option<String> {
        self.inflight.clear();
        self.failed.clear();
        self.ordered.clear();
        self.produce.take().map(|(correlation, _)| correlation)
    }

    /// Что спросить у кэша: желаемое без того, что уже лежит, уже спрошено или
    /// уже не произвелось. `None` — спрашивать нечего либо по источнику идёт
    /// проход.
    ///
    /// Спрошенное сразу считается ожидаемым: `want` зовут на каждое движение
    /// камеры, и без этого одна и та же ячейка уехала бы в кэш десяток раз.
    pub fn want(
        &mut self,
        store: &Store,
        fingerprint: &str,
        level: u32,
        desired: impl IntoIterator<Item = Addr>,
    ) -> Option<Ask> {
        if self.producing() {
            return None;
        }
        let cells: Vec<Addr> = desired
            .into_iter()
            .filter(|addr| {
                !store.contains(fingerprint, *addr)
                    && !self.inflight.contains(addr)
                    && !self.failed.contains(addr)
            })
            .collect();
        if cells.is_empty() {
            return None;
        }

        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for &(_, x, y) in &cells {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
        self.inflight.extend(cells.iter().copied());
        self.ordered = cells.clone();
        Some(Ask { level, x0, y0, x1, y1, cells })
    }

    /// Промахи кэша → что отдать производителю. `still_wanted` отсеивает
    /// уехавшее с глаз, пока запрос шёл: его переспросят, когда оно вернётся в
    /// кадр, а производить его сейчас значит занять единственный проход не тем.
    ///
    /// Всё, что в производство не пошло, снимается с ожидания: иначе следующий
    /// пересчёт сочтёт эти ячейки уже спрошенными и не переспросит никогда.
    pub fn missed(
        &mut self,
        asked: &[Addr],
        misses: impl IntoIterator<Item = Addr>,
        still_wanted: impl Fn(Addr) -> bool,
    ) -> Vec<Addr> {
        let mut produce = Vec::new();
        for addr in misses {
            // Не из этого запроса — про него мы ничего не знаем.
            if !asked.contains(&addr) {
                continue;
            }
            if still_wanted(addr) && !self.failed.contains(&addr) && !self.producing() {
                produce.push(addr);
            } else {
                self.inflight.remove(&addr);
            }
        }
        produce
    }

    /// Запрос кэша сорвался: ожидания снимаются, и на следующем пересчёте они
    /// спросятся заново.
    pub fn forget_asked(&mut self, cells: &[Addr]) {
        for addr in cells {
            self.inflight.remove(addr);
        }
    }

    /// Проход начат.
    pub fn producing_now(&mut self, correlation: String, level: u32) {
        self.produce = Some((correlation, level));
    }

    /// Тайл приехал — из ожидания вон, чей бы запрос его ни привёз.
    pub fn arrived(&mut self, addr: Addr) {
        self.inflight.remove(&addr);
    }

    /// Проход кончился, чем бы он ни кончился. `failed` — отказ: эти ячейки
    /// больше не просят.
    pub fn produced(&mut self, correlation: &str, cells: &[Addr], failed: bool) {
        if self.produce.as_ref().is_some_and(|(going, _)| going == correlation) {
            self.produce = None;
        }
        self.forget_asked(cells);
        if failed {
            self.failed.extend(cells.iter().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(level: u32, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>) -> Vec<Addr> {
        ys.flat_map(|y| xs.clone().map(move |x| (level, x, y))).collect()
    }

    /// Ступень начинается с вершины пирамиды и спускается за добытым: пока
    /// вершины нет, просят её, а не целевой уровень.
    #[test]
    fn ступень_идёт_сверху_вниз() {
        // По одной ячейке на уровень — считать здесь нечего, проверяется выбор.
        let cells_at = |level| vec![(level, 0, 0)];

        assert_eq!(rung(0, 3, cells_at, |_| false), (3, vec![(3, 0, 0)]));
        // Вершина есть — следующая ступень; и так до целевого уровня.
        let have = |addr: Addr| addr.0 >= 2;
        assert_eq!(rung(0, 3, cells_at, have), (1, vec![(1, 0, 0)]));
        assert_eq!(rung(0, 3, cells_at, |_| true), (0, vec![(0, 0, 0)]), "всё есть — целевой");
        // Целевой и есть вершина — ступать некуда.
        assert_eq!(rung(3, 3, cells_at, |_| false), (3, vec![(3, 0, 0)]));
    }

    /// Ступень держит любая недостающая ячейка, а не только все сразу: уровень
    /// с дырой — это уровень, который ещё едет.
    #[test]
    fn ступень_держит_одна_недостающая_ячейка() {
        let cells_at = |level| cells(level, 0..2, 0..2);
        let have = |addr: Addr| addr != (2, 1, 1);
        assert_eq!(rung(0, 2, cells_at, have).0, 2);
    }

    /// Спрошенное не спрашивается второй раз, а прямоугольник запроса
    /// охватывает ровно нужное: кэш отвечает про область, и лишняя ширина —
    /// это лишняя работа на его стороне.
    #[test]
    fn спрошенное_не_спрашивается_дважды() {
        let store = Store::new(1 << 30);
        let mut fetch = Fetch::default();

        let ask = fetch.want(&store, "s", 2, cells(2, 1..3, 4..6)).expect("есть что спросить");
        assert_eq!((ask.x0, ask.y0, ask.x1, ask.y1), (1, 4, 3, 6));
        assert_eq!(ask.cells.len(), 4);

        assert!(fetch.want(&store, "s", 2, cells(2, 1..3, 4..6)).is_none(), "то же самое");
        // Приехавшее снимается с ожидания и спросится снова, если его вытеснят.
        fetch.arrived((2, 1, 4));
        let again = fetch.want(&store, "s", 2, cells(2, 1..3, 4..6)).expect("одна вернулась");
        assert_eq!(again.cells, vec![(2, 1, 4)]);
    }

    /// Пока идёт проход, новых запросов нет: тайлер читает файл насквозь, и
    /// второй проход мешал бы первому.
    #[test]
    fn во_время_прохода_не_спрашивают() {
        let store = Store::new(1 << 30);
        let mut fetch = Fetch::default();
        fetch.producing_now("c1".into(), 2);

        assert!(fetch.want(&store, "s", 2, cells(2, 0..2, 0..1)).is_none());
        // Проход того же уровня — не устаревший; грубее нужного — тоже: его
        // тайлами ячейку нарисуют предком.
        assert!(fetch.stale(2).is_none());
        assert!(fetch.stale(1).is_none(), "проход грубее нужного ещё пригодится");
        // А подробнее нужного — устаревший: такими тайлами грубую ячейку не
        // нарисовать.
        assert_eq!(fetch.stale(3).as_deref(), Some("c1"));
        assert!(fetch.want(&store, "s", 3, cells(3, 0..2, 0..1)).is_some());
    }

    /// Ход добычи меряется заказом, а не тем, что видно: заказ переписывается
    /// только вместе с новым запросом, и камера его не двигает.
    #[test]
    fn ход_меряется_заказом() {
        let store = Store::new(1 << 30);
        let mut fetch = Fetch::default();
        assert_eq!(fetch.ordered(), (0, 0), "заказа ещё не было");

        let ask = fetch.want(&store, "s", 0, cells(0, 0..2, 0..2)).expect("запрос");
        assert_eq!(fetch.ordered(), (0, 4));
        fetch.arrived(ask.cells[0]);
        assert_eq!(fetch.ordered(), (1, 4));
        // Отказ считается доведённым наравне с приездом: этой ячейки не будет
        // вовсе, и ждать её — значит не досчитать до конца никогда.
        fetch.producing_now("c1".into(), 0);
        fetch.produced("c1", &ask.cells[1..], true);
        assert_eq!(fetch.ordered(), (4, 4));
    }

    /// Промах, уехавший с глаз, в производство не идёт и с ожидания снимается —
    /// иначе он не спросится больше никогда.
    #[test]
    fn промахи_отбираются_по_желанности() {
        let store = Store::new(1 << 30);
        let mut fetch = Fetch::default();
        let ask = fetch.want(&store, "s", 0, cells(0, 0..3, 0..1)).expect("запрос");

        let produce = fetch.missed(&ask.cells, ask.cells.clone(), |addr| addr.1 == 0);
        assert_eq!(produce, vec![(0, 0, 0)]);
        // Чужой промах не наш: про него мы ничего не знаем.
        assert!(fetch.missed(&ask.cells, [(0, 9, 9)], |_| true).is_empty());

        let again = fetch.want(&store, "s", 0, cells(0, 0..3, 0..1)).expect("отсеянные вернулись");
        assert_eq!(again.cells, vec![(0, 1, 0), (0, 2, 0)]);
    }

    /// Не произведённое не спрашивается заново: иначе каждый кадр звал бы
    /// производителя за тем же отказом.
    #[test]
    fn непроизведённое_больше_не_просят() {
        let store = Store::new(1 << 30);
        let mut fetch = Fetch::default();
        let ask = fetch.want(&store, "s", 0, cells(0, 0..2, 0..1)).expect("запрос");
        fetch.producing_now("c1".into(), 0);

        fetch.produced("c1", &ask.cells, true);
        assert!(!fetch.producing(), "проход снят с учёта");
        assert!(fetch.want(&store, "s", 0, cells(0, 0..2, 0..1)).is_none());
        assert!(fetch.hopeless((0, 0, 0)), "и ступень добычи об этом знает");

        // Удавшийся — наоборот: ячейки просто перестают быть ожидаемыми.
        let mut fetch = Fetch::default();
        let ask = fetch.want(&store, "s", 0, cells(0, 0..2, 0..1)).expect("запрос");
        fetch.producing_now("c2".into(), 0);
        fetch.produced("c2", &ask.cells, false);
        assert!(fetch.want(&store, "s", 0, cells(0, 0..2, 0..1)).is_some());
    }
}
