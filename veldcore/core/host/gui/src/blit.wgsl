struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) in_vertex_index: u32,
) -> VertexOutput {
    var out: VertexOutput;
    // Полноэкранный треугольник, гарантированно закрывающий весь экран
    // 0: (-1, -1), 1: (3, -1), 2: (-1, 3)
    let x = f32(i32(in_vertex_index & 1u) << 2u) - 1.0;
    let y = f32(i32(in_vertex_index & 2u) << 1u) - 1.0;
    
    // Но лучше использовать проверенную таблицу:
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0)
    );
    
    out.clip_position = vec4<f32>(pos[in_vertex_index], 0.0, 1.0);
    // UV координаты: 0,0 в левом верхнем углу, 1,1 в правом нижнем.
    // NDC: -1,1 (top-left) -> 0,0 UV
    // NDC:  1,-1 (bottom-right) -> 1,1 UV
    out.tex_coords = vec2<f32>(out.clip_position.x * 0.5 + 0.5, 0.5 - out.clip_position.y * 0.5);
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_diffuse, s_diffuse, in.tex_coords);
}
