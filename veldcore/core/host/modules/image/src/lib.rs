use veldmap_host_core::dispatcher::{AsyncNativeService, Dispatcher};
use veldmap_host_core::resources::ResourceManager;
use veldmap_host_core::core::{
    ImageInfoRequest, ImageInfoResult, ImageLoadRequest, ResourceHandle,
    ImageLoadResult,
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::ColorType;

pub struct ImageService {
    dispatcher: Arc<Dispatcher>,
    resources: Arc<ResourceManager>,
    tasks: Arc<Mutex<HashMap<String, veldmap_host_core::dispatcher::TaskState>>>,
}

impl ImageService {
    pub fn new(dispatcher: Arc<Dispatcher>, resources: Arc<ResourceManager>, tasks: Arc<Mutex<HashMap<String, veldmap_host_core::dispatcher::TaskState>>>) -> Self {
        Self { dispatcher, resources, tasks }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }

    async fn handle_image_info(&self, payload: Vec<u8>) {
        let req = match ImageInfoRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode ImageInfoRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        let result = if !Self::is_path_safe(&req.path) {
            ImageInfoResult { width: 0, height: 0, channels: 0, error: "Access denied".into(), correlation_id }
        } else {
            match image::image_dimensions(&req.path) {
                Ok((w, h)) => ImageInfoResult {
                    width: w, height: h, channels: 4, error: String::new(), correlation_id,
                },
                Err(e) => ImageInfoResult {
                    width: 0, height: 0, channels: 0, error: e.to_string(), correlation_id,
                },
            }
        };
        self.dispatcher.publish("image/info_result", result.encode_to_vec());
    }

    async fn handle_image_load(&self, payload: Vec<u8>, requestor_id: u32) {
        let req = match ImageLoadRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode ImageLoadRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        if !Self::is_path_safe(&req.path) {
            let result = ImageLoadResult {
                handle: None,
                error: "Access denied".into(),
                correlation_id,
            };
            self.dispatcher.publish("image/load_result", result.encode_to_vec());
            return;
        }

        let task_id = uuid::Uuid::new_v4().to_string();
        {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(task_id.clone(), veldmap_host_core::dispatcher::TaskState { 
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
        let dispatcher_clone = self.dispatcher.clone();
        let correlation_id_inner = correlation_id.clone();
        
        let join_handle = tokio::task::spawn_blocking(move || {
            let update_status = |progress: f32, err: String, handle: Option<ResourceHandle>, payload: Option<Vec<u8>>| {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_inner) {
                    t.progress = progress;
                    if !err.is_empty() {
                        t.error = err;
                        t.completed = true;
                    }
                    if handle.is_some() {
                        t.result_handle = handle;
                        if let Some(p) = payload {
                            t.payload = p;
                        }
                        t.completed = true;
                        t.progress = 1.0;
                    }
                }
            };

            let path_lower = path.to_lowercase();
            let is_tiff = path_lower.ends_with(".tif") || path_lower.ends_with(".tiff");

            let rgba = if is_tiff {
                let file = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(e) => { 
                        update_status(0.0, e.to_string(), None, None); 
                        let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                        return; 
                    }
                };
                let buf_reader = std::io::BufReader::with_capacity(1024 * 1024, file);
                let mut decoder = match Decoder::new(buf_reader) {
                    Ok(d) => d,
                    Err(e) => { 
                        update_status(0.0, e.to_string(), None, None); 
                        let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                        return; 
                    }
                };
                
                let (width, height) = match decoder.dimensions() {
                    Ok(d) => d,
                    Err(e) => { 
                        update_status(0.0, e.to_string(), None, None); 
                        let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                        return; 
                    }
                };
                
                let color_type = match decoder.colortype() {
                    Ok(ct) => ct,
                    Err(e) => { 
                        update_status(0.0, e.to_string(), None, None); 
                        let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                        return; 
                    }
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
                update_status(0.2, String::new(), None, None);

                let img_res = decoder.read_image();
                update_status(0.5, String::new(), None, None);

                match img_res {
                    Ok(res) => {
                        let mut rgba8 = image::RgbaImage::new(width, height);
                        
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
                                    Err(fe) => { 
                                        update_status(0.0, format!("Fallback error: {}", fe), None, None); 
                                        let result = ImageLoadResult { handle: None, error: format!("Fallback error: {}", fe), correlation_id: correlation_id_inner.clone() };
                                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                                        return; 
                                    }
                                };
                                let rgba8 = fallback_img.to_rgba8();
                                let (w, h) = rgba8.dimensions();
                                let tex_id = resources.create_texture(w, h, 0, 8, false, requestor_id);
                                if let Err(e) = resources.write_resource(tex_id, 0, &rgba8, requestor_id) {
                                    update_status(0.0, e.to_string(), None, None);
                                    let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                                    dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                                    return;
                                }
                                let handle = ResourceHandle {
                                    id: tex_id,
                                    size: (w * h * 4) as u64,
                                    content_hash: resources.compute_hash(tex_id, requestor_id).unwrap_or_default(),
                                };
                                let payload = handle.encode_to_vec();
                                update_status(1.0, String::new(), Some(handle), Some(payload));
                                let result = ImageLoadResult { handle: Some(handle), error: String::new(), correlation_id: correlation_id_inner.clone() };
                                dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                                return;
                            }
                        };

                        let mut min_val = f32::MAX;
                        let mut max_val = f32::MIN;
                        
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
                            Err(fe) => { 
                                update_status(0.0, format!("TIFF error: {:?}, Fallback error: {}", e, fe), None, None); 
                                let result = ImageLoadResult { handle: None, error: format!("TIFF error: {:?}, Fallback error: {}", e, fe), correlation_id: correlation_id_inner.clone() };
                                dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                                return; 
                            }
                        };
                        fallback_img.to_rgba8()
                    }
                }
            } else {
                let img = match image::open(&path) {
                    Ok(i) => i,
                    Err(e) => { 
                        update_status(0.0, e.to_string(), None, None); 
                        let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                        dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                        return; 
                    }
                };
                update_status(0.3, String::new(), None, None);

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
            update_status(0.8, String::new(), None, None);

            let (w, h) = rgba.dimensions();
            let final_rgba = if is_tiff && (req.target_width > 0 || req.target_height > 0) {
                let tw = if req.target_width == 0 { w } else { req.target_width };
                let th = if req.target_height == 0 { h } else { req.target_height };
                image::DynamicImage::ImageRgba8(rgba).thumbnail(tw, th).to_rgba8()
            } else {
                rgba
            };

            let (w, h) = final_rgba.dimensions();
            let tex_id = resources.create_texture(w, h, 0, 8, false, requestor_id);
            if let Err(e) = resources.write_resource(tex_id, 0, &final_rgba, requestor_id) {
                update_status(0.0, e.to_string(), None, None);
                let result = ImageLoadResult { handle: None, error: e.to_string(), correlation_id: correlation_id_inner.clone() };
                dispatcher_clone.publish("image/load_result", result.encode_to_vec());
                return;
            }

            let handle = ResourceHandle {
                id: tex_id,
                size: (w * h * 4) as u64,
                content_hash: resources.compute_hash(tex_id, requestor_id).unwrap_or_default(),
            };
            let payload = handle.encode_to_vec();
            log::info!("[IMAGE] Loaded '{}': texture_id={}, size={}x{}", path, tex_id, w, h);
            update_status(1.0, String::new(), Some(handle), Some(payload));
            let result = ImageLoadResult { handle: Some(handle), error: String::new(), correlation_id: correlation_id_inner.clone() };
            dispatcher_clone.publish("image/load_result", result.encode_to_vec());
        });

        {
            let mut tasks = self.tasks.lock().unwrap();
            if let Some(t) = tasks.get_mut(&task_id) {
                t.abort_handle = Some(join_handle.abort_handle());
            }
        }
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for ImageService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32) {
        match topic {
            "info" => self.handle_image_info(payload).await,
            "load" => self.handle_image_load(payload, requestor_id).await,
            _ => log::warn!(target: "host", "Unknown image topic: {}", topic),
        }
    }
}
