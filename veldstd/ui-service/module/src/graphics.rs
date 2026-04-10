//! GPU graphics rendering for UI service
//! Handles all GPU resource management and command encoding

use crate::state::PluginUiState;
use crate::renderer::{GpuRenderer, DrawCmd};
use veldsdk::compute::*;
use veldsdk::compute::wgpu_proxy::ComputeRecorder;
use veldsdk::rpc::host::{call_service, gpu_write_resource};
use veldsdk::OwnedResource;
use prost::Message;
use anyhow::anyhow;

/// Renders UI to an offscreen texture and returns its ID
pub fn render_ui(
    plugin: &PluginUiState,
    renderer: &mut GpuRenderer,
    width: u32,
    height: u32,
    scale_factor: f32,
    surface_format: i32,
) -> anyhow::Result<u64> {
    // Create offscreen texture
    let texture_id = create_offscreen_texture(width, height, surface_format)?;
    let view_id = create_texture_view(texture_id, surface_format)?;
    
    // Ensure all GPU resources are ready
    ensure_resources(plugin, renderer, surface_format)?;

    let mut recorder = ComputeRecorder::new(width, height);
    let logical_w = width as f32 / scale_factor;
    let logical_h = height as f32 / scale_factor;

    // Update uniform buffer with logical size
    if let Some(u_id) = *plugin.uniform_buffer_id.borrow() {
        let res_data: [f32; 2] = [logical_w, logical_h];
        let data = unsafe { std::slice::from_raw_parts(res_data.as_ptr() as *const u8, 8) };
        let _ = gpu_write_resource(u_id, 0, data);
    }

    // Update texture atlas if dirty
    if renderer.is_atlas_dirty() {
        if let Some(tid) = renderer.atlas_texture_id {
            // NOTE: For dzn (DirectX 12 on Vulkan), we need full texture writes only
            let data = renderer.atlas_data_full();
            let _ = gpu_write_resource(tid, 0, data);
            renderer.mark_atlas_clean();
        }
    }

    // Render geometry if present
    if !renderer.vertices.is_empty() {
        render_geometry(plugin, renderer, &mut recorder, width, height)?;
    }

    // Execute immediately to the offscreen texture
    let _ = recorder.execute(view_id)?;
    Ok(texture_id)
}

/// Create offscreen texture for UI rendering
fn create_offscreen_texture(width: u32, height: u32, surface_format: i32) -> anyhow::Result<u64> {
    let texture_req = ComputeResourceRequest {
        instance_id: 0,
        command: Some(compute_resource_request::Command::CreateTexture(CreateTexture {
            width, height, 
            format: surface_format, 
            usage: 2 | 4 | 16, // RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC
            dimension: 1, 
            mip_level_count: 1, 
            sample_count: 1, 
            depth_or_array_layers: 1, 
            readonly: false
        }))
    };
    let texture_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", texture_req.encode_to_vec())?[..])?;
    let texture_id = texture_res.handle.ok_or_else(|| anyhow!("Failed to create offscreen texture"))?.id;
    Ok(texture_id)
}

/// Create texture view for rendering
fn create_texture_view(texture_id: u64, surface_format: i32) -> anyhow::Result<u64> {
    let view_req = ComputeResourceRequest {
        instance_id: 0,
        command: Some(compute_resource_request::Command::CreateTextureView(CreateTextureView {
            texture_id, 
            format: surface_format, 
            dimension: 0, 
            aspect: 1, 
            ..Default::default()
        }))
    };
    let view_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", view_req.encode_to_vec())?[..])?;
    let view_id = view_res.handle.ok_or_else(|| anyhow!("Failed to create texture view"))?.id;
    Ok(view_id)
}

