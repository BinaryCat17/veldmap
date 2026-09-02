//! Фальшивый хост для нативных тестов: ресурсы с арендой, журнал чтений,
//! журнал публикаций, захват лога.
//!
//! Нативные заглушки ABI (`abi.rs`, ветка `not(wasm32)`) без него ведут себя
//! как хост, у которого кончилось всё: ресурсы не находятся, публикации
//! уходят в никуда. После [`install`] на этом потоке они отвечают отсюда, и
//! тест видит то, чего нативно не видно иначе: сколько окон прочёл читатель,
//! что и с какой корреляцией опубликовал модуль, освободил ли он ресурс.
//!
//! Ответы собираются той же кодировкой, что у хоста (`abi/wire.rs`, один
//! файл на обе стороны), и кладутся в арену — линейную память гостя понарошку:
//! указатель ответа обязан влезать в 32 бита, как в wasm, и нативный адрес
//! кучи для этого не годится. Место под ответ просит `veld_alloc`, как у
//! хоста, и освобождает `veld_free_wasm`.
//!
//! Всё состояние — `thread_local`: тесты бегут потоками, и один общий хост
//! перепутал бы их журналы. Так же живёт и контекст события (`abi.rs`).

use std::cell::RefCell;
use std::collections::BTreeMap;

use prost::Message;

use crate::abi::wire;
use crate::proto::core::{EventEnvelope, ResourceHandle};

/// Одно чтение ресурса, каким его попросили у хоста: окно, а не байты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub id: u64,
    pub offset: u64,
    pub size: u64,
}

/// Опубликованное модулем событие, разобранное из конверта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub topic: String,
    pub payload: Vec<u8>,
    pub correlation: String,
    pub target: String,
}

/// Строка лога, ушедшая бы хосту.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

/// Имя, под которым живёт сам тестируемый модуль: владелец всего, что он
/// смонтировал или выделил.
pub const MODULE: &str = "module";

struct Resource {
    bytes: Vec<u8>,
    owner: String,
    readers: Vec<String>,
    writers: Vec<String>,
    /// Непрозрачный (текстура): байтового диапазона за ним нет.
    opaque: bool,
}

#[derive(Default)]
struct State {
    resources: BTreeMap<u64, Resource>,
    next_id: u64,
    reads: Vec<Read>,
    published: Vec<Published>,
    logged: Vec<Logged>,
    killed: Vec<String>,
}

