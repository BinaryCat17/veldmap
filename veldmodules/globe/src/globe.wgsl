// Земля и сетка. Обе геометрии проходят через одну вершинную функцию: вершина
// у них одна и та же — точка поверхности со своей нормалью; различаются они
// только тем, чем красятся.
//
// Нормаль приходит атрибутом, а не выводится из позиции: позиция равна нормали
// только у шара, а поверхность здесь — эллипсоид WGS84 (см. geodesy.rs).
//
// Цвета линейные, а не sRGB, — в отличие от разметки, где их называют так же,
// как в макете. Причина та же, по которой разметке нужен перевод: таргет
// sRGB-формата, и GPU кодирует записанное сам. Рядом с каждым числом стоит его
// исходный вид из палитры приложения (data-browser/theme.rs), чтобы сверять
// было с чем.

struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.world = position;
    out.normal = normal;
    return out;
}

// Насколько поверхность повёрнута к нам: 1 — точка прямо под камерой, 0 — край
// силуэта. Обоим фрагментным шейдерам нужна одна и та же величина, и означает
// она у них одно и то же — близость к краю.
fn facing(in: VsOut) -> f32 {
    let view = normalize(camera.eye - in.world);
    return clamp(dot(normalize(in.normal), view), 0.0, 1.0);
}

// Направление на солнце. Не привязано к камере: тень, ползающая за взглядом,
// читается как дефект материала, а не как освещение.
const LIGHT = vec3<f32>(0.42, 0.60, 0.68);

const LIT    = vec3<f32>(0.965, 0.939, 0.880); // #FBF8F1 — бумага
const SHADE  = vec3<f32>(0.590, 0.545, 0.420); // #D3C9AE — она же в тени
const ACCENT = vec3<f32>(0.068, 0.195, 0.050); // #4A7A3F — акцент

@fragment
fn fs_body(in: VsOut) -> @location(0) vec4<f32> {
    // Половина света — направленная, половина ровная: чистый ламберт уводит
    // ночную сторону в чёрный, а Земля должна читаться целиком.
    let lambert = clamp(dot(normalize(in.normal), normalize(LIGHT)), 0.0, 1.0);
    let body = mix(SHADE, LIT, 0.45 + 0.55 * lambert);

    // Ободок: у края нормаль почти перпендикулярна взгляду. Он же и очерчивает
    // силуэт — отдельной линии по контуру для этого не нужно.
    let rim = pow(1.0 - facing(in), 3.0);
    return vec4<f32>(mix(body, ACCENT, rim * 0.9), 1.0);
}

const GRID    = vec3<f32>(0.391, 0.485, 0.328); // #A8B99B — сетка
const OUTLINE = vec3<f32>(0.107, 0.092, 0.064); // #5C5648 — приглушённые чернила

// Линии, лежащие на поверхности, гаснут у края силуэта: там они сходятся под
// острым углом к взгляду и сгущаются в сплошную заливку. Величина одна на всех
// — иначе контур и сетка растворялись бы на разной глубине и расходились.
fn line_alpha(in: VsOut) -> f32 {
    return smoothstep(0.0, 0.35, facing(in));
}

@fragment
fn fs_grid(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(GRID, line_alpha(in));
}

// Контуры снимков: темнее сетки, чтобы читаться поверх неё.
@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(OUTLINE, line_alpha(in));
}