/// Render geometry to command recorder
fn render_geometry(
    plugin: &PluginUiState,
    renderer: &GpuRenderer,
    recorder: &mut ComputeRecorder,
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    let vertex_size = std::mem::size_of::<crate::renderer::Vertex>();
    
    // Ensure vertex buffer exists
    let mut vertex_buffer = plugin.vertex_buffer.borrow_mut();
    if vertex_buffer.is_none() {
        let req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBuffer(CreateBuffer {
                size: 1024 * 1024 * 8, usage: 32, mapped_at_creation: false, readonly: false
            }))
        };
        let res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", req.encode_to_vec())?[..])?;
        *vertex_buffer = res.handle.map(OwnedResource::new);
    }

    // Ensure index buffer exists
    let mut index_buffer = plugin.index_buffer.borrow_mut();
    if index_buffer.is_none() {
        let req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBuffer(CreateBuffer {
                size: 1024 * 1024 * 2, usage: 16, mapped_at_creation: false, readonly: false
            }))
        };
        let res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", req.encode_to_vec())?[..])?;
        *index_buffer = res.handle.map(OwnedResource::new);
    }

    // Upload vertex and index data
    if let (Some(ref v_h), Some(ref i_h)) = (&*vertex_buffer, &*index_buffer) {
        let v_data = unsafe { std::slice::from_raw_parts(renderer.vertices.as_ptr() as *const u8, renderer.vertices.len() * vertex_size) };
        let _ = gpu_write_resource(v_h.id(), 0, v_data);

        let i_data = unsafe { std::slice::from_raw_parts(renderer.indices.as_ptr() as *const u8, renderer.indices.len() * 2) };
        let _ = gpu_write_resource(i_h.id(), 0, i_data);
    }

    // Record draw commands if pipeline is ready
    if let (Some(pipeline), Some(ref v_h), Some(ref i_h), Some(ref u_h)) = 
        (*plugin.ui_pipeline.borrow(), &*vertex_buffer, &*index_buffer, &*plugin.uniform_buffer.borrow()) 
    {
        recorder.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

        let mut current_index_offset = 0;
        for cmd in &renderer.draw_commands {
            match cmd {
                DrawCmd::Quads { count } => {
                    recorder.set_pipeline(pipeline);
                    recorder.set_vertex_buffer(0, v_h.id(), 0, (renderer.vertices.len() * vertex_size) as u64);
                    recorder.set_index_buffer(i_h.id(), IndexFormat::IdxUint16 as u32, 0, (renderer.indices.len() * 2) as u64);
                    recorder.set_bind_group(1, u_h.id());
                    if let Some(atlas_bg) = renderer.atlas_bind_group_id {
                        recorder.set_bind_group(0, atlas_bg);
                    }
                    recorder.draw_indexed(current_index_offset..(*count + current_index_offset), 0, 0..1);
                    current_index_offset += *count;
                }
                DrawCmd::Scissor { x, y, width, height } => {
                    recorder.set_scissor_rect(*x, *y, *width, *height);
                }
                DrawCmd::ExternalImage { texture_id, index_count, .. } => {
                    match get_external_bind_group(plugin, renderer, *texture_id) {
                        Ok(bg_id) => {
                            recorder.set_bind_group(0, bg_id);
                            recorder.draw_indexed(current_index_offset..(current_index_offset + *index_count), 0, 0..1);
                            current_index_offset += *index_count;
                            if let Some(atlas_bg) = renderer.atlas_bind_group_id {
                                recorder.set_bind_group(0, atlas_bg);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to create external bind group for texture {}: {}", texture_id, e);
                            current_index_offset += *index_count;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Get or create bind group for external texture
fn get_external_bind_group(plugin: &PluginUiState, renderer: &GpuRenderer, texture_id: u64) -> anyhow::Result<u64> {
    if texture_id == 0 {
        return Err(anyhow!("Invalid texture_id: 0"));
    }
    let mut cache = plugin.external_bind_groups.borrow_mut();
    if let Some(&bg_id) = cache.get(&texture_id) {
        return Ok(bg_id);
    }

    let view_req = ComputeResourceRequest {
        instance_id: 0,
        command: Some(compute_resource_request::Command::CreateTextureView(CreateTextureView {
            texture_id, format: TextureFormat::TexRgba8Unorm as i32, dimension: 0, aspect: 1, ..Default::default()
        }))
    };
    let view_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", view_req.encode_to_vec())?[..])?;
    let view_id = view_res.handle.ok_or_else(|| anyhow!("Failed to create texture view"))?.id;

    let sampler_req = ComputeResourceRequest {
        instance_id: 0,
        command: Some(compute_resource_request::Command::CreateSampler(CreateSampler { 
            mag_filter: FilterMode::FiltLinear as i32, 
            min_filter: FilterMode::FiltLinear as i32, 
            ..Default::default() 
        }))
    };
    let sampler_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", sampler_req.encode_to_vec())?[..])?;
    let sampler_id = sampler_res.handle.ok_or_else(|| anyhow!("Failed to create sampler"))?.id;

    let bg_req = ComputeResourceRequest {
        instance_id: 0,
        command: Some(compute_resource_request::Command::CreateBindGroup(CreateBindGroup {
            layout_id: renderer.bgl_id.ok_or_else(|| anyhow!("No BGL"))?,
            entries: vec![
                BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::TextureViewId(view_id)) },
                BindGroupEntry { binding: 1, resource: Some(bind_group_entry::Resource::SamplerId(sampler_id)) },
            ],
            label: format!("External Image BG {}", texture_id),
        }))
    };
    let bg_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", bg_req.encode_to_vec())?[..])?;
    let bg_id = bg_res.handle.ok_or_else(|| anyhow!("Failed to create bind group"))?.id;

    cache.insert(texture_id, bg_id);
    log::debug!("Created external bind group {} for texture {}", bg_id, texture_id);
    Ok(bg_id)
}

/// Ensure all GPU resources are created (bind group layouts, pipeline, buffers, atlas)
pub fn ensure_resources(plugin: &PluginUiState, renderer: &mut GpuRenderer, surface_format: i32) -> anyhow::Result<()> {
    ensure_atlas_bind_group_layout(renderer)?;
    ensure_uniform_bind_group_layout(plugin)?;
    ensure_pipeline(plugin, renderer, surface_format)?;
    ensure_uniform_buffer(plugin)?;
    ensure_atlas_texture(renderer)?;
    ensure_atlas_bind_group(renderer)?;
    Ok(())
}

fn ensure_atlas_bind_group_layout(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.bgl_id.is_none() {
        let req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBindGroupLayout(CreateBindGroupLayout {
                label: "Iced Atlas BGL".into(),
                entries: vec![
                    BindGroupLayoutEntry { binding: 0, visibility: 2, ty: Some(bind_group_layout_entry::Ty::Texture(TextureBindingLayout { sample_type: 1, view_dimension: 2, multisampled: false })) },
                    BindGroupLayoutEntry { binding: 1, visibility: 2, ty: Some(bind_group_layout_entry::Ty::Sampler(SamplerBindingLayout { r#type: 1 })) },
                ],
            }))
        };
        if let Ok(res_bytes) = call_service("compute", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = ComputeResourceResponse::decode(&res_bytes[..]) {
                renderer.bgl_id = res.handle.map(|h| h.id);
            }
        }
    }
    Ok(())
}

fn ensure_uniform_bind_group_layout(plugin: &PluginUiState) -> anyhow::Result<()> {
    let mut uniform_layout_id = plugin.uniform_layout_id.borrow_mut();
    if uniform_layout_id.is_none() {
        let bgl_req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBindGroupLayout(CreateBindGroupLayout {
                label: "UI Uniform BGL".into(),
                entries: vec![BindGroupLayoutEntry {
                    binding: 0, visibility: 3, ty: Some(bind_group_layout_entry::Ty::Buffer(BufferBindingLayout { r#type: 1, ..Default::default() }))
                }]
            }))
        };
        let bgl_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", bgl_req.encode_to_vec())?[..])?;
        *uniform_layout_id = bgl_res.handle.map(|h| h.id);
    }
    Ok(())
}

fn ensure_pipeline(plugin: &PluginUiState, renderer: &GpuRenderer, surface_format: i32) -> anyhow::Result<()> {
    let mut ui_pipeline = plugin.ui_pipeline.borrow_mut();
    if ui_pipeline.is_none() {
        let shader_source = include_str!("shaders.wgsl");
        let sh_req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateShader(CreateShaderModule {
                source: shader_source.into(), label: "UI Shader".into()
            }))
        };
        let sh_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", sh_req.encode_to_vec())?[..])?;
        if let Some(sh) = sh_res.handle {
            let mut bgl_ids = Vec::new();
            if let Some(id) = renderer.bgl_id { bgl_ids.push(id); }
            if let Some(id) = *plugin.uniform_layout_id.borrow() { bgl_ids.push(id); }

            let pip_req = ComputeResourceRequest {
                instance_id: 0,
                command: Some(compute_resource_request::Command::CreatePipeline(CreateRenderPipeline {
                    shader_id: sh.id, label: "UI Pipeline".into(), 
                    vertex_entry: "vs_main".into(), fragment_entry: "fs_main".into(),
                    target_format: surface_format,
                    vertex_layouts: vec![VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::renderer::Vertex>() as u64,
                        step_mode: StepMode::StepVertex as i32,
                        attributes: vec![
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 0, shader_location: 0 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x4 as i32, offset: 8, shader_location: 1 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 24, shader_location: 2 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 32, shader_location: 3 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 40, shader_location: 4 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 48, shader_location: 5 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 52, shader_location: 6 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 56, shader_location: 7 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x4 as i32, offset: 60, shader_location: 8 },
                        ],
                    }],
                    bind_group_layout_ids: bgl_ids,
                    primitive_topology: PrimitiveTopology::TopologyTriangleList as i32,
                    front_face: FrontFace::Ccw as i32,
                    cull_mode: CullMode::None as i32,
                    ..Default::default()
                }))
            };
            let pip_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", pip_req.encode_to_vec())?[..])?;
            *ui_pipeline = pip_res.handle.map(|h| h.id);
        }
    }
    Ok(())
}

fn ensure_uniform_buffer(plugin: &PluginUiState) -> anyhow::Result<()> {
    let mut uniform_buffer = plugin.uniform_buffer.borrow_mut();
    let mut uniform_buffer_id = plugin.uniform_buffer_id.borrow_mut();
    if uniform_buffer.is_none() && plugin.uniform_layout_id.borrow().is_some() {
        let buf_req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBuffer(CreateBuffer {
                size: 16, usage: 64, mapped_at_creation: false, readonly: false
            }))
        };
        let buf_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", buf_req.encode_to_vec())?[..])?;
        if let Some(bh) = buf_res.handle {
            *uniform_buffer_id = Some(bh.id);
            let bg_req = ComputeResourceRequest {
                instance_id: 0,
                command: Some(compute_resource_request::Command::CreateBindGroup(CreateBindGroup {
                    layout_id: plugin.uniform_layout_id.borrow().unwrap(), 
                    entries: vec![BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::BufferId(bh.id)) }], 
                    label: "UI Uniform BG".into()
                }))
            };
            let bg_res = ComputeResourceResponse::decode(&call_service("compute", "create_resource", bg_req.encode_to_vec())?[..])?;
            *uniform_buffer = bg_res.handle.map(OwnedResource::new);
        }
    }
    Ok(())
}

