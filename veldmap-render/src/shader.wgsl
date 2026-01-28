struct CameraUniform {
    view_inv: mat4x4<f32>,
    proj_inv: mat4x4<f32>,
    position: vec3<f32>,
    padding: f32,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(0) @binding(1)
var<storage, read> dem_data: array<f32>;
@group(0) @binding(2)
var sampler_dem: sampler;
@group(0) @binding(3)
var texture_geoid: texture_2d<f32>;
@group(0) @binding(4)
var texture_indir: texture_2d<u32>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    // Create one large triangle that covers the screen [-1, 1]
    let x = f32(i32(in_vertex_index) == 1) * 4.0 - 1.0;
    let y = f32(i32(in_vertex_index) == 2) * 4.0 - 1.0;
    
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

const WGS84_A: f32 = 6378137.0;
const WGS84_B: f32 = 6356752.314245;
const PI: f32 = 3.14159265359;

fn cartesian_to_latlon(p: vec3<f32>) -> vec2<f32> {
    let lon = atan2(p.z, p.x);
    let lat = atan2(p.y, length(p.xz));
    return vec2<f32>(lat, lon);
}

fn get_geoid_height(latlon: vec2<f32>) -> f32 {
    let u = (latlon.y / PI) * 0.5 + 0.5;
    let v = 0.5 - (latlon.x / PI);
    return textureSampleLevel(texture_geoid, sampler_dem, vec2<f32>(u, v), 0.0).r;
}

fn get_height(p: vec3<f32>) -> f32 {
    let latlon = cartesian_to_latlon(p);
    
    let u_indir = (latlon.y / PI) * 0.5 + 0.5;
    let v_indir = 0.5 - (latlon.x / PI);
    let tile_index = textureLoad(texture_indir, vec2<i32>(i32(u_indir * 128.0), i32(v_indir * 64.0)), 0).r;
    
    if (tile_index >= 254u) {
        return 0.0;
    }
    
    let local_u = fract(u_indir * 128.0);
    let local_v = fract(v_indir * 64.0);
    
    let tx = u32(local_u * 256.0);
    let ty = u32(local_v * 256.0);
    let idx = tile_index * (256u * 256u) + ty * 256u + tx;
    
    // Return height from storage buffer
    return dem_data[idx];
}

fn calculate_terrain_normal(p: vec3<f32>) -> vec3<f32> {
    let eps = 100.0; 
    let h_center = get_height(p);
    
    let tangent = normalize(cross(p, vec3<f32>(0.0, 1.0, 0.0)));
    let bitangent = normalize(cross(p, tangent));
    
    let h_x = get_height(p + tangent * eps);
    let h_y = get_height(p + bitangent * eps);
    
    let n_ellips = normalize(p);
    return normalize(n_ellips + (tangent * (h_center - h_x) / eps + bitangent * (h_center - h_y) / eps));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let ndc = vec4<f32>(in.uv.x * 2.0 - 1.0, (1.0 - in.uv.y) * 2.0 - 1.0, 0.0, 1.0);
    var target_pos = camera.proj_inv * ndc;
    target_pos = target_pos / target_pos.w;
    let rd = normalize((camera.view_inv * vec4<f32>(target_pos.xyz, 0.0)).xyz);
    let ro = camera.position;

    let inv_abc_top = vec3<f32>(1.0 / (WGS84_A + 9000.0), 1.0 / (WGS84_A + 9000.0), 1.0 / (WGS84_B + 9000.0));
    let ro_norm = ro * inv_abc_top;
    let rd_norm = rd * inv_abc_top;
    let a = dot(rd_norm, rd_norm);
    let b = 2.0 * dot(ro_norm, rd_norm);
    let c = dot(ro_norm, ro_norm) - 1.0;
    let det = b * b - 4.0 * a * c;
    
    if (det < 0.0) { return vec4<f32>(0.002, 0.005, 0.02, 1.0); }
    
    var t = (-b - sqrt(det)) / (2.0 * a);
    let t_max = (-b + sqrt(det)) / (2.0 * a);
    
    let steps = 160;
    let step_size = (t_max - t) / f32(steps);
    
    var hit = false;
    var p: vec3<f32>;
    
    for (var i = 0; i < steps; i++) {
        p = ro + rd * t;
        let latlon = cartesian_to_latlon(p);
        let h_total = get_height(p) + get_geoid_height(latlon);
        
        let cos_lat = cos(latlon.x);
        let sin_lat = sin(latlon.x);
        let r_ellips = sqrt(1.0 / ( (cos_lat*cos_lat)/(WGS84_A*WGS84_A) + (sin_lat*sin_lat)/(WGS84_B*WGS84_B) ));
        
        if (length(p) < r_ellips + h_total) {
            hit = true;
            break;
        }
        t += step_size;
    }

    if (!hit) { return vec4<f32>(0.002, 0.005, 0.02, 1.0); }

    let latlon = cartesian_to_latlon(p);
    let h_dem = get_height(p);
    let normal = calculate_terrain_normal(p);
    let sun_dir = normalize(vec3<f32>(1.0, 1.0, 1.0));
    let diff = max(dot(normal, sun_dir), 0.1);

    var base_color = vec3<f32>(0.2, 0.5, 0.2);
    if (h_dem < 0.0) { base_color = vec3<f32>(0.05, 0.15, 0.4); }
    else if (h_dem > 100.0) { base_color = vec3<f32>(0.5, 0.4, 0.3); }

    return vec4<f32>(base_color * diff, 1.0);
}