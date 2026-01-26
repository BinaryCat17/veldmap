struct CameraUniform {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) lat_lon: vec2<f32>,
};

struct VertexOutput {

    @builtin(position) clip_position: vec4<f32>,

    @location(0) color: vec3<f32>,

    @location(1) lat_lon: vec2<f32>,

    @location(2) world_position: vec3<f32>,

};



@vertex

fn vs_main(

    model: VertexInput,

) -> VertexOutput {

    var out: VertexOutput;

    out.clip_position = camera.view_proj * vec4<f32>(model.position, 1.0);

    out.color = model.color;

    out.lat_lon = model.lat_lon;

    out.world_position = model.position;

    return out;

}



@fragment

fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {

    let lat = in.lat_lon.x;

    let lon = in.lat_lon.y;

    

    // Процедурная сетка

    let grid_size = 5.0;

    let grid_lat = abs(fract(lat / grid_size + 0.5) - 0.5) / fwidth(lat / grid_size);

    let grid_lon = abs(fract(lon / grid_size + 0.5) - 0.5) / fwidth(lon / grid_size);

    let line = min(grid_lat, grid_lon);

    let grid_color = smoothstep(1.0, 0.0, line);

    

    // Базовое освещение (directional light from sun)

    let normal = normalize(in.world_position); // Упрощенная нормаль для сферы

    let light_dir = normalize(vec3<f32>(1.0, 1.0, 1.0));

    let diff = max(dot(normal, light_dir), 0.2); // 0.2 - ambient

    

    let base_color = in.color * diff;

    let final_color = mix(base_color, vec3<f32>(0.3, 0.4, 0.6), grid_color * 0.5);

    

    return vec4<f32>(final_color, 1.0);

}
