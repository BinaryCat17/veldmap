use std::fs;
use std::sync::Arc;

use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::{Actor, Dispatcher, Event};
use crate::{HostState, WasmModule, CallContext};

/// Актор wasm-плагина: владеет его инстансом и доставляет события шины
/// вызовом экспорта `handle_event`. Общий цикл очереди — `Dispatcher::spawn_actor`.
struct WasmActor {
    module: WasmModule,
    dispatcher: Arc<Dispatcher>,
}

#[async_trait::async_trait]
impl Actor for WasmActor {
    async fn deliver(&mut self, ev: Event) {
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
        if let Ok(handle_event) = self.module.instance.get_typed_func::<(), i32>(&mut self.module.store, "handle_event") {
            let _ = handle_event.call_async(&mut self.module.store, ()).await;
        }
    }
}



/// Загружает все *.wasm из `plugins_dir`. Имя каждого плагина на шине — то,
/// что он сам сообщает через `get_service_name` (единственный источник
/// истины: см. [[veldmap-plugin-identity]]), а не имя файла и не запись в
/// каком-либо манифесте. Instance id локальных wasm-сервисов регистрируются
/// в диспетчере (`Dispatcher::instance_of`).
pub async fn load_services(ctx: Arc<crate::setup::HostContext>) -> anyhow::Result<()> {
    let mut config = Config::new();
    config.async_support(true);
    let engine = Engine::new(&config)?;

    let mut wasm_files: Vec<std::path::PathBuf> = match fs::read_dir(&ctx.config.plugins_dir) {
        Ok(entries) => entries.flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
            .collect(),
        Err(e) => {
            log::warn!("Cannot read plugins dir {:?}: {}", ctx.config.plugins_dir, e);
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

        let wasi = WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build_p1();

        // Имя и конфиг заполняются ниже, как только модуль сам себя назовёт
        // (get_service_name) — до этого их знать неоткуда и не нужно.
        let state = HostState {
            dispatcher: ctx.dispatcher.clone(),
            registry: ctx.registry.clone(),
            memory: ctx.memory.clone(),
            graphics: ctx.graphics.clone(),
            tasks: ctx.tasks.clone(),
            plugin_name: String::new(),
            instance_id,
            config: std::collections::HashMap::new(),
            call_context: None,
            wasi,
            resource_limiter: StoreLimitsBuilder::new().memory_size(1024 * 1024 * 1024).build(),
        };

        let mut store = Store::new(&engine, state);

        linker.define_unknown_imports_as_traps(&module)?;
        let instance = linker.instantiate_async(&mut store, &module).await?;

        // Спрашиваем у бинарника его имя (сгенерированный экспорт,
        // buildgen/templates/lib.rs.j2::get_service_name) — единственный
        // источник истины, имя файла тут не при чём.
        let Ok(get_name) = instance.get_typed_func::<(), i32>(&mut store, "get_service_name") else {
            log::error!("{:?}: no get_service_name export, skipping", wasm_path);
            continue;
        };
        let name_ctx = CallContext::new(Vec::new());
        store.data_mut().call_context = Some(name_ctx.clone());
        let name_result = get_name.call_async(&mut store, ()).await;
        store.data_mut().call_context = None;
        let name = match name_result {
            Ok(0) => {
                let out = name_ctx.0.lock().unwrap().output.clone();
                match String::from_utf8(out) {
                    Ok(n) if !n.is_empty() => n,
                    _ => { log::error!("{:?}: get_service_name returned an invalid name, skipping", wasm_path); continue; }
                }
            }
            Ok(code) => { log::error!("{:?}: get_service_name returned code {}, skipping", wasm_path, code); continue; }
            Err(e) => { log::error!("{:?}: get_service_name failed: {}, skipping", wasm_path, e); continue; }
        };

        // Дубликат имени на шине — в том числе имени нативного сервиса:
        // занятость имён знает диспетчер, отдельный учёт здесь был бы вторым
        // источником того же факта.
        if ctx.dispatcher.instance_of(&name).is_some() {
            log::error!("Duplicate service name '{}' (from {:?}), skipping", name, wasm_path);
            continue;
        }

        let service_config_str = match ctx.config.plugin_raw_configs.get(&name) {
            Some(s) => s.clone(),
            None => {
                // Модуль назвал себя `name`, но в config_dir нет файла `<name>.json` —
                // скорее всего схему переименовали, а конфиг забыли. Не падаем
                // (конфиг может быть модулю и не нужен), но не молчим.
                log::warn!("Plugin '{}' ({:?}): no '{}.json' in config dir, using empty config", name, wasm_path, name);
                "{}".to_string()
            }
        };

        let mut config_map = ctx.config.plugin_configs.get(&name)
            .cloned()
            .unwrap_or_default();

        config_map.insert("config".to_string(), serde_json::Value::String(service_config_str.clone()));
        config_map.insert("plugin_name".to_string(), serde_json::Value::String(name.clone()));

        store.data_mut().plugin_name = name.clone();
        store.data_mut().config = config_map;

        // Конфиг уезжает в HostState выше; модуль читает его через
        // ABI-вызов veld_get_config — сервис-посредник не нужен.
        log::trace!("Loading service '{}' with instance_id {}", name, instance_id);

        // Call init if it exists
        if let Ok(init_func) = instance.get_typed_func::<(), i32>(&mut store, "init") {
            log::trace!("Calling init for plugin '{}'...", name);

            // Инъектируемые хостом ключи едут в init тем же JSON, что и конфиг
            // из файла: один канал, и типизированный Config модуля видит всё
            // (адресный veld_get_config для этого не нужен).
            let mut init_config = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&service_config_str)
                .unwrap_or_default();
            init_config.insert("surface_format".to_string(), ctx.graphics.get_surface_format_proto().into());
            let init_input = serde_json::to_vec(&serde_json::Value::Object(init_config))?;
            let call_ctx = CallContext::new(init_input);
            store.data_mut().call_context = Some(call_ctx);

            match init_func.call_async(&mut store, ()).await {
                Ok(0) => log::info!("Plugin '{}' initialized successfully.", name),
                Ok(code) => {
                    log::error!("Plugin '{}' failed to initialize with code: {}", name, code);
                    continue;
                }
                Err(e) => {
                    log::error!("Error while calling init for '{}': {}", name, e);
                    continue;
                }
            }
            // Reset call context after init
            store.data_mut().call_context = None;
        }

        let mut wasm_module = WasmModule { store, instance };

        // Extract subscriptions
        let mut subs: Vec<String> = Vec::new();
        if let Ok(get_subs) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "get_subscriptions") {
            let call_ctx = CallContext::new(Vec::new());
            wasm_module.store.data_mut().call_context = Some(call_ctx.clone());
            match get_subs.call_async(&mut wasm_module.store, ()).await {
                Ok(0) => {
                    let out = {
                        let inner = call_ctx.0.lock().unwrap();
                        inner.output.clone()
                    };
                    if let Ok(topics) = serde_json::from_slice::<Vec<String>>(&out) {
                        subs = topics;
                    }
                }
                Ok(code) => log::warn!("Plugin '{}' get_subscriptions returned code: {}", name, code),
                Err(e) => log::warn!("Plugin '{}' get_subscriptions failed: {}", name, e),
            }
            wasm_module.store.data_mut().call_context = None;
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

        Dispatcher::spawn_actor(rx, WasmActor {
            module: wasm_module,
            dispatcher: ctx.dispatcher.clone(),
        });

        ctx.dispatcher.register_instance(name.clone(), instance_id);
        for topic in subs {
            ctx.dispatcher.register_subscription(topic, name.clone(), tx.clone());
        }
    }
    Ok(())
}
