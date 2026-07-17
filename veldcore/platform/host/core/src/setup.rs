use std::sync::{Arc, Mutex};
use crate::dispatcher::Dispatcher;
use crate::registry::ResourceRegistry;
use crate::memory::MemoryManager;
use crate::graphics::GraphicsDevice;
use std::io::Write;

pub fn init_logging(config_dir: &str, host_config: &crate::config::HostConfig) -> anyhow::Result<()> {
    let mut log_path = std::path::PathBuf::from("logs/host.log");
    
    if let Some(logs) = &host_config.manifest.logs {
        log_path = std::path::PathBuf::from(logs);
    }

    let final_log_path = host_config.project_root.join(log_path);

    if let Some(parent) = final_log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&final_log_path);
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&final_log_path)
        .ok();
    let log_file: Option<Arc<Mutex<std::fs::File>>> = log_file.map(|f| Arc::new(Mutex::new(f)));

    let file_log = log_file.clone();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("veldmap=trace,veldmap_vlog=trace,info"))
        .format(move |buf, record| {
            let log_line = format!(
                "[{}] <{}> {}\n",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ"),
                record.target(),
                record.args()
            );

            if let Some(file) = &file_log {
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(log_line.as_bytes());
                }
            }

            let is_veldmap = record.target().starts_with("veldmap");
            let is_info = record.level() == log::Level::Info;
            let is_warn_or_error = record.level() == log::Level::Warn || record.level() == log::Level::Error;
            
            if (is_veldmap && is_info) || is_warn_or_error {
                write!(buf, "{}", log_line)
            } else {
                Ok(())
            }
        })
        .init();

    let core_config: crate::CoreConfig = 
        crate::config::load_config_with_path::<crate::CoreConfig, _>(&format!("{}/core.json", config_dir))
            .unwrap_or_default();
    
    crate::logging::init_logging(core_config.log_flags);
    crate::logging::init_rate_limiting(core_config.log_rate_limit_ms);
    
    crate::vinfo!(crate::logging::FLAG_HOST_RENDER, "Log flags: 0b{:b}", core_config.log_flags);
    crate::vinfo!(crate::logging::FLAG_HOST_RENDER, "Log rate limit: {}ms", core_config.log_rate_limit_ms);

    Ok(())
}

/// Initializes WGPU Adapter, Device, Queue and Surface configuration
pub async fn init_wgpu<'a>(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'a>,
    window_width: u32,
    window_height: u32,
) -> anyhow::Result<(wgpu::Adapter, Arc<wgpu::Device>, Arc<Mutex<wgpu::Queue>>, wgpu::SurfaceConfiguration, wgpu::TextureFormat)> {
    crate::vinfo!(crate::logging::FLAG_HOST_RENDER, "Enumerating Vulkan adapters...");
    let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN).await;
    for (i, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        crate::vinfo!(crate::logging::FLAG_HOST_RENDER, "Adapter {}: {:?} (vendor: 0x{:04X}, device: 0x{:04X})", 
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
            crate::vwarn!(crate::logging::FLAG_HOST_RENDER, "No discrete GPU found, trying fallback...");
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: true,
            }).await.map_err(|e| anyhow::anyhow!("Adapter error: {}", e))?
        }
    };

    crate::vinfo!(crate::logging::FLAG_HOST_RENDER, "Selected GPU: {:?}", adapter.get_info().name);

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

pub struct HostContext {
    pub dispatcher: Arc<Dispatcher>,
    pub registry: Arc<ResourceRegistry>,
    pub memory: Arc<MemoryManager>,
    pub graphics: Arc<GraphicsDevice>,
    pub endpoint: iroh::Endpoint,
    pub config: Arc<crate::config::HostConfig>,
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
    
    let mut rng = rand::rng();
    let secret_key = iroh::SecretKey::generate(&mut rng);
    let endpoint = iroh::Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![b"veldmap/rpc/1".to_vec()])
        .bind()
        .await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));

    Ok(Arc::new(HostContext {
        dispatcher,
        registry,
        memory,
        graphics,
        endpoint,
        config,
    }))
}
