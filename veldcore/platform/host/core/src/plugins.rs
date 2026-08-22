use std::fs;
use std::sync::Arc;

use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::{Actor, Dispatcher, Event};
use crate::{HostState, WasmModule, CallContext};

/// Как часто хост подкручивает эпоху движка. Это же и задержка убийства:
/// приговор wasm-инстансу приводится в исполнение на ближайшем тике, потому
/// что раньше движку просто негде проверить, не пора ли уронить вызов.
const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(10);

/// Всё, из чего собирается инстанс плагина. Хранится у актора, потому что
/// собирать приходится не только на старте: убитый инстанс поднимается заново
/// из этого же набора.
struct PluginSpec {
    engine: Engine,
    module: Module,
    linker: Linker<HostState>,
    ctx: Arc<crate::setup::HostContext>,
    instance_id: u32,
    name: String,
    init_input: Vec<u8>,
}

impl PluginSpec {
    /// Свежий инстанс: новый Store — а значит и чистое состояние модуля, —
    /// приговор для epoch-прерывания и инстанцирование.
    ///
    /// `init` сюда не входит: на первом заходе конфига ещё нет (его подбирают
    /// по имени, которое спрашивают у уже поднятого инстанса), а имя без
    /// конфига спросить можно.
    async fn instantiate(&self) -> anyhow::Result<(WasmModule, Arc<crate::tasks::Sentence>)> {
        let state = HostState {
            dispatcher: self.ctx.dispatcher.clone(),
            registry: self.ctx.registry.clone(),
            memory: self.ctx.memory.clone(),
            graphics: self.ctx.graphics.clone(),
            tasks: self.ctx.tasks.clone(),
            plugin_name: self.name.clone(),
            instance_id: self.instance_id,
            call_context: None,
            wasi: WasiCtxBuilder::new().inherit_stdout().inherit_stderr().build_p1(),
            resource_limiter: StoreLimitsBuilder::new()
                .memory_size(crate::INSTANCE_MEMORY_LIMIT as usize)
                .build(),
        };
        let mut store = Store::new(&self.engine, state);
        // Потолок памяти инстанса действует, только пока Store знает, где его
        // искать: собранный, но не выданный движку StoreLimits — мёртвое поле.
        store.limiter(|state| &mut state.resource_limiter);

        // Приговор и его исполнитель. Движок сам вызвать модуль не прервёт —
        // проверка живёт здесь и срабатывает на каждом тике эпохи; пока
        // приговора нет, дедлайн просто продлевается дальше.
        let sentence = Arc::new(crate::tasks::Sentence::new());
        let watch = sentence.clone();
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(move |_| {
            if watch.struck() {
                Err(wasmtime::Error::msg("killed"))
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });

        let instance = self.linker.instantiate_async(&mut store, &self.module).await?;
        Ok((WasmModule { store, instance }, sentence))
    }

    /// Готовый к работе инстанс: поднятый и прошедший `init`. Один путь и для
    /// загрузки, и для воскрешения убитого — разойтись им негде, потому что
    /// он один.
    async fn build(&self) -> anyhow::Result<(WasmModule, Arc<crate::tasks::Sentence>)> {
        let (mut wasm, doomed) = self.instantiate().await?;
        self.run_init(&mut wasm).await?;
        Ok((wasm, doomed))
    }

    /// Отдаёт модулю его конфиг. Экспорт необязателен: модуль без состояния
    /// может обойтись и без init.
    async fn run_init(&self, wasm: &mut WasmModule) -> anyhow::Result<()> {
        let Ok(init) = wasm.instance.get_typed_func::<(), i32>(&mut wasm.store, "init") else {
            return Ok(());
        };
        wasm.store.data_mut().call_context = Some(CallContext::new(self.init_input.clone()));
        let code = init.call_async(&mut wasm.store, ()).await;
        wasm.store.data_mut().call_context = None;
        match code {
            Ok(0) => Ok(()),
            Ok(code) => anyhow::bail!("init вернул код {}", code),
            Err(e) => anyhow::bail!("init не выполнился: {}", e),
        }
    }
}

/// Актор wasm-плагина: владеет его инстансом и доставляет события шины
/// вызовом экспорта `handle_event`. Общий цикл очереди — `Dispatcher::spawn_actor`.
struct WasmActor {
    spec: PluginSpec,
    module: WasmModule,
    /// Приговор текущему инстансу вместе с номером идущей доставки — общий с
    /// epoch-колбэком его стора (см. `tasks::Sentence`).
    sentence: Arc<crate::tasks::Sentence>,
    dispatcher: Arc<Dispatcher>,
    tasks: Arc<crate::tasks::TaskRegistry>,
}

#[async_trait::async_trait]
impl Actor for WasmActor {
    async fn deliver(&mut self, ev: Event) {
        // Приговор действует ровно на одну операцию: шаг счётчика доставок
        // его и снимает — выписанный на прошлую, он больше ни с чем не
        // совпадает (см. `tasks::Sentence`).
        self.sentence.next();

        // Учтённый запрос: отдаём платформе то, чем нас снять. Убийство —
        // это трап на ближайшем тике эпохи, посреди любой работы и без всякой
        // раскрутки: ресурсы убитого возвращает хост, а состояние модуля
        // теряется вместе со стором.
        //
        // Записи уже нет — операцию убили в окне между публикацией и этой
        // доставкой. Тогда событие не доставляется вовсе: терминальный ответ
        // за убитого уже опубликовал хост, и исполненный сейчас запрос
        // прислал бы заказчику второй конец той же операции (нативная
        // сторона это окно закрывает так же — см. util::Tasks::spawn).
        if let Some(terminal) = ev.accounted {
            let doom = crate::tasks::Doom::new(self.sentence.clone());
            if !self.tasks.arm(&ev.correlation, terminal, move |victim| victim.doomed = Some(doom)) {
                return;
            }
        }

        // handle_event() декодирует вход как EventEnvelope, восстанавливая
        // топик "{service}/{method}", поэтому событие оборачивается здесь.
        // Конверт кодирует хост, поэтому publisher достоверен: модуль может
        // авторизовать отправителя по имени.
        let req = crate::core::EventEnvelope {
            service: ev.service, method: ev.method, payload: ev.payload,
            publisher: self.dispatcher.name_of(ev.publisher).unwrap_or_default(),
            // Корреляция едет к модулю как есть: опознать по ней свой запрос
            // может только он сам.
            correlation_id: ev.correlation,
            // Адресат уже разрешён на стороне доставки (только целевой
            // подписчик получил это событие) — модулю его знать не нужно.
            target: String::new(),
        };
        let call_ctx = CallContext::new(prost::Message::encode_to_vec(&req));
        self.module.store.data_mut().call_context = Some(call_ctx);
        // Экспорта нет — инстанс не поднялся (отравленный стор после
        // неудавшегося воскрешения). Учтённую операцию надо договорить и
        // здесь: она уже на учёте, а исполнить её больше нечем.
        let Ok(handle_event) = self.module.instance.get_typed_func::<(), i32>(&mut self.module.store, "handle_event") else {
            log::error!("Модуль '{}' не отвечает: экспорта handle_event нет", self.spec.name);
            self.answer_for_lost();
            return;
        };
        if let Err(trap) = handle_event.call_async(&mut self.module.store, ()).await {
            self.revive(trap).await;
            // И после убийства тоже: снятую операцию диспетчер уже договорил и
            // с учёта снял (`Dispatcher::kill`), но инстанс убийство уносит
            // целиком — состояние у него одно на все обмены, и начатые сверх
            // убитого доводить теперь некому.
            self.answer_for_lost();
        }
    }
}

impl WasmActor {
    /// Договорить концы всех обменов, которые исполнял инстанс, которого
    /// больше нет.
    ///
    /// Терминальный ответ приходит всегда — иначе заказчик ждёт вечно, а запись
    /// об операции живёт до конца процесса. Зовётся из каждого пути, где
    /// инстанс потерян: и из трапа, и из убийства, и из «экспорта нет вовсе».
    ///
    /// Всех начатых, а не одного того, на котором упали. Инстанс уносится
    /// целиком, и вместе с состоянием пропадает всё, чем модуль помнил
    /// незакрытые обмены. Отвечать только за идущую доставку было бы достаточно
    /// лишь у модуля, который всякий запрос доводит в одном обработчике;
    /// асинхронный — спросил и отвечает уже в обработчике чужого ответа —
    /// оставил бы своих заказчиков ждать молча.
    ///
    /// Именно начатых: непочатая очередь смерть инстанса переживает, и
    /// поднявшийся ответит на неё сам (см. `TaskRegistry::abandon_by`).
    ///
    /// Уже снятое с учёта сюда не попадает по построению (`abandon_by` берёт
    /// живые записи), поэтому второго конца одной операции здесь взяться
    /// неоткуда.
    fn answer_for_lost(&self) {
        for (task, terminal) in self.tasks.abandon_by(&self.spec.name) {
            log::warn!(target: "tasks",
                "Модуль '{}' не довёл операцию {}, отвечаем за неё топиком {}",
                self.spec.name, task, terminal);
            self.dispatcher.publish_from(terminal, Vec::new(), 0, &task, "");
        }
    }

