struct Globals {
    res: vec2<f32>,
};

@group(1) @binding(0)
var<uniform> globals: Globals;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) tex_coords: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    // Конвертация пикселей в NDC (-1..1). 
    // Iced выдает координаты в логических пикселях. 
    // globals.res должен быть тоже в логических пикселях (или мы должны учитывать scale factor).
    let x = (model.position.x / globals.res.x) * 2.0 - 1.0;
    let y = 1.0 - (model.position.y / globals.res.y) * 2.0;
    
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.color = model.color;
    out.tex_coords = model.tex_coords;
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;

@fragment

fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {

    // Выборка из RGBA текстуры (атласа)

    let tex_sample = textureSample(t_diffuse, s_diffuse, in.tex_coords);

    

    // Пиксель (0,0) зарезервирован для сплошного цвета (белый 255)

    if (in.tex_coords.x < 0.001 && in.tex_coords.y < 0.001) {

        return in.color;

    }

    

    // Если это обычный текст (белый глиф в атласе), применяем цвет вершины

    // Если это цветной эмодзи, используем цвета из атласа, но учитываем альфу вершины

    let is_color_glyph = abs(tex_sample.r - tex_sample.g) > 0.01 || abs(tex_sample.g - tex_sample.b) > 0.01;

    

    if (is_color_glyph) {

        return vec4<f32>(tex_sample.rgb, tex_sample.a * in.color.a);

    } else {

        // Обычный текст: цвет вершины * интенсивность (из любого канала, так как они равны для маски)

        return vec4<f32>(in.color.rgb, in.color.a * tex_sample.a);

    }

}
