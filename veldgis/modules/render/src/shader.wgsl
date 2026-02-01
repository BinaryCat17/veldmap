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

struct TileInfo {
    index: u32,
    zoom: u32,
    local_uv: vec2<f32>,
    valid: bool,
};

fn get_tile_info(p: vec3<f32>) -> TileInfo {
    let latlon = cartesian_to_latlon(p);
    let u_indir = (latlon.y / PI) * 0.5 + 0.5;
    let v_indir = 0.5 - (latlon.x / PI);
    
    // Sample indirection texture (64x32 grid)
    let indir_x = i32(u_indir * 64.0);
    let indir_y = i32(v_indir * 32.0);
    
    let tile_data = textureLoad(texture_indir, vec2<i32>(indir_x, indir_y), 0).rg;
    let tile_index = tile_data.r;
    let tile_zoom = tile_data.g;
    
    if (tile_index >= 254u) {
        return TileInfo(0u, 0u, vec2<f32>(0.0), false);
    }
    
    let scale = pow(2.0, f32(tile_zoom));
    let local_u = fract(u_indir * scale);
    let local_v = fract(v_indir * scale);
    
    return TileInfo(tile_index, tile_zoom, vec2<f32>(local_u, local_v), true);
}

fn sample_dem_raw(tile_index: u32, x: i32, y: i32) -> f32 {
    // 256x256 tiles. Wrap X, Clamp Y.
    let tx = u32((x + 256) % 256);
    let ty = u32(clamp(y, 0, 255));
    let idx = tile_index * (256u * 256u) + ty * 256u + tx;
    return dem_data[idx];
}

fn sample_dem_bilinear(info: TileInfo) -> f32 {
    if (!info.valid) { return -99999.0; }

    let u_px = info.local_uv.x * 256.0 - 0.5;
    let v_px = info.local_uv.y * 256.0 - 0.5;
    
    let x0 = i32(floor(u_px));
    let y0 = i32(floor(v_px));
    let x1 = x0 + 1;
    let y1 = y0 + 1;
    
    let wx = fract(u_px);
    let wy = fract(v_px);
    
    let h00 = sample_dem_raw(info.index, x0, y0);
    let h10 = sample_dem_raw(info.index, x1, y0);
    let h01 = sample_dem_raw(info.index, x0, y1);
    let h11 = sample_dem_raw(info.index, x1, y1);
    
    let h0 = mix(h00, h10, wx);
    let h1 = mix(h01, h11, wx);
    return mix(h0, h1, wy);
}

fn get_height(p: vec3<f32>) -> f32 {
    let info = get_tile_info(p);
    return sample_dem_bilinear(info);
}

fn calculate_terrain_normal(p: vec3<f32>) -> vec3<f32> {
    let info = get_tile_info(p);
    if (!info.valid) { return normalize(p); }
    
    let h_center = sample_dem_bilinear(info);
    
    // Dynamic epsilon: approx half a pixel size in meters
    // Earth circ ~ 40M meters. 
    // Grid size = 256 * 2^zoom
    let grid_dim = 256.0 * pow(2.0, f32(info.zoom));
    let pixel_size = 40000000.0 / grid_dim;
    let eps = pixel_size * 0.5; 
    
    let tangent = normalize(cross(p, vec3<f32>(0.0, 1.0, 0.0)));
    let bitangent = normalize(cross(p, tangent));
    
    let h_x = get_height(p + tangent * eps);
    let h_y = get_height(p + bitangent * eps);
    
    let n_ellips = normalize(p);
    // Gradient
    let grad_x = (h_center - h_x) / eps;
    let grad_y = (h_center - h_y) / eps;
    
    return normalize(n_ellips + tangent * grad_x + bitangent * grad_y);
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

        let h_dem = get_height(p);

        let h_total = max(h_dem, 0.0) + get_geoid_height(latlon);

        

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

    

    if (h_dem < -90000.0) {

        // Visual feedback for missing data: Dark magenta with a grid pattern

        let grid = sin(latlon.x * 50.0) * sin(latlon.y * 50.0);

        let color = select(vec3<f32>(0.1, 0.0, 0.1), vec3<f32>(0.2, 0.0, 0.2), grid > 0.0);

        return vec4<f32>(color, 1.0);

    }



    let normal = calculate_terrain_normal(p);

    let sun_dir = normalize(vec3<f32>(1.0, 1.0, 1.0));

    let diff = max(dot(normal, sun_dir), 0.1);



    var base_color = vec3<f32>(0.2, 0.5, 0.2);

    if (h_dem < 0.0) { base_color = vec3<f32>(0.05, 0.15, 0.4); }

    else if (h_dem > 100.0) { base_color = vec3<f32>(0.5, 0.4, 0.3); }



    return vec4<f32>(base_color * diff, 1.0);

}
