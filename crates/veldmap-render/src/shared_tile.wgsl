@group(0) @binding(0)
var tile_texture: texture_2d<f32>;
@group(1) @binding(0)
var tile_sampler: sampler;

struct Viewport {
    transform: vec4<f32>,
};
@group(2) @binding(0)
var<uniform> viewport: Viewport;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let angle = viewport.transform.w;
    let rotated = vec2<f32>(
        input.position.x * cos(angle) - input.position.y * sin(angle),
        input.position.x * sin(angle) + input.position.y * cos(angle),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(
        rotated * viewport.transform.z + viewport.transform.xy,
        0.0,
        1.0,
    );
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tile_texture, tile_sampler, input.uv);
}
