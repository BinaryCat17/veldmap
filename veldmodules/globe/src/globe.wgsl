// Earth and graticule. Both geometries go through one vertex function: their
// vertex is one and the same — a surface point with its own normal; they differ
// only in how they are colored.
//
// The normal arrives as an attribute rather than derived from position: position
// equals the normal only on a sphere, while the surface here is the WGS84
// ellipsoid (see geodesy.rs).
//
// The colors match the application palette (data-browser/theme.rs): LIT, ACCENT
// and OUTLINE are its PAGE, ACCENT and INK_MUTED divided by 255; SHADE and GRID
// are the sphere's own midtones and have no UI-palette counterparts. The numbers
// are deliberately sRGB, and this is not sloppiness: the place for the sphere is
// an ordinary UNORM texture, and the markup shows it the same way as any other
// image — linearizing what it reads itself (see shaders.wgsl in ui-service).
// Written here as linear, it would arrive on screen still linear, that is,
// noticeably darker than the mock-up.

struct Camera {
    /// No translation: the eye stands at the origin, and it is subtracted below,
    /// from the vertex itself. The translation is a number of order one, and
    /// arriving here in f32 it would eat eight tenths of a meter from any vertex,
    /// however precise.
    view_proj: mat4x4<f32>,
    /// The eye in halves — high and low (see geodesy::parts in globe).
    eye: vec3<f32>,
    _pad: f32,
    eye_low: vec3<f32>,
    _pad2: f32,
    /// Frame size in pixels — the selected-outline ribbon measures its width
    /// with it (see vs_ribbon). The hatching does not need it: it computes from
    /// the fragment coordinate, which is already in pixels.
    viewport: vec2<f32>,
    /// Where the ripple wave stands (fraction of the period) and how visible it
    /// is. Both quantities are the same for the whole frame: what ripples is the
    /// layer's loading step, not an individual cell, and the cell only brings the
    /// flag — whether to ripple or not.
    phase: f32,
    ripple: f32,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Vertex measured from the eye. Both points arrive in halves, and the halves are
// subtracted separately: the difference of the high parts of nearby numbers of
// one magnitude in f32 is **exact**, so it cancels without loss, while the low
// halves carry the detail. Hence the coordinate step goes by the distance to the
// eye rather than by the Earth's radius: at a kilometer it is fractions of a
// micron instead of eighty centimeters.
//
// The order of the parentheses here is load-bearing: add the high to the low
// first — and the detail burns up in the very first addition, before the
// subtraction.
fn relative(high: vec3<f32>, low: vec3<f32>) -> vec3<f32> {
    return (high - camera.eye) + (low - camera.eye_low);
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) position_low: vec3<f32>,
    @location(2) normal: vec3<f32>,
) -> VsOut {
    var out: VsOut;
    let here = relative(position, position_low);
    out.clip = camera.view_proj * vec4<f32>(here, 1.0);
    out.world = here;
    out.normal = normal;
    return out;
}

// How much the surface faces us: 1 — the point straight under the camera, 0 —
// the silhouette edge. Both fragment shaders need one and the same quantity, and
// it means one and the same thing for them — closeness to the edge.
fn facing(in: VsOut) -> f32 {
    // The eye stands at zero and the world is measured from it — so the view
    // direction is the vertex itself, taken backwards.
    let view = normalize(-in.world);
    return clamp(dot(normalize(in.normal), view), 0.0, 1.0);
}

// Direction to the sun. Not tied to the camera: a shadow crawling after the
// view reads as a material defect, not as lighting.
const LIGHT = vec3<f32>(0.42, 0.60, 0.68);

const LIT    = vec3<f32>(0.984, 0.973, 0.945); // #FBF8F1 — paper
const SHADE  = vec3<f32>(0.827, 0.788, 0.682); // #D3C9AE — the same in shadow
const ACCENT = vec3<f32>(0.290, 0.478, 0.247); // #4A7A3F — accent

@fragment
fn fs_body(in: VsOut) -> @location(0) vec4<f32> {
    // Half the light is directional, half is flat: pure lambertian turns the
    // night side black, while the Earth must read as a whole.
    let lambert = clamp(dot(normalize(in.normal), normalize(LIGHT)), 0.0, 1.0);
    let body = mix(SHADE, LIT, 0.45 + 0.55 * lambert);

    // Rim: near the edge the normal is almost perpendicular to the view. It also
    // outlines the silhouette — no separate contour line is needed for that.
    let rim = pow(1.0 - facing(in), 3.0);
    return vec4<f32>(mix(body, ACCENT, rim * 0.9), 1.0);
}

