// Квады тайлов: позиции приходят уже в NDC, текстура — sRGB-тайл.
//
// Таргет — UNORM с sRGB-числами (разметка сэмплит его как обычную картинку и
// линеаризует сама, см. SURFACE_FORMAT у владельца), а сэмплер sRGB-текстуры
// отдаёт линейное — поэтому фильтрация идёт в линейном пространстве, а перед
// записью значение кодируется обратно в sRGB здесь.

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) glow: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) glow: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.glow = in.glow;
    return out;
}

@group(0) @binding(0) var tile_texture: texture_2d<f32>;
@group(0) @binding(1) var tile_sampler: sampler;

fn encode_srgb(linear: vec3<f32>) -> vec3<f32> {
    let lo = linear * 12.92;
    let hi = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, linear <= vec3<f32>(0.0031308));
}

// Синева грузящегося — та же, что у шара (globe.wgsl): работа одна и та же, и
// два разных цвета для неё читались бы как два разных события. Через
// encode_srgb она не проходит: он только для сэмплированного.
const LOADING = vec3<f32>(0.247, 0.443, 0.643); // #3F71A4

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(tile_texture, tile_sampler, in.uv);
    // Сила приходит уже с волной внутри: сетка канвы переписывается каждым
    // кадром, и печь волну дешевле на месте (см. gpu::Vertex::glow).
    let rgb = mix(encode_srgb(sampled.rgb), LOADING, in.glow);
    return vec4<f32>(rgb, sampled.a);
}
