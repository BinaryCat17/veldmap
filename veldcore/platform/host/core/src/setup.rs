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

    let log_path = host_config.runtime_dir.join(
        host_config.logs.as_deref().unwrap_or("logs/host.log")
    );

    crate::logging::init(crate::logging::Options {
        log_filter: &core_config.log_filter,
        trace_filter: &core_config.trace_filter,
        rate_limit_ms: core_config.log_rate_limit_ms,
        log_path: &log_path,
    })?;

    log::info!(target: "log", "Filter: {} (trace.log: {})", core_config.log_filter, core_config.trace_filter);
    log::info!(target: "log", "Rate limit: {}ms", core_config.log_rate_limit_ms);

    Ok(())
}

/// Initializes WGPU Adapter, Device, Queue and Surface configuration
pub async fn init_wgpu<'a>(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'a>,
    window_width: u32,
    window_height: u32,
) -> anyhow::Result<(wgpu::Adapter, Arc<wgpu::Device>, Arc<Mutex<wgpu::Queue>>, wgpu::SurfaceConfiguration, wgpu::TextureFormat)> {
    log::info!(target: "render", "Enumerating Vulkan adapters...");
    let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN).await;
    for (i, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        log::info!(target: "render", "Adapter {}: {:?} (vendor: 0x{:04X}, device: 0x{:04X})", 
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
            log::warn!(target: "render", "No discrete GPU found, trying fallback...");
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: true,
            }).await.map_err(|e| anyhow::anyhow!("Adapter error: {}", e))?
        }
    };

    log::info!(target: "render", "Selected GPU: {:?}", adapter.get_info().name);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        ..Default::default()
    }).await?;

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(Mutex::new(queue));

    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats.iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);

    let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    };

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: window_width,
        height: window_height,
        present_mode,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    
    surface.configure(&device_arc, &config);

    Ok((adapter, device_arc, queue_arc, config, surface_format))
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
) -> anyhow::Result<Arc<HostContext>> {
    let registry = Arc::new(ResourceRegistry::new());
    let memory = Arc::new(MemoryManager::new(registry.clone(), device.clone(), queue.clone()));
    let graphics = Arc::new(GraphicsDevice::new(registry.clone(), memory.clone(), device.clone(), queue.clone(), surface_format));
    let tasks = Arc::new(crate::tasks::TaskRegistry::new());
    let surfaces = Arc::new(crate::surfaces::SurfaceQueue::new());

    let dispatcher = Arc::new(Dispatcher::new());

    Ok(Arc::new(HostContext {
        dispatcher: dispatcher.clone(),
        registry,
        memory,
        graphics,
        tasks,
        surfaces,
        config,
        publisher: dispatcher,
    }))
}
