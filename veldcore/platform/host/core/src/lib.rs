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

/// Хост гасится: рантайм вот-вот разберут.
///
/// Флаг нужен долгожителям на blocking-пуле, и прежде всего оконному чтению
/// удалённого ресурса. Оно синхронно для гостя — вызов ABI памяти, а не задача,
/// — и отменить его снаружи нечем: система задач до этого слоя не достаёт.
/// Поэтому спрашивает оно само, и ответ у него один: в сеть больше не ходить.
/// Таймеры рантайма разбирают первыми, и запрос, начатый после начала гашения,
/// паникует внутри реквеста, — а результата такого чтения всё равно уже никто
/// не ждёт.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Объявить гашение. Зовёт раннер — раньше, чем роняет рантайм.
pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn shutting_down() -> bool {
    SHUTTING_DOWN.load(std::sync::atomic::Ordering::Relaxed)
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