    /// Поднимает инстанс заново после трапа.
    ///
    /// Трап отравляет Store безвозвратно — продолжать в нём нельзя ни после
    /// убийства, ни после падения самого модуля. Поэтому инстанс собирается
    /// с нуля и проходит init: состояние модуля при этом теряется целиком,
    /// и это ровно то, что означает отключение электричества. Пережить его
    /// должно только то, что модуль успел положить на диск.
    ///
    /// Пересобирается инстанс, но не бинарник: `Module` уже скомпилирован и
    /// переиспользуется, поэтому цена — новый Store с чистой линейной памятью
    /// плюс init. Её и печатает лог: подниматься дорого — это про компиляцию,
    /// а её здесь нет.
    async fn revive(&mut self, trap: wasmtime::Error) {
        let started = std::time::Instant::now();
        if self.sentence.struck() {
            log::info!(target: "tasks", "Модуль '{}' снят посреди обработчика, поднимаем заново", self.spec.name);
        } else {
            log::error!("Модуль '{}' поймал трап: {:#}; поднимаем заново", self.spec.name, trap);
        }

        // Деструкторов у убитого не было — их исполняет хост. Модуль мог
        // остаться владельцем наполовину собранного ресурса, и вернуть его
        // может только тот, у кого лежит таблица владения.
        let freed = self.spec.ctx.registry.free_owned_by(self.spec.instance_id);
        if freed > 0 {
            log::info!(target: "tasks", "Возвращено ресурсов: {} — от модуля '{}'", freed, self.spec.name);
        }
        match self.spec.build().await {
            Ok((module, sentence)) => {
                self.module = module;
                self.sentence = sentence;
                log::info!(target: "tasks", "Модуль '{}' поднят заново за {:?}", self.spec.name, started.elapsed());
            }
            // Инстанс не поднялся — актор остаётся с отравленным стором, и
            // каждое следующее событие будет падать. Молчать об этом нельзя:
            // сервис фактически выбыл из системы.
            Err(e) => log::error!("Модуль '{}' не поднялся заново: {:#}", self.spec.name, e),
        }
    }
}

/// Загружает все *.wasm из `plugins_dir`. Имя каждого плагина на шине — то,
/// что он сам сообщает через `get_service_name` (единственный источник
/// истины), а не имя файла и не запись в каком-либо манифесте. Instance id
/// локальных wasm-сервисов заводит диспетчер
/// (`Dispatcher::register_instance`).
pub async fn load_services(ctx: Arc<crate::setup::HostContext>) -> anyhow::Result<()> {
    let mut config = Config::new();
    // Без этого вызов wasm нельзя прервать ничем: движок не проверяет условий
    // выхода, пока модуль не вернёт управление сам.
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;

    // Тикающая эпоха — то самое место, где движок оглядывается на приговор.
    // Один тикер на весь процесс: эпоха у движка общая, а кого ронять,
    // решает колбэк конкретного стора.
    {
        let engine = engine.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(EPOCH_TICK).await;
                engine.increment_epoch();
            }
        });
    }

    let mut wasm_files: Vec<std::path::PathBuf> = match fs::read_dir(&ctx.config.plugins_dir) {
        Ok(entries) => entries.flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
            .collect(),
        Err(e) => {
            log::warn!("Каталог модулей {:?} не читается: {}", ctx.config.plugins_dir, e);
            Vec::new()
        }
    };
    wasm_files.sort(); // для предсказуемых instance id между запусками

    for wasm_path in wasm_files {
        let wasm_bytes = fs::read(&wasm_path)?;
        let module = Module::from_binary(&engine, &wasm_bytes)?;

        // Instance id — из общего пула диспетчера: нативные сервисы уже
        // зарегистрированы (при старте, в порядке композиции раннера), wasm —
        // после них в отсортированном порядке файлов, поэтому id
        // детерминированы от запуска к запуску.
        let instance_id = ctx.dispatcher.alloc_instance_id();

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_async(&mut linker, |s: &mut HostState| &mut s.wasi)?;
        crate::abi::add_to_linker(&mut linker)?;
        linker.define_unknown_imports_as_traps(&module)?;

        // Имя и конфиг заполняются ниже, как только модуль сам себя назовёт
        // (get_service_name) — до этого их знать неоткуда и не нужно.
        let mut spec = PluginSpec {
            engine: engine.clone(),
            module,
            linker,
            ctx: ctx.clone(),
            instance_id,
            name: String::new(),
            init_input: Vec::new(),
        };

        // Первое инстанцирование — только чтобы спросить имя: конфиг для init
        // подбирается по нему, а до ответа его негде взять. Оно же проверяет,
        // что бинарник вообще поднимается.
        let (mut probe, _) = match spec.instantiate().await {
            Ok(built) => built,
            Err(e) => { log::error!("{:?}: не инстанцируется: {:#}, пропускаем", wasm_path, e); continue; }
        };

        // Спрашиваем у бинарника его имя (сгенерированный экспорт,
        // buildgen/templates/lib.rs.j2::get_service_name) — единственный
        // источник истины, имя файла тут не при чём.
        let Some(name) = call_for_output(&mut probe, "get_service_name").await
            .and_then(|out| String::from_utf8(out).ok())
            .filter(|n| !n.is_empty())
        else {
            log::error!("{:?}: экспорт get_service_name не назвал имени, пропускаем", wasm_path);
            continue;
        };

        // Дубликат имени на шине — в том числе имени нативного сервиса:
        // занятость имён знает диспетчер, отдельный учёт здесь был бы вторым
        // источником того же факта.
        if ctx.dispatcher.instance_of(&name).is_some() {
            log::error!("Имя сервиса '{}' уже занято (из {:?}), пропускаем", name, wasm_path);
            continue;
        }

        let service_config_str = match ctx.config.plugin_raw_configs.get(&name) {
            Some(s) => s.clone(),
            None => {
                // Модуль назвал себя `name`, но в config_dir нет файла `<name>.json` —
                // скорее всего схему переименовали, а конфиг забыли. Не падаем
                // (конфиг может быть модулю и не нужен), но не молчим.
                log::warn!("Модуль '{}' ({:?}): в каталоге конфигов нет '{}.json', берём пустой", name, wasm_path, name);
                "{}".to_string()
            }
        };

        // Конфиг модуля едет в init одним JSON, как он записан в файле.
        // Инъектируемых хостом ключей здесь нет: всё, что модуль узнаёт от
        // платформы, приезжает ему топиком — а значит адресно, вовремя и
        // столько раз, сколько меняется. Формат поверхности окна, например,
        // едет вместе с самой поверхностью (`core.SurfaceDelegated`).
        let init_config = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&service_config_str)
            .unwrap_or_default();

        spec.name = name.clone();
        spec.init_input = serde_json::to_vec(&serde_json::Value::Object(init_config))?;

        // Подписки спрашиваем у пробного инстанса: они свойство бинарника, а
        // не живого состояния, и после воскрешения теми же и останутся.
        let subs: Vec<String> = call_for_output(&mut probe, "get_subscriptions").await
            .and_then(|out| serde_json::from_slice(&out).ok())
            .unwrap_or_default();
        drop(probe);

        // Рабочий инстанс — уже с именем и конфигом, то есть прошедший init.
        let (module, sentence) = match spec.build().await {
            Ok(built) => built,
            Err(e) => { log::error!("Модуль '{}' не инициализировался: {:#}, пропускаем", name, e); continue; }
        };
        log::info!("Модуль '{}' поднят.", name);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        Dispatcher::spawn_actor(rx, WasmActor {
            spec,
            module,
            sentence,
            dispatcher: ctx.dispatcher.clone(),
            tasks: ctx.tasks.clone(),
        });

        ctx.dispatcher.register_instance(name.clone(), instance_id);
        for topic in subs {
            ctx.dispatcher.register_subscription(topic, name.clone(), tx.clone());
        }
    }
    Ok(())
}

/// Зовёт безаргументный экспорт и забирает то, что он положил в выход.
/// `None` — экспорта нет, он вернул код ошибки или упал.
async fn call_for_output(wasm: &mut WasmModule, export: &str) -> Option<Vec<u8>> {
    let func = wasm.instance.get_typed_func::<(), i32>(&mut wasm.store, export).ok()?;
    let call_ctx = CallContext::new(Vec::new());
    wasm.store.data_mut().call_context = Some(call_ctx.clone());
    let code = func.call_async(&mut wasm.store, ()).await;
    wasm.store.data_mut().call_context = None;
    match code {
        Ok(0) => Some(call_ctx.0.lock().unwrap().output.clone()),
        Ok(code) => { log::warn!("экспорт '{}' вернул код {}", export, code); None }
        Err(e) => { log::warn!("экспорт '{}' не выполнился: {}", export, e); None }
    }
}
