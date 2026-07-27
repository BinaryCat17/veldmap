//! Логирование платформы: один `Log` на всех, два фильтра, три получателя.
//!
//! Подсистема едет в таргете записи, и таргет всюду строится по одному
//! правилу — `veldmap::<компонент>::<подсистема>`, где компонент — это `host`
//! или имя wasm-плагина. Дальше отбирает стандартный env-фильтр.
//!
//! Префикс приписывается здесь, а не на местах. Раньше его писали руками, и
//! `target: "host"` вместо `veldmap::host::network` тихо выводил network за
//! пределы фильтра: сообщения не доходили ни до консоли, ни до файлов, причём
//! молча — отличить «не логировали» от «отфильтровали» было нечем. Теперь
//! автор пишет только подсистему (`target: "network"`), а промахнуться мимо
//! префикса нельзя: его не существует на уровне вызова. Симметрично гостевой
//! стороне — там имя плагина точно так же приписывает хост (см. abi.rs).
//!
//! Два фильтра, потому что у логов два читателя. `log_filter` — то, что видит
//! человек в консоли и в host.log; `trace_filter` — то, что остаётся в
//! trace.log на случай разбора. Раньше вторым «фильтром» была зашитая в
//! форматтер эвристика «важное = наше info или любой warn», из-за которой
//! поднять уровень одной подсистеме до debug было невозможно: запись
//! проходила фильтр и молча терялась в форматтере.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{Log, Metadata, Record};

/// Файл лога: несколько получателей на один писатель.
type Sink = Arc<Mutex<std::fs::File>>;

pub struct Logger {
    /// Консоль и host.log — то, что читают глазами.
    main: env_filter::Filter,
    /// trace.log — полный поток для разбора постфактум.
    trace: env_filter::Filter,
    /// Подавление повторов. Только для человекочитаемых получателей: в
    /// trace.log повтор — это факт, а не шум.
    dedup: Option<Mutex<Dedup>>,
    host_log: Option<Sink>,
    trace_log: Option<Sink>,
}

impl Logger {
    /// Таргет записи, приведённый к `veldmap::<компонент>::<подсистема>`.
    ///
    /// Чужие крейты не трогаем: их таргеты — их дело, и фильтровать их надо
    /// под теми именами, под которыми они пишут.
    fn target<'a>(record_target: &'a str, module_path: Option<&str>) -> std::borrow::Cow<'a, str> {
        use std::borrow::Cow;

        // Уже приведённый таргет: логи wasm-модулей (им имя плагина приписал
        // abi.rs) и записи, пришедшие сюда повторно.
        if record_target.starts_with("veldmap::") {
            return Cow::Borrowed(record_target);
        }
        // Наш крейт или чужой — видно по пути модуля, а не по таргету:
        // таргет автор задаёт как хочет, путь модуля подставляет компилятор.
        let Some(module_path) = module_path.filter(|p| p.starts_with("veldmap")) else {
            return Cow::Borrowed(record_target);
        };
        // `log` подставляет в таргет путь модуля, когда таргет не указан —
        // равенство и означает «автор подсистему не назвал». Тогда берём её
        // из пути: veldmap_host_core::plugins → plugins.
        let subsystem = if record_target == module_path {
            module_path.rsplit("::").next().unwrap_or(module_path)
        } else {
            record_target
        };
        Cow::Owned(format!("veldmap::host::{}", subsystem))
    }

    fn write(sink: &Option<Sink>, line: &str) {
        if let Some(file) = sink {
            if let Ok(mut file) = file.lock() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let target = Self::target(metadata.target(), None);
        let normalized = Metadata::builder()
            .level(metadata.level())
            .target(&target)
            .build();
        self.main.enabled(&normalized) || self.trace.enabled(&normalized)
    }

    fn log(&self, record: &Record) {
        let target = Self::target(record.target(), record.module_path());
        let normalized = Metadata::builder()
            .level(record.level())
            .target(&target)
            .build();

        let to_trace = self.trace.enabled(&normalized);
        let to_main = self.main.enabled(&normalized);
        if !to_trace && !to_main {
            return;
        }

        let message = format!("{}", record.args());
        let line = format!(
            "[{}] <{}> {:5} {}\n",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ"),
            target,
            record.level(),
            message
        );

        if to_trace {
            Self::write(&self.trace_log, &line);
        }
        if !to_main {
            return;
        }
        // Повтор гасим после trace.log: там он уже записан.
        if let Some(dedup) = &self.dedup {
            if dedup.lock().unwrap().suppress(&target, &message) {
                return;
            }
        }
        Self::write(&self.host_log, &line);
        let _ = std::io::stderr().write_all(line.as_bytes());
    }

    fn flush(&self) {
        for sink in [&self.host_log, &self.trace_log].into_iter().flatten() {
            if let Ok(mut file) = sink.lock() {
                let _ = file.flush();
            }
        }
    }
}

