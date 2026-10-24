// Tile quads: positions arrive already in NDC, the texture is an sRGB tile.
//
// The target is UNORM carrying sRGB numbers (the layout samples it as an
// ordinary image and linearizes it itself, see SURFACE_FORMAT at the owner),
// while the sampler of an sRGB texture returns linear values — so filtering
// happens in linear space, and the value is encoded back to sRGB here before
// the write.

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

// The loading tint is the same as on the globe (globe.wgsl): the work is the
// same, and two different colors for it would read as two different events. It
// does not go through encode_srgb: that one is only for sampled values.
const LOADING = vec3<f32>(0.247, 0.443, 0.643); // #3F71A4

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(tile_texture, tile_sampler, in.uv);
    // The strength arrives with the wave already baked in: the canvas mesh is
    // rewritten every frame, and baking the wave on the spot is cheaper (see
    // gpu::Vertex::glow).
    let rgb = mix(encode_srgb(sampled.rgb), LOADING, in.glow);
    return vec4<f32>(rgb, sampled.a);
}