thread_local! {
    static FAKE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Поднимает фальшивый хост на этом потоке с чистого листа. Зовётся в начале
/// теста: прежнее состояние (чужого теста на том же потоке) уходит.
///
/// Контекст события фальшивка не трогает: отправитель по умолчанию пуст, как у
/// события от хоста, и `accept` с `reply::undelivered` прочтут пустой ответ
/// как договорённый хостом. Тест ответа живого исполнителя называет
/// отправителя сам — `abi::set_event_context`.
pub fn install() {
    FAKE.with(|fake| *fake.borrow_mut() = Some(State { next_id: 1, ..State::default() }));
}

/// Снимает фальшивый хост: заглушки снова отвечают нулями.
pub fn uninstall() {
    FAKE.with(|fake| *fake.borrow_mut() = None);
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    FAKE.with(|fake| fake.borrow_mut().as_mut().map(f))
}

fn installed<T>(f: impl FnOnce(&mut State) -> T) -> T {
    with(f).expect("фальшивый хост не поднят: зовите veldsdk::fake::install() в начале теста")
}

/// Кладёт байты ресурсом во владении модуля и отдаёт его дескриптор.
pub fn mount(bytes: impl Into<Vec<u8>>) -> ResourceHandle {
    let bytes = bytes.into();
    let size = bytes.len() as u64;
    let id = installed(|s| s.add(bytes, false));
    ResourceHandle { id, size }
}

/// Все чтения с момента [`install`], по порядку.
pub fn reads() -> Vec<Read> {
    installed(|s| s.reads.clone())
}

/// Все публикации с момента [`install`], по порядку.
pub fn published() -> Vec<Published> {
    installed(|s| s.published.clone())
}

/// Всё, что модуль написал в лог.
pub fn logged() -> Vec<Logged> {
    installed(|s| s.logged.clone())
}

/// Ресурсы, которые никто не освободил: смонтированные, выделенные и
/// переданные — всё, что ещё лежит у хоста.
pub fn leaked() -> Vec<u64> {
    installed(|s| s.resources.keys().copied().collect())
}

/// Владелец ресурса; `None` — ресурса нет.
pub fn owner(id: u64) -> Option<String> {
    installed(|s| s.resources.get(&id).map(|r| r.owner.clone()))
}

/// Кому выдано право чтения.
pub fn readers(id: u64) -> Vec<String> {
    installed(|s| s.resources.get(&id).map(|r| r.readers.clone()).unwrap_or_default())
}

/// Операции, которые модуль просил убить.
pub fn killed() -> Vec<String> {
    installed(|s| s.killed.clone())
}

impl Resource {
    /// Аренда — как у хоста (`registry.rs`): читает владелец или тот, кому
    /// выдано чтение; пишет и освобождает владелец или тот, кому выдана запись.
    fn readable(&self) -> bool {
        self.owner == MODULE || self.readers.iter().any(|r| r == MODULE)
    }

    fn writable(&self) -> bool {
        self.owner == MODULE || self.writers.iter().any(|w| w == MODULE)
    }
}

impl State {
    fn add(&mut self, bytes: Vec<u8>, opaque: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.resources.insert(id, Resource {
            bytes,
            owner: MODULE.to_string(),
            readers: Vec::new(),
            writers: Vec::new(),
            opaque,
        });
        id
    }
}

// ── Арена: линейная память гостя понарошку ────────────────────────

pub(crate) mod arena {
    use std::cell::RefCell;

    /// Смещения считаются от начала буфера; ноль занят — им хост говорит
    /// «ответа нет», поэтому первое место выдаётся дальше.
    const FIRST: usize = 8;

    struct Arena {
        buf: Vec<u8>,
        top: usize,
        live: usize,
    }

    thread_local! {
        static ARENA: RefCell<Arena> = const { RefCell::new(Arena { buf: Vec::new(), top: FIRST, live: 0 }) };
    }

    /// Место под `size` байт; возвращается смещение, годное в 32 бита.
    pub fn alloc(size: u64) -> u64 {
        ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            let at = (arena.top + 7) & !7;
            let end = at + size as usize;
            if arena.buf.len() < end {
                arena.buf.resize(end, 0);
            }
            arena.top = end;
            arena.live += 1;
            at as u64
        })
    }

    /// Освобождает выданное; когда живых мест не остаётся, арена начинается
    /// заново — отдельного учёта дыр ей не нужно.
    pub fn free(_ptr: u64, _size: u64) {
        ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            arena.live = arena.live.saturating_sub(1);
            if arena.live == 0 {
                arena.top = FIRST;
            }
        });
    }

    /// Адрес в нативной памяти за смещением гостя. Годен до следующего
    /// `alloc`: тот вправе переложить буфер.
    pub fn address(ptr: u64) -> *mut u8 {
        ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            let end = ptr as usize;
            if arena.buf.len() < end {
                arena.buf.resize(end, 0);
            }
            unsafe { arena.buf.as_mut_ptr().add(end) }
        })
    }

    /// Пишет ответ в арену и отдаёт упакованную пару, как хост.
    pub fn respond(bytes: &[u8]) -> u64 {
        let ptr = crate::abi::veld_alloc(bytes.len() as u64);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address(ptr), bytes.len()) };
        super::wire::pack(ptr, bytes.len() as u64)
    }
}

fn respond(result: Result<Vec<u8>, String>) -> u64 {
    arena::respond(&wire::tagged(result))
}

// ── Заглушки: то, что зовут нативные `veld_*` из abi.rs ───────────────

pub(crate) fn publish(envelope: &[u8]) {
    with(|s| {
        if let Ok(request) = EventEnvelope::decode(envelope) {
            s.published.push(Published {
                topic: format!("{}/{}", request.service, request.method),
                payload: request.payload,
                correlation: request.correlation_id,
                target: request.target,
            });
        }
    });
}

pub(crate) fn log(level: log::Level, target: &str, message: &str) {
    with(|s| s.logged.push(Logged { level, target: target.to_string(), message: message.to_string() }));
}