const GRID    = vec3<f32>(0.659, 0.725, 0.608); // #A8B99B — graticule
const OUTLINE = vec3<f32>(0.361, 0.337, 0.282); // #5C5648 — muted ink

// Lines lying on the surface fade out at the silhouette edge: there they
// converge at an acute angle to the view and pack into a solid fill. The
// quantity is one for all — otherwise the outline and the graticule would
// dissolve at different depths and come apart.
fn line_alpha(in: VsOut) -> f32 {
    return smoothstep(0.0, 0.35, facing(in));
}

@fragment
fn fs_grid(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(GRID, line_alpha(in));
}

// Image outlines: darker than the graticule so they read on top of it.
@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(OUTLINE, line_alpha(in));
}

// The others, when one is selected: the same outline at half strength. The
// selected ribbon is conspicuous anyway, but muted neighbors do not compete
// with it for the eye.
@fragment
fn fs_outline_dim(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(OUTLINE, line_alpha(in) * 0.35);
}

// --- Selected outline ribbon ---
// A line in the pipeline is always one pixel wide, while the selected image must
// be visible among fifty neighbors. So it is outlined not by a line but by a
// strip of triangles: the outline vertex arrives here twice — as the left edge
// of the strip and the right — and this shader spreads them apart.
//
// It spreads them on the screen, not in the world: a world-space width would
// depend on zoom — far away the strip would blur into a spot, up close it would
// thin into that same line. A screen-space one stays itself at any scale.

/// Ribbon width in frame pixels.
const RIBBON_PX: f32 = 5.0;
/// How far the joint is allowed to stretch at a bend. The joint's miter is the
/// longer the sharper the angle, and on a near-reversal it goes to infinity;
/// the ceiling trims it, leaving the joint slightly beveled.
const MITER_LIMIT: f32 = 3.0;

fn to_screen(clip: vec4<f32>) -> vec2<f32> {
    return clip.xy / clip.w * camera.viewport * 0.5;
}

fn along(tail: vec2<f32>, head: vec2<f32>) -> vec2<f32> {
    let delta = head - tail;
    let len = length(delta);
    // Coincident points give no direction: a vertex's neighbors converge when
    // the outline arrives with a repeated vertex.
    if len < 1e-5 {
        return vec2<f32>(1.0, 0.0);
    }
    return delta / len;
}

@vertex
fn vs_ribbon(
    @location(0) position: vec3<f32>,
    @location(1) position_low: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) prev: vec3<f32>,
    @location(4) next: vec3<f32>,
    @location(5) side: f32,
) -> VsOut {
    var out: VsOut;
    let at_eye = relative(position, position_low);
    out.world = at_eye;
    out.normal = normal;

    // Neighbors arrive as offsets from this vertex rather than at their own
    // places: their high halves coincide with its own, and a difference taken
    // here would give zero where the neighbor is fifty meters away.
    let clip = camera.view_proj * vec4<f32>(at_eye, 1.0);
    let clip_prev = camera.view_proj * vec4<f32>(at_eye + prev, 1.0);
    let clip_next = camera.view_proj * vec4<f32>(at_eye + next, 1.0);
    out.clip = clip;
    // The vertex or its neighbor is behind the camera: dividing by such a w is
    // not allowed, and the segment is invisible anyway — leave the strip
    // collapsed into a line.
    if clip.w <= 0.0 || clip_prev.w <= 0.0 || clip_next.w <= 0.0 {
        return out;
    }

    let here = to_screen(clip);
    let back = along(to_screen(clip_prev), here);
    let ahead = along(here, to_screen(clip_next));
    // The joint normal is the bisector of the neighboring segments' normals,
    // lengthened just enough for the strip to keep its width at a bend too.
    let n_back = vec2<f32>(-back.y, back.x);
    let n_ahead = vec2<f32>(-ahead.y, ahead.x);
    var miter = n_back + n_ahead;
    let len = length(miter);
    if len < 1e-5 {
        miter = n_ahead;
    } else {
        miter = miter / len;
    }
    let stretch = 1.0 / max(dot(miter, n_ahead), 1.0 / MITER_LIMIT);

    let offset = miter * side * RIBBON_PX * 0.5 * stretch;
    // Back to clip space: screen pixels are frame fractions multiplied by w.
    out.clip = vec4<f32>(clip.xy + offset / (camera.viewport * 0.5) * clip.w, clip.z, clip.w);
    return out;
}

