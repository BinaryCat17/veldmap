#![recursion_limit = "512"]
use std::sync::Arc;
use crate::dispatcher::Dispatcher;

pub mod registry;
pub mod format;
pub mod memory;
pub mod graphics;
pub mod config;
pub mod plugins;
pub mod setup;
pub mod abi;
pub mod dispatcher;
pub mod logging;
pub mod tasks;
pub mod places;
pub mod surfaces;

// Protobuf-типы платформенных контрактов — из сгенерированного
// биндинг-крейта (platform/host/generated, buildgen).
pub use veldmap_host_bindings::proto::{app, core};

/// Потолок памяти одного инстанса плагина. Держит две вещи сразу, и это одна
/// величина, а не две похожие: линейную память самого инстанса
/// (`Store::limiter`, см. plugins.rs) и размер байтового ресурса, который
/// инстанс просит хост завести за него (`alloc_cpu`, см. memory.rs). Модуль не
/// вправе поручить хосту держать больше, чем позволено держать ему самому.
///
/// Перебор линейной памяти отказывает `memory.grow` — аллокатор внутри модуля
/// на этом обрывается трапом, и инстанс поднимается заново с чистым
/// состоянием. Задача потолка не в том, чтобы это было красиво, а в том, чтобы
/// разросшийся модуль уносил только себя, а не хост целиком; на границы,
/// вычисленные декодерами `image-tiler`, он опирается как на предел, до
/// которого те обязаны отказать сами.
pub const INSTANCE_MEMORY_LIMIT: u64 = 1024 * 1024 * 1024;

/// Гашение хоста: рантайм вот-вот разберут.
///
/// Нужно долгожителям на blocking-пуле, и прежде всего оконному чтению
/// удалённого ресурса. Оно синхронно для гостя — вызов ABI памяти, а не задача,
/// — и отменить его снаружи нечем: система задач до этого слоя не достаёт.
/// Поэтому чтение справляется само, и ответ у него один: в сеть больше не
/// ходить, а начатый поход бросить. Именно бросить, а не доиграть: таймеры
/// рантайма разбирают раньше, чем дожидаются blocking-потоков, и опрос
/// таймера реквеста после этого — паника внутри tokio, а с `panic = "abort"` —
/// конец хоста. Поэтому кроме флага здесь уведомление: поход в сеть ждёт его
/// наравне с ответом сервера (`range::awaited` в модуле network), — и счёт
/// походов в полёте: раннер ждёт, пока брошенные вернутся, прежде чем ронять
/// рантайм ([`Shutdown::settled`]).
pub struct Shutdown {
    begun: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
    /// Походов в сеть в полёте — тех, кому объявление ещё предстоит уронить
    /// запрос или кто с ним уже возвращается.
    flying: std::sync::atomic::AtomicUsize,
}

impl Shutdown {
    pub const fn new() -> Self {
        Self {
            begun: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::const_new(),
            flying: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Объявить гашение: флаг — спрашивающим, уведомление — ждущим.
    pub fn begin(&self) {
        self.begun.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn begun(&self) -> bool {
        self.begun.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Дождаться гашения; объявленное прежде — сразу. В очередь ждущих встаёт
    /// раньше, чем смотрит на флаг: гашение, объявленное между взглядом и
    /// очередью, иначе никого бы не разбудило.
    pub async fn awaited(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.begun() {
            notified.await;
        }
    }

    /// Поход в сеть на время своей жизни: пока такие есть, [`Shutdown::settled`]
    /// ждёт.
    pub fn flight(&self) -> Flight<'_> {
        self.flying.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Flight(self)
    }

    /// Дождаться, пока походы в полёте вернутся, но не дольше `limit`; ждёт
    /// раннер — после объявления и до разборки рантайма. Объявление будит
    /// поход, но поток, вытесненный посреди опроса своего запроса, докончит
    /// опрос, когда его снова пустят, — и упрётся в таймер, если драйвер к тому
    /// времени разобран. Предел — на случай похода, чей опрос застрял: ждать
    /// его дольше, чем стоит выход, незачем.
    pub fn settled(&self, limit: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while self.flying.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        true
    }
}

/// Поход в сеть, считаемый гашением, — пока жив (см. [`Shutdown::flight`]).
pub struct Flight<'a>(&'a Shutdown);

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.flying.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

static SHUTDOWN: Shutdown = Shutdown::new();

/// Объявить гашение. Зовёт раннер — раньше, чем роняет рантайм.
pub fn begin_shutdown() {
    SHUTDOWN.begin();
}

pub fn shutting_down() -> bool {
    SHUTDOWN.begun()
}

/// Гашение хоста — тем, кто его ждёт, а не только спрашивает.
pub fn shutdown() -> &'static Shutdown {
    &SHUTDOWN
}

pub struct CallContextInner {
    pub input: Vec<u8>,
    pub output: Vec<u8>,
}

#[derive(Clone)]
pub struct CallContext(pub Arc<std::sync::Mutex<CallContextInner>>);

impl CallContext {
    pub fn new(input: Vec<u8>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(CallContextInner { input, output: Vec::new() })))
    }
}

