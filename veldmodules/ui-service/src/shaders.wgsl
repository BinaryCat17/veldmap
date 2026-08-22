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

// Цель рендера — sRGB-формата, и записанное GPU кодирует сам, то есть шейдер
// обязан отдавать линейное. Из вершин оно и приходит (см. Vertex::color), а
// вот текстуры — атлас глифов и чужие картинки — лежат в UNORM и хранят
// sRGB-числа: их приходится линеаризовать здесь. Альфа остаётся как есть —
// она не цвет и кодированию не подлежит.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let cutoff = step(c, vec3<f32>(0.04045));
    return mix(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, cutoff);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Mode 3.0 = Shadow: пятно по форме виджета, мягкое по краю.
    //
    // Форма здесь — сам виджет, а квад больше него: `local_pos` выходит за
    // форму со всех сторон — слева и сверху в минус, справа и снизу за её
    // размер, — и расстояние там считается тем же кодом. Ширину спада везёт
    // поле рамки: у тени рамки нет.
    if (in.mode > 2.5) {
        let dist = sd_rounded_box(in.local_pos, in.rect_size, in.radius);
        // Спад — половина размытия в каждую сторону от края, то есть всего оно
        // и есть: заявленная плотность набирается на `blur/2` внутрь, а наружу
        // на столько же сходит на нет. Растяни спад на весь `blur` в обе
        // стороны — и тень отдала бы половину того, что у неё просили.
        //
        // На нуле спад берёт производную: делить на ноль нельзя, а жёсткая тень
        // со смещением — законный запрос.
        let ramp = max(in.border_width * 0.5, fwidth(dist));
        return vec4<f32>(in.color.rgb, in.color.a * (1.0 - smoothstep(-ramp, ramp, dist)));
    }

    // Mode 2.0 = External Image
    if (in.mode > 1.5) {
        let tex_sample = textureSample(t_diffuse, s_diffuse, in.tex_coords);
        return vec4<f32>(srgb_to_linear(tex_sample.rgb), in.color.a * tex_sample.a);
    }

    // Mode 1.0 = SDF Quad, Mode 0.0 = Text/Texture (Atlas)
    if (in.mode > 0.5) {
        // Fast path for simple rectangles with no radius and no border
        if (in.radius < 0.05 && in.border_width < 0.05) {
            return in.color;
        }

        let dist = sd_rounded_box(in.local_pos, in.rect_size, in.radius);
        let smoothing = fwidth(dist);
        
        // Linear smoothing instead of smoothstep for performance
        let alpha = clamp(0.5 - dist / smoothing, 0.0, 1.0);
        
        var res_color = in.color.rgb;
        var res_alpha = in.color.a * alpha;
        
        if (in.border_width > 0.01) {
            let interior_dist = dist + in.border_width;
            let border_mask = clamp(0.5 - dist / smoothing, 0.0, 1.0) - 
                              clamp(0.5 - interior_dist / smoothing, 0.0, 1.0);
            
            res_color = mix(res_color, in.border_color.rgb, border_mask);
            res_alpha = max(res_alpha, border_mask * in.border_color.a);
        }
        
        return vec4<f32>(res_color, res_alpha);
    }

    // Text/Texture mode
    let tex_sample = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    
    // Check if grayscale or color (e.g. emoji)
    let is_grayscale = abs(tex_sample.r - tex_sample.g) < 0.01 && abs(tex_sample.g - tex_sample.b) < 0.01;
    
    if (is_grayscale) {
        return vec4<f32>(in.color.rgb, in.color.a * tex_sample.a);
    } else {
        return vec4<f32>(srgb_to_linear(tex_sample.rgb), in.color.a * tex_sample.a);
    }
}