/// Чтение за концом — короткий или пустой ответ, как у хоста
/// (`MemoryManager::read`): читатель идёт окнами, и последнее почти всегда
/// неполное. Отказ — нет ресурса, нет права или за ним нет байт; тексты
/// отказов — те же, что у хоста (`abi.rs` ядра).
pub(crate) fn resource_read(id: u64, offset: u64, size: u64) -> u64 {
    let Some(result) = with(|s| {
        s.reads.push(Read { id, offset, size });
        match s.resources.get(&id) {
            None => Err(format!("ресурс {} не найден", id)),
            Some(r) if !r.readable() => Err(format!("чтение ресурса {} запрещено", id)),
            Some(r) if r.opaque => Err(format!("ресурс {} — не байты", id)),
            Some(r) => {
                let len = r.bytes.len() as u64;
                if offset >= len {
                    return Ok(Vec::new());
                }
                let end = (offset + size).min(len);
                Ok(r.bytes[offset as usize..end as usize].to_vec())
            }
        }
    }) else {
        return 0;
    };
    respond(result)
}

/// Запись за концом растит байты, как у хоста память `Cpu`
/// (`MemoryManager::write`): размер назначен буферам GPU, а не памяти.
pub(crate) fn resource_write(id: u64, offset: u64, data: &[u8]) -> u64 {
    let Some(result) = with(|s| match s.resources.get_mut(&id) {
        None => Err(format!("ресурс {} не найден", id)),
        Some(r) if !r.writable() => Err(format!("запись в ресурс {} запрещена", id)),
        Some(r) if r.opaque => Err(format!("ресурс {} — не байты", id)),
        Some(r) => {
            let end = offset as usize + data.len();
            if end > r.bytes.len() {
                r.bytes.resize(end, 0);
            }
            r.bytes[offset as usize..end].copy_from_slice(data);
            Ok(Vec::new())
        }
    }) else {
        return 0;
    };
    respond(result)
}

pub(crate) fn alloc_bytes(size: u64) -> u64 {
    with(|s| s.add(vec![0; size as usize], false)).unwrap_or(0)
}

pub(crate) fn alloc_opaque() -> u64 {
    with(|s| s.add(Vec::new(), true)).unwrap_or(0)
}

/// Освобождает право записи — как у хоста: `free` идёт по нему же.
pub(crate) fn free(id: u64) -> u64 {
    with(|s| match s.resources.get(&id) {
        Some(r) if r.writable() => { s.resources.remove(&id); 1 }
        _ => 0,
    })
    .unwrap_or(0)
}

/// Передаёт и выдаёт права только владелец.
pub(crate) fn transfer(id: u64, to: &str) -> u64 {
    with(|s| match s.resources.get_mut(&id) {
        Some(r) if r.owner == MODULE => {
            r.owner = to.to_string();
            r.readers.clear();
            r.writers.clear();
            1
        }
        _ => 0,
    })
    .unwrap_or(0)
}

pub(crate) fn grant_read(id: u64, to: &str) -> u64 {
    with(|s| match s.resources.get_mut(&id) {
        Some(r) if r.owner == MODULE => { r.readers.push(to.to_string()); 1 }
        _ => 0,
    })
    .unwrap_or(0)
}

pub(crate) fn grant_write(id: u64, to: &str) -> u64 {
    with(|s| match s.resources.get_mut(&id) {
        Some(r) if r.owner == MODULE => { r.writers.push(to.to_string()); 1 }
        _ => 0,
    })
    .unwrap_or(0)
}

pub(crate) fn task_kill(task: &str) -> u64 {
    with(|s| { s.killed.push(task.to_string()); 0 }).unwrap_or(0)
}

