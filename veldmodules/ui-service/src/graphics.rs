//! GPU-часть сервиса: ресурсы устройства и запись команд кадра.
//!
//! Геометрию считает renderer, сюда она приходит готовой — здесь только то,
//! что нужно хосту, чтобы её нарисовать.

use std::collections::HashMap;

use crate::module::state::PluginUiState;
use crate::module::renderer::{GpuRenderer, DrawCmd};
use veldsdk::graphics::{
    self as gfx, buffer_usage, texture_usage, BindGroupId, CreateRenderPipeline, CullMode,
    FilterMode, FrontFace, IndexFormat, PrimitiveTopology, RenderRecorder, StepMode,
    TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat,
    VISIBILITY_FRAGMENT, VISIBILITY_VERTEX,
};
use veldsdk::abi::{resource_write, resource_upload_image, resource_alloc_buffer, resource_alloc_texture};
use veldsdk::proto::core::ResourceHandle;
use veldsdk::OwnedResource;

use anyhow::anyhow;

const VERTEX_BUFFER_SIZE: u64 = 8 * 1024 * 1024;
const INDEX_BUFFER_SIZE: u64 = 2 * 1024 * 1024;

/// Рисует UI в render-таргет, делегированный владельцем окна (его текстура,
/// выданная нам write-lease'ом). View текстуры кэшируется по её id: на resize
/// владелец выделяет новый таргет, id меняется, и прежний view заменяется.
pub fn render_ui(
    plugin: &mut PluginUiState,
    renderer: &mut GpuRenderer,
    target_texture: u64,
    width: u32,
    height: u32,
    scale_factor: f32,
    surface_format: i32,
) -> anyhow::Result<()> {
    veldsdk::log::trace!(target: "graphics", "START {}x{} into texture {}", width, height, target_texture);

    let cached = matches!(&plugin.target_view, Some((tex, _)) if *tex == target_texture);
    if !cached {
        let view = gfx::create_texture_view(target_texture)?;
        plugin.target_view = Some((target_texture, view));
    }

    ensure_resources(plugin, renderer, surface_format)?;
    evict_unused_bind_groups(&mut plugin.external_bind_groups, renderer);

    let mut recorder = RenderRecorder::new();
    let logical_w = width as f32 / scale_factor;
    let logical_h = height as f32 / scale_factor;

    // Размер холста в логических пикселях: по нему шейдер переводит координаты
    // раскладки в clip space.
    if let Some(u) = plugin.uniform_buffer_region.as_ref() {
        let res_data: [f32; 2] = [logical_w, logical_h];
        let data = unsafe { std::slice::from_raw_parts(res_data.as_ptr() as *const u8, 8) };
        resource_write(u.id(), 0, data)?;
    }

    // Новые глифы с прошлого кадра.
    if renderer.is_atlas_dirty() {
        let atlas_id = renderer.atlas_texture_id.as_ref().map(|t| t.id());
        if let Some(tid) = atlas_id {
            // Атлас заливается целиком, частичных обновлений нет: dzn
            // (DirectX 12 поверх Vulkan) принимает только полную запись.
            resource_upload_image(tid, renderer.atlas_data_full())?;
            renderer.mark_atlas_clean();
        }
    }

    if !renderer.vertices.is_empty() {
        veldsdk::log::trace!(target: "graphics", "Rendering {} vertices", renderer.vertices.len());
        render_geometry(plugin, renderer, &mut recorder, width, height)?;
    }

    // Записанное исполняет кадровый цикл хоста — не мы: наш вызов лишь ставит
    // работу в очередь на этот таргет.
    let view = &plugin.target_view.as_ref().expect("target view ensured above").1;
    recorder.submit(view)?;
    veldsdk::log::trace!(target: "graphics", "END");
    Ok(())
}

