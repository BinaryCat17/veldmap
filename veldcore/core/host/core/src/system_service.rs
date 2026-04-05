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
                let tasks_clone = self.tasks.clone();
                let resources = self.resources.clone();
                let path = req.path.clone();
                
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
                        let mut decoder = match Decoder::new(file) {
                            Ok(d) => d,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        
                        let (width, height) = match decoder.dimensions() {
                            Ok(d) => d,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        update_status(0.2, String::new(), None);

                        let color_type = match decoder.colortype() {
                            Ok(ct) => ct,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };

                        let img_res = match decoder.read_image() {
                            Ok(res) => res,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        update_status(0.5, String::new(), None);

                        // Normalize and convert to RGBA8
                        match img_res {
                            DecodingResult::U8(data) => {
                                let mut rgba8 = image::RgbaImage::new(width, height);
                                let channels = match color_type { ColorType::Gray(8) => 1, ColorType::RGB(8) => 3, ColorType::RGBA(8) => 4, _ => 1 };
                                for y in 0..height {
                                    for x in 0..width {
                                        let base = (y as usize * width as usize + x as usize) * channels;
                                        let r = data[base];
                                        let g = if channels >= 3 { data[base+1] } else { r };
                                        let b = if channels >= 3 { data[base+2] } else { r };
                                        let a = if channels == 4 { data[base+3] } else { 255 };
                                        rgba8.put_pixel(x, y, image::Rgba([r, g, b, a]));
                                    }
                                }
                                rgba8
                            }
                            DecodingResult::U16(data) => {
                                let mut min_val = u16::MAX;
                                let mut max_val = u16::MIN;
                                for &v in &data { if v > 0 { min_val = min_val.min(v); } max_val = max_val.max(v); }
                                if min_val == u16::MAX { min_val = 0; }
                                let range = (max_val as f32 - min_val as f32).max(1.0);

                                let mut rgba8 = image::RgbaImage::new(width, height);
                                let channels = match color_type { ColorType::Gray(16) => 1, ColorType::RGB(16) => 3, ColorType::RGBA(16) => 4, _ => 1 };
                                for y in 0..height {
                                    for x in 0..width {
                                        let base = (y as usize * width as usize + x as usize) * channels;
                                        let v = data[base];
                                        let val = if v > 0 { (((v as f32 - min_val as f32) / range) * 255.0) as u8 } else { 0 };
                                        let r = val;
                                        let g = if channels >= 3 { 
                                            let vg = data[base+1];
                                            if vg > 0 { (((vg as f32 - min_val as f32) / range) * 255.0) as u8 } else { 0 }
                                        } else { r };
                                        let b = if channels >= 3 { 
                                            let vb = data[base+2];
                                            if vb > 0 { (((vb as f32 - min_val as f32) / range) * 255.0) as u8 } else { 0 }
                                        } else { r };
                                        let a = if channels == 4 { (data[base+3] >> 8) as u8 } else { 255 };
                                        rgba8.put_pixel(x, y, image::Rgba([r, g, b, a]));
                                    }
                                }
                                rgba8
                            }
                            DecodingResult::I16(data) => {
                                let mut min_val = i16::MAX;
                                let mut max_val = i16::MIN;
                                for &v in &data { if v != 0 { min_val = min_val.min(v); } max_val = max_val.max(v); }
                                if min_val == i16::MAX { min_val = 0; }
                                let range = (max_val as f32 - min_val as f32).max(1.0);

                                let mut rgba8 = image::RgbaImage::new(width, height);
                                for (i, &v) in data.iter().enumerate() {
                                    let x = (i as u32) % width;
                                    let y = (i as u32) / width;
                                    let val = if v != 0 { (((v as f32 - min_val as f32) / range) * 255.0) as u8 } else { 0 };
                                    rgba8.put_pixel(x, y, image::Rgba([val, val, val, 255]));
                                }
                                rgba8
                            }
                            DecodingResult::F32(data) => {
                                let mut min_val = f32::MAX;
                                let mut max_val = f32::MIN;
                                for &v in &data { if v.is_finite() && v != 0.0 { min_val = min_val.min(v); } max_val = max_val.max(v); }
                                if min_val == f32::MAX { min_val = 0.0; }
                                let range = (max_val - min_val).max(0.0001);

                                let mut rgba8 = image::RgbaImage::new(width, height);
                                for (i, &v) in data.iter().enumerate() {
                                    let x = (i as u32) % width;
                                    let y = (i as u32) / width;
                                    let val = if v != 0.0 { (((v - min_val) / range) * 255.0) as u8 } else { 0 };
                                    rgba8.put_pixel(x, y, image::Rgba([val, val, val, 255]));
                                }
                                rgba8
                            }
                            _ => { // Fallback to standard
                                let img = match image::open(&path) {
                                    Ok(i) => i,
                                    Err(e) => { update_status(0.0, e.to_string(), None); return; }
                                };
                                img.to_rgba8()
                            }
                        }
                    } else {
                        // Standard formats (PNG, JPEG, etc)
                        let img = match image::open(&path) {
                            Ok(i) => i,
                            Err(e) => { update_status(0.0, e.to_string(), None); return; }
                        };
                        update_status(0.3, String::new(), None);

                        // Resize if needed
                        let final_img = if req.target_width > 0 || req.target_height > 0 {
                            let tw = if req.target_width == 0 { img.width() } else { req.target_width };
                            let th = if req.target_height == 0 { img.height() } else { req.target_height };
                            
                            if req.preserve_aspect {
                                img.thumbnail(tw, th)
                            } else {
                                img.thumbnail_exact(tw, th)
                            }
                        } else {
                            img
                        };
                        update_status(0.6, String::new(), None);

                        let is_hdr_or_16bit = match final_img {
                            image::DynamicImage::ImageLuma16(_) |
                            image::DynamicImage::ImageLumaA16(_) |
                            image::DynamicImage::ImageRgb16(_) |
                            image::DynamicImage::ImageRgba16(_) |
                            image::DynamicImage::ImageRgb32F(_) |
                            image::DynamicImage::ImageRgba32F(_) => true,
                            _ => false,
                        };

                        if is_hdr_or_16bit {
                            let mut img_32f = final_img.into_rgba32f();
                            let (w, h) = img_32f.dimensions();
                            let mut min_val = f32::MAX;
                            let mut max_val = f32::MIN;
                            
                            for pixel in img_32f.pixels() {
                                if pixel[3] > 0.0 {
                                    if pixel[0] > 0.0 || pixel[1] > 0.0 || pixel[2] > 0.0 {
                                        min_val = min_val.min(pixel[0]).min(pixel[1]).min(pixel[2]);
                                    }
                                    max_val = max_val.max(pixel[0]).max(pixel[1]).max(pixel[2]);
                                }
                            }
                            if min_val == f32::MAX { min_val = 0.0; }
                            
                            if max_val > min_val {
                                let range = max_val - min_val;
                                for pixel in img_32f.pixels_mut() {
                                    if pixel[3] > 0.0 {
                                        pixel[0] = if pixel[0] > 0.0 { ((pixel[0] - min_val) / range).clamp(0.0, 1.0) } else { 0.0 };
                                        pixel[1] = if pixel[1] > 0.0 { ((pixel[1] - min_val) / range).clamp(0.0, 1.0) } else { 0.0 };
                                        pixel[2] = if pixel[2] > 0.0 { ((pixel[2] - min_val) / range).clamp(0.0, 1.0) } else { 0.0 };
                                    }
                                }
                            }
                            
                            let mut rgba8 = image::RgbaImage::new(w, h);
                            for (x, y, pixel) in img_32f.enumerate_pixels() {
                                rgba8.put_pixel(x, y, image::Rgba([
                                    (pixel[0] * 255.0) as u8,
                                    (pixel[1] * 255.0) as u8,
                                    (pixel[2] * 255.0) as u8,
                                    (pixel[3] * 255.0) as u8,
                                ]));
                            }
                            rgba8
                        } else {
                            // Standard 8-bit image with auto-contrast
                            let mut rgba8 = final_img.into_rgba8();
                            let mut min_val = 255u8;
                            let mut max_val = 0u8;
                            
                            for pixel in rgba8.pixels() {
                                if pixel[3] > 0 {
                                    if pixel[0] > 0 || pixel[1] > 0 || pixel[2] > 0 {
                                        min_val = min_val.min(pixel[0]).min(pixel[1]).min(pixel[2]);
                                    }
                                    max_val = max_val.max(pixel[0]).max(pixel[1]).max(pixel[2]);
                                }
                            }
                            if min_val == 255 { min_val = 0; }
                            
                            if max_val > min_val && (min_val > 0 || max_val < 255) {
                                let range = max_val as f32 - min_val as f32;
                                for pixel in rgba8.pixels_mut() {
                                    if pixel[3] > 0 {
                                        pixel[0] = if pixel[0] > 0 { (((pixel[0] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
                                        pixel[1] = if pixel[1] > 0 { (((pixel[1] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
                                        pixel[2] = if pixel[2] > 0 { (((pixel[2] as f32 - min_val as f32) / range) * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
                                    }
                                }
                            }
                            rgba8
                        }
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
