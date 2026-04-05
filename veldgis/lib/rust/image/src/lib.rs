//! Вспомогательная библиотека для обработки и загрузки изображений на стороне WASM

use veldsdk::rpc::core::ResourceHandle;
use image::DynamicImage;
use veldsdk::wgpu::{GpuResourceRequest, gpu_resource_request, CreateTexture, TextureFormat, GpuResourceResponse};
use veldsdk::prost::Message;

/// Загружает изображение с диска хоста через RPC, обрабатывает (включая авто-контраст
/// для 16-битных и HDR снимков) и отправляет обратно на GPU хоста, возвращая дескриптор ресурса.
pub async fn load_image_to_gpu(path: &str) -> anyhow::Result<ResourceHandle> {
    // 1. Читаем байты файла через RPC хоста
    let file_bytes = veldsdk::core::fs_read_bytes(path)?;
    
    // 2. Декодируем изображение в памяти
    let img = image::load_from_memory(&file_bytes)?;
    let (w, h) = image::GenericImageView::dimensions(&img);
    
    // 3. Авто-контраст для 16-битных и HDR снимков
    let is_hdr_or_16bit = match img {
        DynamicImage::ImageLuma16(_) |
        DynamicImage::ImageLumaA16(_) |
        DynamicImage::ImageRgb16(_) |
        DynamicImage::ImageRgba16(_) |
        DynamicImage::ImageRgb32F(_) |
        DynamicImage::ImageRgba32F(_) => true,
        _ => false,
    };

    let rgba = if is_hdr_or_16bit {
        let mut img_32f = img.into_rgba32f();
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
        rgba8.into_raw()
    } else {
        let mut rgba8 = img.into_rgba8();
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
        rgba8.into_raw()
    };

    // 4. Отправляем текстуру на GPU через RPC
    let req = GpuResourceRequest {
        instance_id: 0,
        command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
            width: w,
            height: h,
            depth_or_array_layers: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: 1, // 2D
            format: TextureFormat::TexRgba8Unorm as i32,
            usage: 8 | 4, // TEXTURE_BINDING (8) | COPY_DST (4)
            readonly: false,
        }))
    };
    
    let res_bytes = veldsdk::rpc::host::call_service("wgpu", "create_resource", req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_bytes[..])?;
    let mut handle = res.handle.ok_or_else(|| anyhow::anyhow!("No texture handle returned"))?;
    handle.size = (w * h * 4) as u64;

    veldsdk::rpc::host::gpu_write_resource(handle.id, 0, &rgba);

    // Замораживаем ресурс, чтобы другие могли безопасно его читать (readonly)
    let freeze_req = veldsdk::rpc::core::FreezeResourceRequest { id: handle.id };
    veldsdk::rpc::host::call_service("system", "freeze_resource", freeze_req.encode_to_vec())?;

    Ok(handle)
}