pub(crate) fn unsupported(what: &str) -> u64 {
    match with(|_| ()) {
        Some(()) => respond(Err(format!("фальшивый хост не умеет {}", what))),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi;

    /// Без `install` заглушки — прежний хост, у которого кончилось всё.
    #[test]
    fn without_install_the_stubs_stay_silent() {
        uninstall();
        assert!(abi::resource_read(1, 0, 10).is_err());
        assert_eq!(abi::resource_alloc_cpu(10), None);
        abi::publish("svc/topic", vec![1], "", "");
    }

    /// Публикация доезжает разобранной: топик, нагрузка, корреляция, адресат.
    #[test]
    fn a_publication_lands_in_the_journal() {
        install();
        abi::publish("svc/on_topic", vec![1, 2, 3], "corr-1", "addressee");
        assert_eq!(published(), vec![Published {
            topic: "svc/on_topic".to_string(),
            payload: vec![1, 2, 3],
            correlation: "corr-1".to_string(),
            target: "addressee".to_string(),
        }]);
    }

    /// Чтение за концом коротко, а не ошибочно, — то же правило, что у хоста;
    /// ошибка приезжает текстом, тем же тегом, что кладёт хост.
    #[test]
    fn reads_beyond_the_end_are_short_and_errors_travel_as_text() {
        install();
        let handle = mount(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(abi::resource_read(handle.id, 8, 100).unwrap(), vec![9, 10]);
        assert_eq!(abi::resource_read(handle.id, 10, 5).unwrap(), Vec::<u8>::new());
        let why = abi::resource_read(77, 0, 1).unwrap_err().to_string();
        assert!(why.contains("не найден"), "{why}");
        assert_eq!(reads().len(), 3, "каждое чтение — запись журнала, и отказ тоже");
    }

    /// Выделенное и записанное читается назад; запись за концом растит
    /// память, как у хоста; освобождённое исчезает.
    #[test]
    fn allocated_bytes_are_written_read_and_freed() {
        install();
        let id = abi::resource_alloc_cpu(4).expect("память есть");
        abi::resource_write(id, 1, &[7, 8]).unwrap();
        assert_eq!(abi::resource_read(id, 0, 4).unwrap(), vec![0, 7, 8, 0]);
        abi::resource_write(id, 3, &[1, 2]).unwrap();
        assert_eq!(abi::resource_read(id, 0, 8).unwrap(), vec![0, 7, 8, 1, 2]);
        assert_eq!(leaked(), vec![id]);
        crate::resource::release(ResourceHandle { id, size: 4 });
        assert!(leaked().is_empty());
    }

    /// Отданное заказчику модулю больше не принадлежит: ни прочитать, ни
    /// освободить — как у хоста, и теми же словами.
    #[test]
    fn a_handed_off_resource_is_no_longer_ours() {
        install();
        let handle = crate::resource::hand_off(mount(vec![1, 2, 3]), "data-browser").unwrap();
        let why = abi::resource_read(handle.id, 0, 3).unwrap_err().to_string();
        assert!(why.contains("запрещено"), "{why}");
        assert!(abi::resource_write(handle.id, 0, &[9]).unwrap_err().to_string().contains("запрещена"));
        crate::resource::release(handle.clone());
        assert_eq!(leaked(), vec![handle.id], "чужое не освобождается");
        assert_eq!(crate::resource::grant_read_or_free(handle.id, "x").is_err(), true,
            "права раздаёт только владелец, а при отказе хелпер зовёт free — тоже впустую");
        assert_eq!(leaked(), vec![handle.id]);
    }

    /// Передача владения меняет владельца и снимает гранты — как у хоста.
    #[test]
    fn hand_off_changes_the_owner_and_drops_grants() {
        install();
        let handle = mount(vec![0; 3]);
        crate::resource::grant_read_or_free(handle.id, "reader").unwrap();
        assert_eq!(readers(handle.id), vec!["reader".to_string()]);
        let handle = crate::resource::hand_off(handle, "data-browser").unwrap();
        assert_eq!(owner(handle.id).as_deref(), Some("data-browser"));
        assert!(readers(handle.id).is_empty());
    }

    /// Лог модуля ловится вместе с уровнем и подсистемой.
    #[test]
    fn the_log_is_captured() {
        install();
        abi::log(log::Level::Warn, "handlers", "что-то не так");
        assert_eq!(logged(), vec![Logged {
            level: log::Level::Warn,
            target: "handlers".to_string(),
            message: "что-то не так".to_string(),
        }]);
    }

    /// Арена переживает ответ крупнее себя и начинается заново, когда всё
    /// отпущено: адреса не растут без предела.
    #[test]
    fn the_arena_recycles_when_nothing_is_live() {
        install();
        let big = mount(vec![9; 3 * 1024 * 1024]);
        let first = abi::resource_read(big.id, 0, 3 * 1024 * 1024).unwrap();
        assert_eq!(first.len(), 3 * 1024 * 1024);
        let a = arena::alloc(16);
        arena::free(a, 16);
        let b = arena::alloc(16);
        assert_eq!(a, b, "после освобождения всего арена начинается с того же места");
        arena::free(b, 16);
    }
}