fn ensure_atlas_texture(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.atlas_texture_id.is_none() {
        let (w, h) = renderer.atlas_dimensions();
        let req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateTexture(CreateTexture {
                width: w, height: h, format: TextureFormat::TexRgba8Unorm as i32, usage: 2 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1, readonly: false
            }))
        };
        if let Ok(res_bytes) = call_service("compute", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = ComputeResourceResponse::decode(&res_bytes[..]) {
                renderer.atlas_texture_id = res.handle.map(|h| h.id);
                renderer.mark_atlas_dirty();
            }
        }
    }
    Ok(())
}

fn ensure_atlas_bind_group(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.atlas_bind_group_id.is_none() && renderer.atlas_texture_id.is_some() && renderer.bgl_id.is_some() {
        let sampler_req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateSampler(CreateSampler { 
                mag_filter: FilterMode::FiltLinear as i32, 
                min_filter: FilterMode::FiltLinear as i32, 
                ..Default::default() 
            }))
        };
        let sampler_id = call_service("compute", "create_resource", sampler_req.encode_to_vec())
            .ok().and_then(|b| ComputeResourceResponse::decode(&b[..]).ok())
            .and_then(|r| r.handle).map(|h| h.id).unwrap_or(0);
        
        let req = ComputeResourceRequest {
            instance_id: 0,
            command: Some(compute_resource_request::Command::CreateBindGroup(CreateBindGroup {
                layout_id: renderer.bgl_id.unwrap(), 
                entries: vec![
                    BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::TextureViewId(renderer.atlas_texture_id.unwrap())) },
                    BindGroupEntry { binding: 1, resource: Some(bind_group_entry::Resource::SamplerId(sampler_id)) },
                ],
                label: "Iced Atlas BG".into(),
            }))
        };
        if let Ok(res_bytes) = call_service("compute", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = ComputeResourceResponse::decode(&res_bytes[..]) {
                renderer.atlas_bind_group_id = res.handle.map(|h| h.id);
            }
        }
    }
    Ok(())
}