/// Переводит команды рисования в записи для хоста. Порядок сохраняется: у
/// ножниц и внешних картинок он значащий.
fn render_geometry(
    plugin: &mut PluginUiState,
    renderer: &GpuRenderer,
    recorder: &mut RenderRecorder,
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    let vertex_size = std::mem::size_of::<crate::module::renderer::Vertex>();

    if plugin.vertex_buffer.is_none() {
        let id = resource_alloc_buffer(VERTEX_BUFFER_SIZE, buffer_usage::VERTEX, false)
            .ok_or_else(|| anyhow!("Failed to allocate vertex buffer"))?;
        plugin.vertex_buffer = Some(OwnedResource::new(ResourceHandle { id, size: VERTEX_BUFFER_SIZE, ..Default::default() }));
    }

    if plugin.index_buffer.is_none() {
        let id = resource_alloc_buffer(INDEX_BUFFER_SIZE, buffer_usage::INDEX, false)
            .ok_or_else(|| anyhow!("Failed to allocate index buffer"))?;
        plugin.index_buffer = Some(OwnedResource::new(ResourceHandle { id, size: INDEX_BUFFER_SIZE, ..Default::default() }));
    }

    if let (Some(v_h), Some(i_h)) = (&plugin.vertex_buffer, &plugin.index_buffer) {
        let v_data = unsafe { std::slice::from_raw_parts(renderer.vertices.as_ptr() as *const u8, renderer.vertices.len() * vertex_size) };
        resource_write(v_h.id(), 0, v_data)?;

        let i_data = unsafe { std::slice::from_raw_parts(renderer.indices.as_ptr() as *const u8, renderer.indices.len() * 2) };
        resource_write(i_h.id(), 0, i_data)?;
    }

    // Без пайплайна или буферов рисовать нечем — кадр просто пропускается.
    // Кэш bind group'ов берётся отдельным полем: остальные здесь заимствованы
    // на чтение, и одолжить ради него весь plugin было бы нельзя.
    let external_bind_groups = &mut plugin.external_bind_groups;
    if let (Some(pipeline), Some(v_h), Some(i_h), Some(uniform_bg)) =
        (&plugin.ui_pipeline, &plugin.vertex_buffer, &plugin.index_buffer, &plugin.uniform_bind_group)
    {
        recorder.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

        let mut current_index_offset = 0;
        for cmd in &renderer.draw_commands {
            match cmd {
                DrawCmd::Quads { count } => {
                    recorder.set_pipeline(pipeline);
                    recorder.set_vertex_buffer(0, v_h.id(), 0, (renderer.vertices.len() * vertex_size) as u64);
                    recorder.set_index_buffer(i_h.id(), IndexFormat::IdxUint16, 0, (renderer.indices.len() * 2) as u64);
                    recorder.set_bind_group(1, uniform_bg);
                    if let Some(atlas_bg) = renderer.atlas_bind_group.as_ref() {
                        recorder.set_bind_group(0, atlas_bg);
                    }
                    recorder.draw_indexed(current_index_offset..(*count + current_index_offset), 0, 0..1);
                    current_index_offset += *count;
                }
                DrawCmd::Scissor { x, y, width, height } => {
                    recorder.set_scissor_rect(*x, *y, *width, *height);
                }
                DrawCmd::ExternalImage { texture_id, index_count, .. } => {
                    match get_external_bind_group(external_bind_groups, renderer, *texture_id) {
                        Ok(bg) => {
                            recorder.set_bind_group(0, &bg);
                            recorder.draw_indexed(current_index_offset..(current_index_offset + *index_count), 0, 0..1);
                            current_index_offset += *index_count;
                            if let Some(atlas_bg) = renderer.atlas_bind_group.as_ref() {
                                recorder.set_bind_group(0, atlas_bg);
                            }
                        }
                        Err(e) => {
                            veldsdk::log::error!(target: "graphics", "Failed to create external bind group for texture {}: {}", texture_id, e);
                            current_index_offset += *index_count;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Выбрасывает из кэша bind group'ы текстур, которых в кадре больше нет.
///
/// Держать их «на всякий случай» нельзя: bind group на хосте хранит wgpu-ссылку
/// на текстуру, поэтому пока запись жива, видеопамять не освобождается даже
/// после того, как владелец освободил текстуру. Без этого каждая просмотренная
/// картинка оставалась бы в VRAM до конца сессии.
fn evict_unused_bind_groups(cache: &mut HashMap<u64, BindGroupId>, renderer: &GpuRenderer) {
    if cache.is_empty() { return; }
    cache.retain(|texture_id, _| {
        renderer.draw_commands.iter().any(|cmd| {
            matches!(cmd, DrawCmd::ExternalImage { texture_id: id, .. } if id == texture_id)
        })
    });
}

/// Bind group внешней текстуры — из кэша или новая. Кэш чистится
/// `evict_unused_bind_groups`.
fn get_external_bind_group(cache: &mut HashMap<u64, BindGroupId>, renderer: &GpuRenderer, texture_id: u64) -> anyhow::Result<BindGroupId> {
    if texture_id == 0 {
        return Err(anyhow!("Invalid texture_id: 0"));
    }
    if let Some(bg) = cache.get(&texture_id) {
        return Ok(bg.clone());
    }

    let layout = renderer.atlas_layout.as_ref().ok_or_else(|| anyhow!("No atlas layout"))?;
    let view = gfx::create_texture_view(texture_id)?;
    let sampler = gfx::create_sampler(FilterMode::FiltLinear, FilterMode::FiltLinear)?;
    let bg = gfx::create_bind_group(
        &format!("External Image BG {}", texture_id),
        layout,
        vec![gfx::texture_entry(0, &view), gfx::sampler_entry(1, &sampler)],
    )?;

    cache.insert(texture_id, bg.clone());
    veldsdk::log::debug!(target: "graphics", "Created external bind group {:?} for texture {}", bg, texture_id);
    Ok(bg)
}

/// Ленивое создание всего, что живёт дольше кадра: layout'ы, пайплайн, буферы,
/// атлас. Каждый шаг проверяет своё и ничего не пересоздаёт.
pub fn ensure_resources(plugin: &mut PluginUiState, renderer: &mut GpuRenderer, surface_format: i32) -> anyhow::Result<()> {
    ensure_atlas_layout(renderer)?;
    ensure_uniform_layout(plugin)?;
    ensure_pipeline(plugin, renderer, surface_format)?;
    ensure_uniform_buffer(plugin)?;
    ensure_atlas_texture(renderer)?;
    ensure_atlas_bind_group(renderer)?;
    Ok(())
}

fn ensure_atlas_layout(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.atlas_layout.is_none() {
        renderer.atlas_layout = Some(gfx::create_bind_group_layout("Iced Atlas BGL", vec![
            gfx::texture_layout_entry(0, VISIBILITY_FRAGMENT),
            gfx::sampler_layout_entry(1, VISIBILITY_FRAGMENT),
        ])?);
    }
    Ok(())
}

fn ensure_uniform_layout(plugin: &mut PluginUiState) -> anyhow::Result<()> {
    if plugin.uniform_layout.is_none() {
        plugin.uniform_layout = Some(gfx::create_bind_group_layout("UI Uniform BGL", vec![
            gfx::uniform_buffer_layout_entry(0, VISIBILITY_VERTEX | VISIBILITY_FRAGMENT),
        ])?);
    }
    Ok(())
}

fn ensure_pipeline(plugin: &mut PluginUiState, renderer: &GpuRenderer, surface_format: i32) -> anyhow::Result<()> {
    if plugin.ui_pipeline.is_none() {
        let shader = gfx::create_shader(include_str!("shaders.wgsl"), "UI Shader")?;

        let mut bgl_ids = Vec::new();
        if let Some(layout) = renderer.atlas_layout.as_ref() { bgl_ids.push(layout.id()); }
        if let Some(layout) = plugin.uniform_layout.as_ref() { bgl_ids.push(layout.id()); }

        let pipeline = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "UI Pipeline".into(),
            vertex_entry: "vs_main".into(),
            fragment_entry: "fs_main".into(),
            target_format: surface_format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::module::renderer::Vertex>() as u64,
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
        })?;
        plugin.ui_pipeline = Some(pipeline);
    }
    Ok(())
}

/// Uniform-буфер и bind group поверх него. Буфер сразу заворачивается во
/// владельца: сорвись создание bind group — освободит его Drop, а голый id
/// остался бы висеть на хосте.
fn ensure_uniform_buffer(plugin: &mut PluginUiState) -> anyhow::Result<()> {
    if plugin.uniform_bind_group.is_some() {
        return Ok(());
    }
    let Some(layout) = plugin.uniform_layout.as_ref() else { return Ok(()) };

    let id = resource_alloc_buffer(16, buffer_usage::UNIFORM, false)
        .ok_or_else(|| anyhow!("Failed to allocate uniform buffer"))?;
    let region = OwnedResource::new(ResourceHandle { id, size: 16, ..Default::default() });
    let bind_group = gfx::create_bind_group(
        "UI Uniform BG", layout, vec![gfx::buffer_entry(0, region.id())],
    )?;

    plugin.uniform_buffer_region = Some(region);
    plugin.uniform_bind_group = Some(bind_group);
    Ok(())
}

fn ensure_atlas_texture(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.atlas_texture_id.is_none() {
        let (w, h) = renderer.atlas_dimensions();
        let usage = texture_usage::COPY_DST | texture_usage::TEXTURE_BINDING;
        if let Some(id) = resource_alloc_texture(w, h, TextureFormat::TexRgba8Unorm as i32, usage) {
            renderer.atlas_texture_id = Some(OwnedResource::new(ResourceHandle { id, size: 0 }));
            renderer.mark_atlas_dirty();
        }
    }
    Ok(())
}

fn ensure_atlas_bind_group(renderer: &mut GpuRenderer) -> anyhow::Result<()> {
    if renderer.atlas_bind_group.is_none() {
        let atlas_id = renderer.atlas_texture_id.as_ref().map(|t| t.id());
        if let (Some(atlas_texture), Some(layout)) = (atlas_id, renderer.atlas_layout.as_ref()) {
            let view = gfx::create_texture_view(atlas_texture)?;
            let sampler = gfx::create_sampler(FilterMode::FiltLinear, FilterMode::FiltLinear)?;
            renderer.atlas_bind_group = Some(gfx::create_bind_group(
                "Iced Atlas BG", layout,
                vec![gfx::texture_entry(0, &view), gfx::sampler_entry(1, &sampler)],
            )?);
        }
    }
    Ok(())
}
