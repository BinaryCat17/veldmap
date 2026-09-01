use std::sync::{Arc, Mutex};
use crate::dispatcher::Dispatcher;
use crate::registry::ResourceRegistry;
use crate::memory::MemoryManager;
use crate::graphics::GraphicsDevice;

pub fn init_logging(config_dir: &str, host_config: &crate::config::HostConfig) -> anyhow::Result<()> {
    // Конфиг нужен до инициализации логгера: из него берутся фильтры.
    let core_config: crate::CoreConfig =
        crate::config::load_config_with_path::<crate::CoreConfig, _>(&format!("{}/core.json", config_dir))
            .unwrap_or_default();

    let log_path = host_config.log_path();

    crate::logging::init(crate::logging::Options {
        log_filter: &core_config.log_filter,
        trace_filter: &core_config.trace_filter,
        rate_limit_ms: core_config.log_rate_limit_ms,
        log_path: &log_path,
    })?;

    log::info!(target: "log", "Фильтр: {} (trace.log: {})", core_config.log_filter, core_config.trace_filter);
    log::info!(target: "log", "Подавление повторов: {} мс", core_config.log_rate_limit_ms);

    Ok(())
}

/// Поднимает графику: адаптер, устройство, очередь, настройку поверхности и
/// ловушку отказов видеокарты. Ловушка выходит отсюда потому, что здесь ставят
/// обработчик ошибок, — а нужна она выделяющему.
pub async fn init_wgpu<'a>(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'a>,
    window_width: u32,
    window_height: u32,
) -> anyhow::Result<(
    wgpu::Adapter,
    Arc<wgpu::Device>,
    Arc<Mutex<wgpu::Queue>>,
    wgpu::SurfaceConfiguration,
    wgpu::TextureFormat,
    Arc<crate::memory::GpuFaults>,
)> {
    // Все основные бэкенды, а не один Vulkan: под Windows аппаратный адаптер
    // приходит через DX12, под macOS — через Metal, и перебор одного лишь
    // Vulkan не нашёл бы там ничего вовсе. Тогда сработал бы запасной путь ниже,
    // и единственным режимом на этих платформах стал бы программный растеризатор.
    log::info!(target: "render", "Перебор графических адаптеров...");
    let adapters = instance.enumerate_adapters(wgpu::Backends::PRIMARY).await;
    for (i, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        log::info!(target: "render", "Адаптер {}: {:?} (вендор 0x{:04X}, устройство 0x{:04X})", 
            i, info.name, info.vendor, info.device);
    }
    
    let mut adapter = None;
    for a in adapters {
        let info = a.get_info();
        if !info.name.to_lowercase().contains("llvmpipe") && 
           !info.name.to_lowercase().contains("software") &&
           info.vendor != 0x1414 { 
            adapter = Some(a);
            break;
        }
    }
    
    let adapter = match adapter {
        Some(a) => a,
        None => {
            log::warn!(target: "render", "Аппаратного адаптера не нашлось — берём программный");
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: true,
                ..Default::default()
            }).await.map_err(|e| anyhow::anyhow!("адаптер не выдан: {}", e))?
        }
    };

    log::info!(target: "render", "Выбран адаптер: {:?}", adapter.get_info().name);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        ..Default::default()
    }).await?;

    let device_arc = Arc::new(device);
    // Ошибка валидации wgpu по умолчанию роняет процесс. Всё предсказуемое
    // хост отсекает заранее (лимиты текстур, роли аттачментов, порядок команд),
    // но исчерпывающим этот список не бывает — а обещание изоляции держит
    // именно хост: модуль после ошибки трапается и воскресает, хосту же
    // падать из-за бага одного модуля нельзя. Поэтому неперехваченное
    // становится строкой в логе, а не паникой кадрового цикла.
    //
    // Отказы выделения сюда не доходят: их ловит на месте тот, кто выделял
    // (`memory::watched`), — иначе модуль получил бы живой номер битого
    // ресурса. Здесь остаётся всё прочее: рисование, шейдеры, раскладки.
    device_arc.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        log::error!(target: "render", "wgpu: {}", error);
    }));

    // Потеря устройства областью ошибок не ловится вовсе — у неё свой обратный
    // вызов, и без него выделения после потери снова молча отдавали бы живые
    // номера битых ресурсов.
    let faults = Arc::new(crate::memory::GpuFaults::default());
    let losing = faults.clone();
    device_arc.set_device_lost_callback(move |reason, message| {
        log::error!(target: "render", "wgpu: устройство потеряно ({:?}): {}", reason, message);
        losing.lose(format!("устройство потеряно: {}", message));
    });
    let queue_arc = Arc::new(Mutex::new(queue));

    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats.iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);

    // Ожидание вертикальной синхронизации — оно же и темп всего приложения:
    // кадровый цикл раннера упирается в `get_current_texture`, а из его витков
    // растут кадровые тики модулей.
    //
    // Без него цикл крутится со скоростью GPU (на этой машине — за две сотни
    // кадров в секунду при экране в 60), и лишние кадры не просто рисуются
    // впустую. Тик — событие шины, очередь подписчика не ограничена, и модуль,
    // которому кадр обходится дороже витка цикла, отстаёт безвозвратно:
    // очередь растёт, а с ней и задержка между нажатием и увиденным.
    // Ограничитель здесь ровно один, и это экран.
    let present_mode = wgpu::PresentMode::Fifo;

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        // Auto — единственное значение, работающее для любого формата из
        // `caps.formats`; широкий охват и HDR потребовали бы другого
        // кодирования на выходе, чего шейдеры модулей не делают.
        color_space: wgpu::SurfaceColorSpace::Auto,
        width: window_width,
        height: window_height,
        present_mode,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    
    surface.configure(&device_arc, &config);

    Ok((adapter, device_arc, queue_arc, config, surface_format, faults))
}

