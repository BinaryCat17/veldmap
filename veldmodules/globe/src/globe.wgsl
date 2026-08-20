// Земля и сетка. Обе геометрии проходят через одну вершинную функцию: вершина
// у них одна и та же — точка поверхности со своей нормалью; различаются они
// только тем, чем красятся.
//
// Нормаль приходит атрибутом, а не выводится из позиции: позиция равна нормали
// только у шара, а поверхность здесь — эллипсоид WGS84 (см. geodesy.rs).
//
// Цвета согласованы с палитрой приложения (data-browser/theme.rs): LIT, ACCENT
// и OUTLINE — её PAGE, ACCENT и INK_MUTED, делённые на 255; SHADE и GRID —
// собственные полутона шара, в UI-палитре их нет. Числа именно sRGB, и это не
// небрежность: место под шар — обычная UNORM-текстура, и показывает её
// разметка тем же путём, что всякую другую картинку — линеаризует прочитанное
// сама (см. shaders.wgsl в ui-service). Записанное сюда линейным приехало бы на
// экран линейным же, то есть заметно темнее макета.

struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec3<f32>,
    _pad: f32,
    /// Размер кадра в пикселях — им ленты контуров меряют свою ширину и шаг
    /// штриха (см. vs_ribbon).
    viewport: vec2<f32>,
    _pad2: vec2<f32>,
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

const LIT    = vec3<f32>(0.984, 0.973, 0.945); // #FBF8F1 — бумага
const SHADE  = vec3<f32>(0.827, 0.788, 0.682); // #D3C9AE — она же в тени
const ACCENT = vec3<f32>(0.290, 0.478, 0.247); // #4A7A3F — акцент

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

const GRID    = vec3<f32>(0.659, 0.725, 0.608); // #A8B99B — сетка
const OUTLINE = vec3<f32>(0.361, 0.337, 0.282); // #5C5648 — приглушённые чернила

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

// Остальные, когда выбран один: тот же контур вполсилы. Лента выбранного и так
// заметна, но приглушённые соседи не спорят с ней за взгляд.
@fragment
fn fs_outline_dim(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(OUTLINE, line_alpha(in) * 0.35);
}

// --- Лента контура ---
// Линия в пайплайне всегда в пиксель шириной, а выделенный снимок должен быть
// виден среди полусотни соседних. Поэтому он обводится не линией, а полосой из
// треугольников: вершина контура приезжает сюда дважды — левым краем полосы и
// правым, — и разводит их в стороны этот шейдер.
//
// Разводит на экране, а не в мире: мировая ширина зависела бы от приближения —
// на отлёте полоса расплылась бы в пятно, вблизи истончилась бы в ту же линию.
// Экранная же остаётся собой на любом масштабе.
//
// Полуширину со знаком приносит сама вершина (`side`), а не константа отсюда:
// лент две — выделенного и едущего на шар, — и разной их делает как раз
// ширина. Знать её здесь означало бы завести вторую вершинную функцию, ничем,
// кроме числа, от этой не отличающуюся.

/// Насколько сильно позволено растянуть стык на изломе. Ус стыка тем длиннее,
/// чем острее угол, и на развороте почти назад он уходит в бесконечность;
/// потолок обрезает его, оставляя стык слегка срезанным.
const MITER_LIMIT: f32 = 3.0;

fn to_screen(clip: vec4<f32>) -> vec2<f32> {
    return clip.xy / clip.w * camera.viewport * 0.5;
}

fn along(tail: vec2<f32>, head: vec2<f32>) -> vec2<f32> {
    let delta = head - tail;
    let len = length(delta);
    // Совпавшие точки направления не задают: соседи вершины сходятся, когда
    // контур пришёл с повторённой вершиной.
    if len < 1e-5 {
        return vec2<f32>(1.0, 0.0);
    }
    return delta / len;
}