/// Подавление одинаковых сообщений, идущих чаще заданного интервала.
///
/// Ключ — таргет и полный текст: одинаковый текст из разных подсистем это
/// разные события, а фиксированный префикс схлопывал бы разные сообщения с
/// общим началом («Registering subscription: …» на каждый топик плагина).
struct Dedup {
    min_interval: Duration,
    seen: HashMap<(String, String), Instant>,
}

/// Потолок таблицы повторов. Ключ — текст сообщения, то есть множество ключей
/// неограниченно: за долгую сессию таблица иначе растёт без края.
const DEDUP_CAPACITY: usize = 4096;

impl Dedup {
    /// true — запись надо пропустить: такую же писали меньше интервала назад.
    fn suppress(&mut self, target: &str, message: &str) -> bool {
        let now = Instant::now();
        let key = (target.to_string(), message.to_string());
        if let Some(last) = self.seen.get(&key) {
            if now.duration_since(*last) < self.min_interval {
                return true;
            }
        }
        // Записи старше интервала уже ничего не подавляют — на них и чистим.
        if self.seen.len() >= DEDUP_CAPACITY {
            self.seen.retain(|_, last| now.duration_since(*last) < self.min_interval);
        }
        self.seen.insert(key, now);
        false
    }
}

/// Настройки логирования из core.json (см. `crate::CoreConfig`).
pub struct Options<'a> {
    pub log_filter: &'a str,
    pub trace_filter: &'a str,
    pub rate_limit_ms: u64,
    /// Основной файл; полный поток ляжет рядом под именем trace.log.
    pub log_path: &'a Path,
}

/// Ставит логгер процесса. Второй вызов ничего не делает — как и у
/// env_logger, логгер в процессе один.
pub fn init(options: Options) -> anyhow::Result<()> {
    let parse = |filters: &str| env_filter::Builder::new().parse(filters).build();
    // RUST_LOG переопределяет то, что видит человек. Полный поток остаётся
    // полным: сужать его переменной окружения незачем — он и заводился для
    // случая «в консоли не хватило».
    let main = match std::env::var("RUST_LOG") {
        Ok(value) if !value.is_empty() => parse(&value),
        _ => parse(options.log_filter),
    };
    let trace = parse(options.trace_filter);

    let trace_path = options.log_path.with_file_name("trace.log");
    if let Some(parent) = options.log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Лог предыдущего запуска не дописываем: смешанные сессии в одном файле
    // читать невозможно.
    let _ = std::fs::remove_file(options.log_path);
    let _ = std::fs::remove_file(&trace_path);

    let open = |path: &Path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(|file| Arc::new(Mutex::new(file)))
    };

    let level = main.filter().max(trace.filter());
    let logger = Logger {
        dedup: (options.rate_limit_ms > 0).then(|| {
            Mutex::new(Dedup {
                min_interval: Duration::from_millis(options.rate_limit_ms),
                seen: HashMap::new(),
            })
        }),
        main,
        trace,
        host_log: open(options.log_path),
        trace_log: open(&trace_path),
    };

    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(level);
    Ok(())
}