#[derive(Clone)]
pub struct HostContext {
    pub dispatcher: Arc<Dispatcher>,
    pub registry: Arc<ResourceRegistry>,
    pub memory: Arc<MemoryManager>,
    pub graphics: Arc<GraphicsDevice>,
    pub tasks: Arc<crate::tasks::TaskRegistry>,
    /// Поверхности окон: пишет модуль app, дренирует кадровый цикл раннера.
    /// Живёт в контексте, а не в State модуля, именно потому, что у неё два
    /// потребителя по разные стороны шины.
    pub surfaces: Arc<crate::surfaces::SurfaceQueue>,
    /// Найденные места элементов: пишет модуль app, дренирует кадровый цикл
    /// раннера. Живёт здесь потому же, почему и поверхности, — у неё два
    /// потребителя по разные стороны шины. В отличие от них, наполняется она
    /// только на прогоне по сценарию: обычный запуск вопросов не задаёт.
    pub places: Arc<crate::places::PlaceQueue>,
    pub config: Arc<crate::config::HostConfig>,
    /// Паблишер шины для emit-стабов. В базовом контексте — хост (id 0);
    /// каждый нативный сервис получает клон контекста со своей идентичностью
    /// (см. `for_service`) — как HostState.instance_id у wasm.
    pub publisher: Arc<dyn veldmap_host_bindings::Publisher + Send + Sync>,
}

impl HostContext {
    /// Контекст нативного сервиса: регистрирует его имя на шине (instance id
    /// из общего пула) и возвращает клон с паблишером, штампующим его
    /// идентичность. Вызывается клеем сервиса до сборки его State — поэтому
    /// подписка на топики (`subscribe_named`) идёт отдельным шагом, уже с
    /// готовым сервисом.
    pub fn for_service(self: &Arc<Self>, name: &str) -> Arc<Self> {
        let id = self.dispatcher.alloc_instance_id();
        self.dispatcher.register_instance(name.to_string(), id);
        let publisher: Arc<dyn veldmap_host_bindings::Publisher + Send + Sync> =
            Arc::new(self.dispatcher.publisher_for(id));
        Arc::new(Self { publisher, ..(**self).clone() })
    }
}

pub async fn init_core_services(
    device: Arc<wgpu::Device>,
    queue: Arc<Mutex<wgpu::Queue>>,
    surface_format: wgpu::TextureFormat,
    config: Arc<crate::config::HostConfig>,
    faults: Arc<crate::memory::GpuFaults>,
) -> anyhow::Result<Arc<HostContext>> {
    let registry = Arc::new(ResourceRegistry::new());
    let memory = Arc::new(MemoryManager::new(registry.clone(), device.clone(), queue.clone(), faults));
    let graphics = Arc::new(GraphicsDevice::new(registry.clone(), memory.clone(), device.clone(), surface_format));
    let tasks = Arc::new(crate::tasks::TaskRegistry::new());
    let surfaces = Arc::new(crate::surfaces::SurfaceQueue::new());
    let places = Arc::new(crate::places::PlaceQueue::new());

    // Диспетчер ведёт учёт операций в полёте, поэтому реестр задач создаётся
    // до него и разделяется с контекстом: из него же убивает ABI.
    let dispatcher = Arc::new(Dispatcher::new(tasks.clone()));

    Ok(Arc::new(HostContext {
        dispatcher: dispatcher.clone(),
        registry,
        memory,
        graphics,
        tasks,
        surfaces,
        places,
        config,
        publisher: dispatcher,
    }))
}