// The selected image: the accent at full strength. It fades at the silhouette
// edge by the same rule as the lines — otherwise at the very edge the strip
// would pack into a fill.
@fragment
fn fs_ribbon(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(ACCENT, line_alpha(in));
}

// --- Reserved area ---
// An image that is only on its way to the globe: the place where it will land is
// hatched, and its edge is outlined with the usual contour. There is no picture
// yet — and this is the only way to say of it "it will be here" rather than
// "somewhere".

/// Hatching step in frame pixels: half of it is painted, half discarded.
///
/// Measured by the frame, not the world: in the world the step would depend on
/// zoom — far away the area would merge into a solid fill, up close a hatch line
/// would sprawl across half the screen. The slant keeps the hatch from aligning
/// with the area's edges: an image outline most often has sides close to
/// horizontal and vertical.
const HATCH_PX: f32 = 18.0;

@fragment
fn fs_hatch(in: VsOut) -> @location(0) vec4<f32> {
    if fract((in.clip.x + in.clip.y) / HATCH_PX) > 0.5 {
        discard;
    }
    // Weaker than the outline: the hatching covers a whole area, and at full
    // strength it would compete both with the outline itself and with what
    // already lies on the globe nearby.
    return vec4<f32>(OUTLINE, line_alpha(in) * 0.5);
}

// --- Overlays ---
// The carrier tile's warp mesh: positions are already in world space, and the
// GPU interpolates between the nodes. The tile is an sRGB texture, the sampler
// hands out linear values, and the target is UNORM holding sRGB values (see the
// header), so before writing, the value is encoded back — by the same rule as
// the preview canvas.

struct OverlayVsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    /// Zero — the cell is covered by what it should be and does not ripple.
    /// Otherwise — its phase offset, 0..1: each cell has its own, so a loading
    /// place shimmers instead of blinking all at once (see overlay::ripple_of in
    /// globe).
    @location(2) ripple: f32,
};

@vertex
fn vs_overlay(
    @location(0) position: vec3<f32>,
    @location(1) position_low: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) alpha: f32,
    @location(4) ripple: f32,
) -> OverlayVsOut {
    var out: OverlayVsOut;
    out.clip = camera.view_proj * vec4<f32>(relative(position, position_low), 1.0);
    out.uv = uv;
    out.alpha = alpha;
    out.ripple = ripple;
    return out;
}

@group(1) @binding(0) var overlay_texture: texture_2d<f32>;
@group(1) @binding(1) var overlay_sampler: sampler;

fn encode_srgb(linear: vec3<f32>) -> vec3<f32> {
    let lo = linear * 12.92;
    let hi = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, linear <= vec3<f32>(0.0031308));
}

/// The blue of the loading: a cold color that belongs to neither the Earth, nor
/// the outlines, nor the accent. sRGB like the other numbers here, and it does
/// not pass through encode_srgb — that one is only for sampled values.
const LOADING = vec3<f32>(0.247, 0.443, 0.643); // #3F71A4

/// How much the ripple overrides the picture at its strongest blink. "Barely
/// visible" is about it: the image must show through it, not the blue.
const RIPPLE_DEPTH: f32 = 0.30;

const TAU: f32 = 6.2831855;

@fragment
fn fs_overlay(in: OverlayVsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(overlay_texture, overlay_sampler, in.uv);
    var rgb = encode_srgb(sampled.rgb);
    // The cell brings the flag and its own offset, the frame brings the phase
    // itself and the strength. The strength grows toward the end of the loading
    // step: the ripple is the more visible the closer the sharp picture is.
    if in.ripple > 0.0 {
        let wave = 0.5 + 0.5 * sin(TAU * (camera.phase + in.ripple));
        rgb = mix(rgb, LOADING, RIPPLE_DEPTH * camera.ripple * wave);
    }
    // The carrier's alpha passes as is — the transparent fields of quicklooks
    // show the Earth — while the layer's opacity multiplies it: what was visible
    // becomes translucent, and what was not does not show through. The ripple
    // does not affect it: it paints what is visible rather than reveals what is
    // invisible, and a second multiplication by the layer's opacity would dim
    // it by half where the slider has already lowered it.
    return vec4<f32>(rgb, sampled.a * in.alpha);
}