@vertex
fn vs_ribbon(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) prev: vec3<f32>,
    @location(3) next: vec3<f32>,
    // Полуширина ленты в пикселях кадра со знаком: минус — левый край, плюс —
    // правый.
    @location(4) side: f32,
) -> VsOut {
    var out: VsOut;
    out.world = position;
    out.normal = normal;

    let clip = camera.view_proj * vec4<f32>(position, 1.0);
    let clip_prev = camera.view_proj * vec4<f32>(prev, 1.0);
    let clip_next = camera.view_proj * vec4<f32>(next, 1.0);
    out.clip = clip;
    // Вершина или её сосед за камерой: делить на такое w нельзя, а звено всё
    // равно не видно — оставляем полосу схлопнутой в линию.
    if clip.w <= 0.0 || clip_prev.w <= 0.0 || clip_next.w <= 0.0 {
        return out;
    }

    let here = to_screen(clip);
    let back = along(to_screen(clip_prev), here);
    let ahead = along(here, to_screen(clip_next));
    // Нормаль стыка — биссектриса нормалей соседних звеньев, удлинённая
    // ровно настолько, чтобы полоса осталась своей ширины и на изломе.
    let n_back = vec2<f32>(-back.y, back.x);
    let n_ahead = vec2<f32>(-ahead.y, ahead.x);
    var miter = n_back + n_ahead;
    let len = length(miter);
    if len < 1e-5 {
        miter = n_ahead;
    } else {
        miter = miter / len;
    }
    let stretch = min(1.0 / max(dot(miter, n_ahead), 1.0 / MITER_LIMIT), MITER_LIMIT);

    let offset = miter * side * stretch;
    // Обратно в отсечение: экранные пиксели — это доли кадра, умноженные на w.
    out.clip = vec4<f32>(clip.xy + offset / (camera.viewport * 0.5) * clip.w, clip.z, clip.w);
    return out;
}

// Выбранный снимок: акцент в полную силу. Гаснет у края силуэта тем же
// правилом, что и линии, — иначе на самом краю полоса сгустилась бы в заливку.
@fragment
fn fs_ribbon(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(ACCENT, line_alpha(in));
}

/// Шаг штриховки в пикселях кадра: половина его закрашена, половина
/// выброшена. Связан с шириной ленты и потому не свободен — см.
/// `PENDING_HALF_PX` в outlines.rs.
const HATCH_PX: f32 = 5.0;

// Снимок, который на шар только едет: тот же приглушённый цвет, что у обычного
// контура, но лента штриховая. Штрих говорит «место занято, картинки пока
// нет» — сплошная лента этого не скажет, она уже занята выделенным.
//
// Шаг штриха меряется кадром, а не миром: в мире он зависел бы от приближения
// и на отлёте слился бы в сплошную полосу — ровно там, где отличать её от
// выделенной нужнее всего. Наклон — чтобы штрих не совпал с самой лентой: у
// контура снимка стороны почти всегда близки к горизонтали и вертикали.
@fragment
fn fs_ribbon_pending(in: VsOut) -> @location(0) vec4<f32> {
    if fract((in.clip.x + in.clip.y) / HATCH_PX) > 0.5 {
        discard;
    }
    return vec4<f32>(OUTLINE, line_alpha(in));
}

// --- Наложения ---
// Варп-сетка тайла-носителя: позиции уже в мире, интерполяцию между узлами
// делает GPU. Тайл — sRGB-текстура, сэмплер отдаёт линейное, а таргет —
// UNORM с sRGB-числами (см. заголовок), поэтому перед записью значение
// кодируется обратно — тем же правилом, что у канвы превью.

struct OverlayVsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_overlay(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
) -> OverlayVsOut {
    var out: OverlayVsOut;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.alpha = alpha;
    return out;
}

@group(1) @binding(0) var overlay_texture: texture_2d<f32>;
@group(1) @binding(1) var overlay_sampler: sampler;

fn encode_srgb(linear: vec3<f32>) -> vec3<f32> {
    let lo = linear * 12.92;
    let hi = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, linear <= vec3<f32>(0.0031308));
}

@fragment
fn fs_overlay(in: OverlayVsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(overlay_texture, overlay_sampler, in.uv);
    // Альфа носителя проходит как есть — прозрачные поля квиклуков показывают
    // Землю, — а прозрачность слоя её домножает: полупрозрачным становится то,
    // что было видно, и не проступает то, чего не было.
    return vec4<f32>(encode_srgb(sampled.rgb), sampled.a * in.alpha);
}
