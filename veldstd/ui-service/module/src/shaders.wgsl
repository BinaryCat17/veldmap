struct Globals {
    res: vec2<f32>,
};

@group(1) @binding(0)
var<uniform> globals: Globals;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) tex_coords: vec2<f32>,
    @location(3) local_pos: vec2<f32>,
    @location(4) rect_size: vec2<f32>,
    @location(5) radius: f32,
    @location(6) mode: f32,
    @location(7) border_width: f32,
    @location(8) border_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) local_pos: vec2<f32>,
    @location(3) rect_size: vec2<f32>,
    @location(4) radius: f32,
    @location(5) mode: f32,
    @location(6) border_width: f32,
    @location(7) border_color: vec4<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let x = (model.position.x / globals.res.x) * 2.0 - 1.0;
    let y = 1.0 - (model.position.y / globals.res.y) * 2.0;
    
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.color = model.color;
    out.tex_coords = model.tex_coords;
    out.local_pos = model.local_pos;
    out.rect_size = model.rect_size;
    out.radius = model.radius;
    out.mode = model.mode;
    out.border_width = model.border_width;
    out.border_color = model.border_color;
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;

fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p - b * 0.5) - (b * 0.5 - r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (in.mode > 0.5) {
        let dist = sd_rounded_box(in.local_pos, in.rect_size, in.radius);
        let smoothing = fwidth(dist);
        
        // Маска основного тела
        let alpha = 1.0 - smoothstep(-0.5 * smoothing, 0.5 * smoothing, dist);
        
        // Маска рамки (если задана)
        if (in.border_width > 0.0) {
            // Расстояние до внутренней границы рамки
            let interior_dist = dist + in.border_width;
            let border_mask = smoothstep(-0.5 * smoothing, 0.5 * smoothing, dist) - 
                              smoothstep(-0.5 * smoothing, 0.5 * smoothing, interior_dist);
            
            // Смешиваем цвет фона и цвет рамки
            let res_color = mix(in.color.rgb, in.border_color.rgb, border_mask);
            let res_alpha = max(alpha, border_mask) * in.color.a;
            return vec4<f32>(res_color, res_alpha);
        }
        
        return vec4<f32>(in.color.rgb, in.color.a * alpha);
    }

    let tex_sample = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    let is_color_glyph = abs(tex_sample.r - tex_sample.g) > 0.01 || abs(tex_sample.g - tex_sample.b) > 0.01;
    
    if (is_color_glyph) {
        return vec4<f32>(tex_sample.rgb, tex_sample.a * in.color.a);
    } else {
        return vec4<f32>(in.color.rgb, in.color.a * tex_sample.a);
    }
}
