use crate::dispatcher::NativeService;
use crate::resources::{ResourceManager, Resource};
use crate::core::{
    FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, 
    FsDownloadRequest, TaskStatusRequest, TaskStatusResponse,
    TaskCancelRequest, ResourceHandle,
    ImageInfoRequest, ImageInfoResponse, ImageLoadRequest,
    GetResourceRequest, GetResourceResponse, CreateDataRequest, CreateDataResponse,
    HttpTaskRequest, HttpTaskResponse
};
use prost::Message;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::ColorType;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct SystemService {
    tasks: Arc<Mutex<HashMap<String, crate::dispatcher::TaskState>>>,
    resources: Arc<ResourceManager>,
}

impl SystemService {
    pub fn new(resources: Arc<ResourceManager>, tasks: Arc<Mutex<HashMap<String, crate::dispatcher::TaskState>>>) -> Self {
        Self {
            tasks,
            resources,
        }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "image_info" => {
                let req = ImageInfoRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                
                match image::image_dimensions(&req.path) {
                    Ok((w, h)) => {
                        Ok(ImageInfoResponse { 
                            width: w, height: h, channels: 4, error: String::new() 
                        }.encode_to_vec())
                    }
                    Err(e) => {
                        Ok(ImageInfoResponse { 
                            width: 0, height: 0, channels: 0, error: e.to_string() 
                        }.encode_to_vec())
                    }
                }
            }
            "image_load" => {
                let req = ImageLoadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }

                let task_id = uuid::Uuid::new_v4().to_string();
                
                {
                    let mut tasks = self.tasks.lock().unwrap();
                    tasks.insert(task_id.clone(), crate::dispatcher::TaskState { 
                        progress: 0.0, 
                        completed: false, 
                        error: String::new(),
                        abort_handle: None,
                        result_handle: None,
                        payload: Vec::new(),
                    });
                }
                
                let tasks_clone = self.tasks.clone();
                let resources = self.resources.clone();
                let path = req.path.clone();
                let task_id_inner = task_id.clone();
                
                let join_handle = tokio::task::spawn_blocking(move || {
                    let update_status = |progress: f32, err: String, handle: Option<ResourceHandle>| {
                        let mut tasks = tasks_clone.lock().unwrap();
                        if let Some(t) = tasks.get_mut(&task_id_inner) {
                            t.progress = progress;
                            if !err.is_empty() {
                                t.error = err;
                                t.completed = true;
                            }
                            if handle.is_some() {
                                t.result_handle = handle;
                                t.completed = true;
                                t.progress = 1.0;
                            }
                        }
                    };

                    // Load and decode using appropriate library
                    let path_lower = path.to_lowercase();
                    let is_tiff = path_lower.ends_with(".tif") || path_lower.ends_with(".tiff");

                    let rgba = if is_tiff {
                        let file = match std::fs::File::open(&path) {
                            Ok(f) => f,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        let buf_reader = std::io::BufReader::with_capacity(1024 * 1024, file);
                        let mut decoder = match Decoder::new(buf_reader) {
                            Ok(d) => d,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        
                        let (width, height) = match decoder.dimensions() {
                            Ok(d) => d,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        
                        let color_type = match decoder.colortype() {
                            Ok(ct) => ct,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };

                        let planar_config = decoder.get_tag_u32(tiff::tags::Tag::PlanarConfiguration).unwrap_or(1);
                        let samples_per_pixel = match color_type {
                            ColorType::Gray(_) => 1,
                            ColorType::RGB(_) => 3,
                            ColorType::RGBA(_) => 4,
                            _ => 1,
                        };

                        log::info!(target: "host", "Loading TIFF: {}x{} {:?} (Planar: {}, Samples: {})", 
                            width, height, color_type, planar_config, samples_per_pixel);
                        update_status(0.2, String::new(), None);

                        let img_res = decoder.read_image();
                        update_status(0.5, String::new(), None);

                        match img_res {
                            Ok(res) => {
                                let mut rgba8 = image::RgbaImage::new(width, height);
                                
                                // Helper to convert any DecodingResult to a normalized f32 buffer
                                let data_f32: Vec<f32> = match res {
                                    DecodingResult::U8(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::I8(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
                                    DecodingResult::F32(v) => v,
                                    DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
                                    _ => {
                                        log::warn!(target: "host", "Unhandled TIFF DecodingResult variant. Falling back.");
                                        let fallback_img = match image::open(&path) {
                                            Ok(i) => i,
                                            Err(fe) => { update_status(0.0, format!("Fallback error: {}", fe), None); return; }
                                        };
                                        let rgba8 = fallback_img.to_rgba8();
                                        // Since we can't easily break or return a different type from here, 
                                        // we'll just finish the task with the fallback image right now.
                                        let (w, h) = rgba8.dimensions();
                                        let tex_id = resources.create_texture(w, h, 0, 8, false, requestor_id);
                                        if let Err(e) = resources.write_resource(tex_id, 0, &rgba8, requestor_id) {
                                            update_status(0.0, e.to_string(), None);
                                            return;
                                        }
                                        let handle = ResourceHandle {
                                            id: tex_id,
                                            size: (w * h * 4) as u64,
                                            content_hash: resources.compute_hash(tex_id, requestor_id).unwrap_or_default(),
                                        };
                                        update_status(1.0, String::new(), Some(handle));
                                        return;
                                    }
                                };

                                let mut min_val = f32::MAX;
                                let mut max_val = f32::MIN;
                                
                                // Calculate min/max with outlier rejection
                                for &v in &data_f32 {
                                    if f32::is_finite(v) && v != 0.0 && v != -9999.0 && v != 65535.0 {
                                        if v < min_val { min_val = v; }
                                        if v > max_val { max_val = v; }
                                    }
                                }
                                if min_val >= max_val { min_val = 0.0; max_val = 1.0; }
                                let range = (max_val - min_val).max(0.000001);

                                let get_s = |c: usize, x: u32, y: u32| -> f32 {
                                    let idx = y as usize * width as usize + x as usize;
                                    // Planar configuration 2 means channels are in separate chunks
                                    if planar_config == 2 && samples_per_pixel > 1 {
                                        let p_size = width as usize * height as usize;
                                        data_f32[c * p_size + idx]
                                    } else {
                                        data_f32[idx * samples_per_pixel + c]
                                    }
                                };

                                for y in 0..height {
                                    for x in 0..width {
                                        let r_v = get_s(0, x, y);
                                        let r = (((r_v - min_val) / range) * 255.0).clamp(0.0, 255.0) as u8;
                                        let (g, b) = if samples_per_pixel >= 3 {
                                            ((((get_s(1, x, y) - min_val) / range) * 255.0).clamp(0.0, 255.0) as u8,
                                             (((get_s(2, x, y) - min_val) / range) * 255.0).clamp(0.0, 255.0) as u8)
                                        } else { (r, r) };
                                        let a = if samples_per_pixel == 4 { 
                                            (((get_s(3, x, y) - min_val) / range) * 255.0).clamp(0.0, 255.0) as u8 
                                        } else { 255 };
                                        rgba8.put_pixel(x, y, image::Rgba([r, g, b, a]));
                                    }
                                }
                                rgba8
                            }
                            Err(e) => {
                                log::warn!(target: "host", "Specialized TIFF decoder failed: {:?}. Falling back to standard image crate.", e);
                                let fallback_img = match image::open(&path) {
                                    Ok(i) => i,
                                    Err(fe) => { update_status(0.0, format!("TIFF error: {:?}, Fallback error: {}", e, fe), None); return; }
                                };
                                fallback_img.to_rgba8()
                            }
                        }
                    } else {
                        // Use standard image library for other formats (PNG, JPG)
                        let img = match image::open(&path) {
                            Ok(i) => i,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        update_status(0.3, String::new(), None);

                        let final_img = if req.target_width > 0 || req.target_height > 0 {
                            let tw = if req.target_width == 0 { img.width() } else { req.target_width };
                            let th = if req.target_height == 0 { img.height() } else { req.target_height };
                            img.thumbnail(tw, th)
                        } else {
                            img
                        };
                        
                        let mut rgba8 = final_img.to_rgba8();
                        let mut min_val = 255u8;
                        let mut max_val = 0u8;
                        let mut has_data = false;
                        
                        for pixel in rgba8.pixels() {
                            if pixel[3] > 0 {
                                let v = pixel[0].max(pixel[1]).max(pixel[2]);
                                if v > 0 && v < 255 {
                                    min_val = min_val.min(pixel[0]).min(pixel[1]).min(pixel[2]);
                                    max_val = max_val.max(pixel[0]).max(pixel[1]).max(pixel[2]);
                                    has_data = true;
                                }
                            }
                        }
                        
                        if has_data && max_val > min_val {
                            let range = (max_val as f32 - min_val as f32).max(1.0);
                            for pixel in rgba8.pixels_mut() {
                                if pixel[3] > 0 {
                                    pixel[0] = (((pixel[0] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8;
                                    pixel[1] = (((pixel[1] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8;
                                    pixel[2] = (((pixel[2] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8;
                                }
                            }
                        }
                        rgba8
                    };
                    update_status(0.8, String::new(), None);

                    let (w, h) = rgba.dimensions();
                    // Resize TIFF if targets were specified (since decoder didn't do it)
                    let final_rgba = if is_tiff && (req.target_width > 0 || req.target_height > 0) {
                        let tw = if req.target_width == 0 { w } else { req.target_width };
                        let th = if req.target_height == 0 { h } else { req.target_height };
                        image::DynamicImage::ImageRgba8(rgba).thumbnail(tw, th).to_rgba8()
                    } else {
                        rgba
                    };

                    let (w, h) = final_rgba.dimensions();
                    // Upload to GPU
                    let tex_id = resources.create_texture(w, h, 0, 8, false, requestor_id); // 8 = TEXTURE_BINDING
                    if let Err(e) = resources.write_resource(tex_id, 0, &final_rgba, requestor_id) {
                        update_status(0.0, e.to_string(), None);
                        return;
                    }

                    let handle = ResourceHandle {
                        id: tex_id,
                        size: (w * h * 4) as u64,
                        content_hash: resources.compute_hash(tex_id, requestor_id).unwrap_or_default(),
                    };
                    update_status(1.0, String::new(), Some(handle));
                });

                {
                    let mut tasks = self.tasks.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id) {
                        t.abort_handle = Some(join_handle.abort_handle());
                    }
                }

                Ok(crate::core::TaskResponse { task_id }.encode_to_vec())
            }
            "get_resource" => {
                let req = GetResourceRequest::decode(&payload[..])?;
                if let Some(id) = self.resources.get_named_resource(&req.name) {
                    if let Some(res) = self.resources.get_resource(id, requestor_id) {
                        let mut handle = ResourceHandle { id, ..Default::default() };
                        match res {
                            Resource::Data(v) => { handle.size = v.len() as u64; }
                            Resource::Buffer(b) => { handle.size = b.size(); }
                            Resource::Texture { width, height, .. } => { handle.size = (width * height * 4) as u64; }
                            _ => {}
                        }
                        Ok(GetResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
                    } else {
                        Ok(GetResourceResponse { handle: None, error: "Resource found in registry but not in storage or unauthorized".into() }.encode_to_vec())
                    }
                } else {
                    Ok(GetResourceResponse { handle: None, error: format!("Resource '{}' not found", req.name) }.encode_to_vec())
                }
            }
            "create_data" => {
                let req = CreateDataRequest::decode(&payload[..])?;
                let id = self.resources.create_data_resource(vec![0u8; req.size as usize], requestor_id);
                let handle = ResourceHandle {
                    id,
                    size: req.size,
                    content_hash: Vec::new(),
                };
                Ok(CreateDataResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            "fs_read" => {
                let req = FsReadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                
                let data = fs::read(&req.path)?;
                let size = data.len() as u64;
                let id = self.resources.create_data_resource(data, requestor_id);
                
                let handle = ResourceHandle {
                    id,
                    size,
                    content_hash: self.resources.compute_hash(id, requestor_id).unwrap_or_default(),
                };
                Ok(FsReadResponse { handle: Some(handle) }.encode_to_vec())
            }
            "fs_write" => {
                let req = FsWriteRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let handle = req.handle.ok_or_else(|| anyhow::anyhow!("Missing handle"))?;
                
                let data = if handle.id == 0 {
                    return Err(anyhow::anyhow!("Handle ID 0 not supported for fs_write yet"));
                } else {
                    self.resources.read_resource(handle.id, 0, handle.size, requestor_id)?
                };

                if let Some(parent) = Path::new(&req.path).parent() { fs::create_dir_all(parent)?; }
                fs::write(&req.path, &data)?;
                Ok(Vec::new())
            }
            "fs_download" => {
                let req = FsDownloadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }

                let task_id = uuid::Uuid::new_v4().to_string();

                let tasks_clone = self.tasks.clone();
                if let Some(parent) = Path::new(&req.path).parent() { fs::create_dir_all(parent)?; }

                {
                    let mut tasks = self.tasks.lock().unwrap();
                    tasks.insert(task_id.clone(), crate::dispatcher::TaskState { 
                        progress: 0.0, 
                        completed: false, 
                        error: String::new(),
                        abort_handle: None,
                        result_handle: None,
                        payload: Vec::new(),
                    });
                }

                let task_id_inner = task_id.clone();
                let join_handle = tokio::spawn(async move {                    let client = reqwest::Client::new();
                    let mut builder = client.get(&req.url);
                    for (key, value) in req.headers { builder = builder.header(key, value); }
                    
                    let res = match builder.send().await {
                        Ok(r) => r,
                        Err(e) => {
                            let mut tasks = tasks_clone.lock().unwrap();
                            if let Some(t) = tasks.get_mut(&task_id_inner) {
                                t.error = e.to_string();
                                t.completed = true;
                            }
                            return;
                        }
                    };

                    if !res.status().is_success() {
                        let mut tasks = tasks_clone.lock().unwrap();
                        if let Some(t) = tasks.get_mut(&task_id_inner) {
                            t.error = format!("HTTP {}", res.status());
                            t.completed = true;
                        }
                        return;
                    }

                    let total_size = res.content_length().unwrap_or(0);
                    let mut downloaded: u64 = 0;
                    let mut stream = res.bytes_stream();
                    
                    match tokio::fs::File::create(&req.path).await {
                        Ok(mut async_file) => {
                            while let Some(chunk_res) = stream.next().await {
                                match chunk_res {
                                    Ok(chunk) => {
                                        if let Err(e) = async_file.write_all(&chunk).await {
                                            let mut tasks = tasks_clone.lock().unwrap();
                                            if let Some(t) = tasks.get_mut(&task_id_inner) {
                                                t.error = format!("Write error: {}", e);
                                                t.completed = true;
                                            }
                                            return;
                                        }
                                        downloaded += chunk.len() as u64;
                                        if total_size > 0 {
                                            let mut tasks = tasks_clone.lock().unwrap();
                                            if let Some(t) = tasks.get_mut(&task_id_inner) {
                                                t.progress = downloaded as f32 / total_size as f32;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let mut tasks = tasks_clone.lock().unwrap();
                                        if let Some(t) = tasks.get_mut(&task_id_inner) {
                                            t.error = format!("Stream error: {}", e);
                                            t.completed = true;
                                        }
                                        return;
                                    }
                                }
                            }
                            let _ = async_file.flush().await;
                        }
                        Err(e) => {
                            let mut tasks = tasks_clone.lock().unwrap();
                            if let Some(t) = tasks.get_mut(&task_id_inner) {
                                t.error = format!("File create error: {}", e);
                                t.completed = true;
                            }
                            return;
                        }
                    }

                    let mut tasks = tasks_clone.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                        t.progress = 1.0;
                        t.completed = true;
                    }
                });

                {
                    let mut tasks = self.tasks.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id) {
                        t.abort_handle = Some(join_handle.abort_handle());
                    }
                }

                Ok(crate::core::TaskResponse { task_id }.encode_to_vec())
            }
            "http" => {
                let req = HttpTaskRequest::decode(&payload[..])?;
                let task_id = uuid::Uuid::new_v4().to_string();
                let tasks_clone = self.tasks.clone();
                let task_id_inner = task_id.clone();
                
                log::debug!(target: "host", "Received HTTP request: {} {} (Task ID: {})", req.method, req.url, task_id);

                {
                    let mut tasks = self.tasks.lock().unwrap();
                    tasks.insert(task_id.clone(), crate::dispatcher::TaskState { 
                        progress: 0.0, 
                        completed: false, 
                        error: String::new(),
                        abort_handle: None,
                        result_handle: None,
                        payload: Vec::new(),
                    });
                }

                let join_handle = tokio::spawn(async move {
                    log::debug!(target: "host", "Executing HTTP Task {}...", task_id_inner);
                    let client = reqwest::Client::new();
                    let method = match req.method.to_uppercase().as_str() {
                        "POST" => reqwest::Method::POST,
                        "PUT" => reqwest::Method::PUT,
                        "DELETE" => reqwest::Method::DELETE,
                        _ => reqwest::Method::GET,
                    };

                    let mut builder = client.request(method, &req.url);
                    for (k, v) in req.headers { builder = builder.header(k, v); }
                    if !req.body.is_empty() { builder = builder.body(req.body); }

                    let result = match builder.send().await {
                        Ok(res) => {
                            let status = res.status().as_u16() as u32;
                            let body = res.bytes().await.unwrap_or_default().to_vec();
                            Ok((status, body))
                        }
                        Err(e) => Err(e.to_string()),
                    };

                    let mut tasks = tasks_clone.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                        match result {
                            Ok((status, body)) => {
                                log::debug!(target: "host", "HTTP Task {} finished with status {}", task_id_inner, status);
                                let response = HttpTaskResponse { status, body };
                                t.payload = response.encode_to_vec();
                                t.progress = 1.0;
                                t.completed = true;
                            }
                            Err(e) => {
                                log::warn!(target: "host", "HTTP Task {} failed: {}", task_id_inner, e);
                                t.error = e;
                                t.completed = true;
                            }
                        }
                    }
                });

                if let Some(t) = self.tasks.lock().unwrap().get_mut(&task_id) {
                    t.abort_handle = Some(join_handle.abort_handle());
                }

                Ok(crate::core::TaskResponse { task_id }.encode_to_vec())
            }
            "task_status" => {
                let req = TaskStatusRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get(&req.task_id) {
                    let response = TaskStatusResponse { 
                        progress: task.progress, 
                        completed: task.completed, 
                        error: task.error.clone(),
                        result_handle: task.result_handle.clone(),
                        payload: task.payload.clone(),
                    }.encode_to_vec();
                    
                    if task.completed {
                        log::debug!(target: "host", "Task {} completed and removed from host", req.task_id);
                        tasks.remove(&req.task_id);
                    }
                    
                    Ok(response)
                } else {
                    Err(anyhow::anyhow!("Task {} not found on host", req.task_id))
                }
            }
            "task_cancel" => {
                let req = TaskCancelRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get_mut(&req.task_id) {
                    if let Some(handle) = task.abort_handle.take() {
                        handle.abort();
                    }
                    task.completed = true;
                    task.error = "Cancelled by user".to_string();
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Task not found"))
                }
            }
            "fs_list" => {
                let req = FsListRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let mut entries = Vec::new();
                if Path::new(&req.path).exists() {
                    for entry in fs::read_dir(&req.path)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() { entries.push(name.to_string()); }
                    }
                }
                Ok(FsListResponse { entries }.encode_to_vec())
            }
            "acquire_resource" => {
                use crate::core::AcquireResourceRequest;
                let req = AcquireResourceRequest::decode(&payload[..])?;
                if self.resources.acquire_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found or unauthorized", req.id))
                }
            }
            "release_resource" => {
                use crate::core::ReleaseResourceRequest;
                let req = ReleaseResourceRequest::decode(&payload[..])?;
                self.resources.release_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            "freeze_resource" => {
                use crate::core::FreezeResourceRequest;
                let req = FreezeResourceRequest::decode(&payload[..])?;
                if self.resources.freeze_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found to freeze or unauthorized", req.id))
                }
            }
            "destroy_resource" => {
                use crate::core::DestroyResourceRequest;
                let req = DestroyResourceRequest::decode(&payload[..])?;
                self.resources.destroy_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown method")),
        }
    }
}