pub struct HostState {
    pub dispatcher: Arc<Dispatcher>,
    pub registry: Arc<crate::registry::ResourceRegistry>,
    pub memory: Arc<crate::memory::MemoryManager>,
    pub graphics: Arc<crate::graphics::GraphicsDevice>,
    pub tasks: Arc<crate::tasks::TaskRegistry>,
    pub plugin_name: String,
    pub instance_id: u32,
    pub call_context: Option<CallContext>,
    pub wasi: wasmtime_wasi::p1::WasiP1Ctx,
    pub resource_limiter: wasmtime::StoreLimits,
}

pub struct WasmModule {
    pub store: wasmtime::Store<HostState>,
    pub instance: wasmtime::Instance,
}

/// Конфигурация core модуля
#[derive(serde::Deserialize, Debug)]
pub struct CoreConfig {
    /// Что видно в консоли и в host.log. Синтаксис env_logger; подсистема —
    /// это таргет записи, `veldmap::<компонент>::<подсистема>` (см. logging).
    /// Например "veldmap=info,veldmap::host::network=debug".
    /// RUST_LOG переопределяет значение отсюда.
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
    /// Что остаётся в trace.log — обычно шире предыдущего: этот файл читают,
    /// когда в консоли не хватило подробностей.
    #[serde(default = "default_trace_filter")]
    pub trace_filter: String,
    /// Минимальный интервал между одинаковыми логами в миллисекундах (0 = без
    /// ограничения). Полного потока в trace.log не касается.
    #[serde(default = "default_log_rate_limit_ms")]
    pub log_rate_limit_ms: u64,
}

fn default_log_rate_limit_ms() -> u64 {
    1000 // По умолчанию 1 секунда
}

/// Ход работы своих компонентов, чужие крейты — только warn+.
/// netlink_packet_route на каждом старте пишет бесполезное «ядро новее крейта».
fn default_log_filter() -> String {
    "veldmap=info,netlink_packet_route=error,warn".to_string()
}

/// Свои логи целиком: разбирать постфактум обычно нужно именно их.
fn default_trace_filter() -> String {
    "veldmap=trace,netlink_packet_route=error,warn".to_string()
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            log_filter: default_log_filter(),
            trace_filter: default_trace_filter(),
            log_rate_limit_ms: default_log_rate_limit_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Shutdown;

    /// Гашение будит ждущего и не теряется: спросивший после него не ждёт.
    #[tokio::test]
    async fn shutdown_reaches_those_waiting_and_those_asking_after() {
        let shutdown: &'static Shutdown = Box::leak(Box::new(Shutdown::new()));
        let waiting = tokio::spawn(shutdown.awaited());
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "до объявления ждущий ждёт");

        shutdown.begin();
        let second = std::time::Duration::from_secs(1);
        tokio::time::timeout(second, waiting).await.expect("объявление будит ждущего").expect("задача");
        tokio::time::timeout(second, shutdown.awaited()).await.expect("объявленное прежде — сразу");
        assert!(shutdown.begun());
    }

    /// Гашение ждёт походов в полёте, пока они не вернутся, но не дольше
    /// предела.
    #[test]
    fn shutdown_settles_when_the_flights_return_or_the_limit_runs_out() {
        use std::time::Duration;
        let shutdown = Shutdown::new();
        assert!(shutdown.settled(Duration::ZERO), "без походов — сразу");
        let flight = shutdown.flight();
        assert!(!shutdown.settled(Duration::from_millis(20)), "поход в полёте держит до предела");
        drop(flight);
        assert!(shutdown.settled(Duration::ZERO), "вернувшийся отпускает");
        std::thread::scope(|scope| {
            let flight = shutdown.flight();
            scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                drop(flight);
            });
            assert!(shutdown.settled(Duration::from_secs(2)), "возврат с чужого потока дожидается");
        });
    }
}
