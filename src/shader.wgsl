struct CameraUniform {
    view_inv: mat4x4<f32>,
    proj_inv: mat4x4<f32>,
    position: vec3<f32>,
    padding: f32,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(0) @binding(1)
var texture_dem: texture_2d<f32>;
@group(0) @binding(2)
var sampler_dem: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(in_vertex_index) << 1u & 2) - 1.0;
    let y = f32(i32(in_vertex_index) & 2) - 1.0;
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

const WGS84_A: f32 = 6378137.0;
const WGS84_B: f32 = 6356752.314245;

fn cartesian_to_latlon(p: vec3<f32>) -> vec2<f32> {
    let lon = atan2(p.z, p.x);
    let lat = atan2(p.y, length(p.xz));
    return vec2<f32>(lat, lon);
}

// Получаем высоту из DEM по мировым координатам (UV 0-1 внутри тайла)
fn get_height_uv(uv: vec2<f32>) -> f32 {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return 0.0;
    }
    return textureSampleLevel(texture_dem, sampler_dem, uv, 0.0).r;
}

fn get_uv_from_p(p: vec3<f32>) -> vec2<f32> {
    let latlon = cartesian_to_latlon(p);
    let lat_deg = latlon.x * 57.2957795131;
    let lon_deg = latlon.y * 57.2957795131;
    // Временная привязка для теста: Ростов-на-Дону (47N, 39E)
    return vec2<f32>((lon_deg - 39.0) + 0.5, (47.0 - lat_deg) + 0.5);
}

// Расчет нормали поверхности на основе градиента высот
fn calculate_terrain_normal(p: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let eps = 0.001; // Шаг для поиска градиента
    let h_center = get_height_uv(uv);
    let h_x = get_height_uv(uv + vec2<f32>(eps, 0.0));
    let h_y = get_height_uv(uv + vec2<f32>(0.0, eps));
    
    // Масштабируем градиент (здесь нужна привязка к реальному размеру пикселя в метрах)
    let terrain_scale = 100.0; 
    let normal_tangent = normalize(vec3<f32>(h_center - h_x, eps * terrain_scale, h_center - h_y));
    
    // Переводим из локального пространства тайла в мировое (упрощенно - используем нормаль эллипсоида как базис)
    let n_ellips = normalize(p);
    return n_ellips + (normal_tangent.x * vec3<f32>(1.0, 0.0, 0.0) + normal_tangent.z * vec3<f32>(0.0, 0.0, 1.0)) * 0.5;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let ndc = vec4<f32>(in.uv.x * 2.0 - 1.0, (1.0 - in.uv.y) * 2.0 - 1.0, 0.0, 1.0);
    var target = camera.proj_inv * ndc;
    target = target / target.w;
    let rd = normalize((camera.view_inv * vec4<f32>(target.xyz, 0.0)).xyz);
    let ro = camera.position;

    // Внешний контур (эллипсоид + макс высота гор)
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
    var current_uv: vec2<f32>;
    
    for (var i = 0; i < steps; i++) {
        p = ro + rd * t;
        current_uv = get_uv_from_p(p);
        let h_real = get_height_uv(current_uv);
        
        // Точный радиус эллипсоида
        let latlon = cartesian_to_latlon(p);
        let cos_lat = cos(latlon.x);
        let sin_lat = sin(latlon.x);
        let r_ellips = sqrt(1.0 / ( (cos_lat*cos_lat)/(WGS84_A*WGS84_A) + (sin_lat*sin_lat)/(WGS84_B*WGS84_B) ));
        
        if (length(p) < r_ellips + h_real) {
            hit = true;
            break;
        }
        t += step_size;
    }

    if (!hit) {
        return vec4<f32>(0.002, 0.005, 0.02, 1.0);
    }

    // Освещение с учетом рельефа
    let h = get_height_uv(current_uv);
    let normal = normalize(calculate_terrain_normal(p, current_uv));
    let sun_dir = normalize(vec3<f32>(1.0, 1.0, 1.0));
    let diff = max(dot(normal, sun_dir), 0.1);

    // Цвет в зависимости от высоты
    var base_color: vec3<f32>;
    if (h < 0.0) {
        base_color = vec3<f32>(0.05, 0.15, 0.4); // Глубокая вода
    } else if (h < 2.0) {
        base_color = vec3<f32>(0.1, 0.4, 0.15); // Берег
    } else if (h < 50.0) {
        base_color = vec3<f32>(0.2, 0.5, 0.2); // Лес/Поле
    } else {
        base_color = vec3<f32>(0.5, 0.4, 0.3); // Горы
    }

    let final_color = base_color * diff;
    return vec4<f32>(final_color, 1.0);
}