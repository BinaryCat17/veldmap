use std::fs;
use std::sync::Arc;

use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::Event;
use crate::{HostState, WasmModule, CallContext};



use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_INSTANCE_ID: AtomicU32 = AtomicU32::new(100); // Local plugins start from 100



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

    let mut loaded_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for wasm_path in wasm_files {
        let wasm_bytes = fs::read(&wasm_path)?;
        let module = Module::from_binary(&engine, &wasm_bytes)?;

        let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::SeqCst);

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

        if !loaded_names.insert(name.clone()) {
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
        config_map.insert("surface_format".to_string(), serde_json::Value::Number(ctx.graphics.get_surface_format_proto().into()));

        store.data_mut().plugin_name = name.clone();
        store.data_mut().config = config_map;

        // Конфиг уезжает в HostState выше; модуль читает его через
        // ABI-вызов veld_get_config — сервис-посредник не нужен.
        log::trace!("Loading service '{}' with instance_id {}", name, instance_id);

        // Call init if it exists
        if let Ok(init_func) = instance.get_typed_func::<(), i32>(&mut store, "init") {
            log::trace!("Calling init for plugin '{}'...", name);

            let init_input = service_config_str.as_bytes().to_vec();
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

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

        let plugin_name_clone = name.clone();
        let dispatcher_for_actor = ctx.dispatcher.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                // handle_event() decodes its input as an EventEnvelope to recover the
                // "{service}/{method}" topic, so the event is wrapped here.
                // Конверт кодирует хост, поэтому publisher достоверен:
                // модуль может авторизовать отправителя по имени.
                let req = crate::core::EventEnvelope {
                    service: ev.service, method: ev.method, payload: ev.payload,
                    publisher: dispatcher_for_actor.name_of(ev.publisher).unwrap_or_default(),
                    // Адресат уже разрешён на стороне доставки (только целевой
                    // подписчик получил это событие) — модулю его знать не нужно.
                    target: String::new(),
                };
                let call_ctx = CallContext::new(prost::Message::encode_to_vec(&req));
                wasm_module.store.data_mut().call_context = Some(call_ctx);
                if let Ok(handle_event) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "handle_event") {
                    let _ = handle_event.call_async(&mut wasm_module.store, ()).await;
                }
            }
            log::info!("Plugin '{}' actor channel closed, shutting down.", plugin_name_clone);
        });

        ctx.dispatcher.register_instance(name.clone(), instance_id);
        for topic in subs {
            ctx.dispatcher.register_subscription(topic, Some(name.clone()), tx.clone());
        }
    }
    Ok(())
}
