//! Wheel step — one for the globe and for the viewing canvas.
//!
//! The file is included twice: by the module (`crate::module::wheel`) and by
//! the wrap crate (`veldmap_globe_wrap::wheel`, through `#[path]` in wrap.rs),
//! from where data-browser takes it for the canvas. The wheel is one and its
//! click is one: two different steps are something the user has no way to
//! learn, and two reconciled numbers hold only while someone keeps reconciling
//! them.

/// By how much one wheel click changes the zoom of the viewing canvas.
pub const ZOOM_PER_CLICK: f64 = 1.25;

/// By how much one click changes the globe camera height — the inverse of the
/// canvas step: on the canvas bigger means closer, on the globe nearer means
/// lower.
pub const ZOOM_PER_STEP: f64 = 1.0 / ZOOM_PER_CLICK;
