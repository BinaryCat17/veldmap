# 0007 — Overlay accuracy and the camera floor

Status: accepted (2026-09-06). Written down from the analysis of 2026-08-26.

## Context

The owner's data will come at centimetres — RTK contours, drone orthophotos —
and the question was whether the globe places a raster to that accuracy, the
suspected culprit being the arithmetic of latitude and longitude. The
placement is held by a test, `a_raster_lands_where_its_binding_puts_it`
(`globe/src/overlay.rs`): it draws five bindings in UTM zone 37 — an
orthophoto at 2 cm per pixel, a Sentinel-2 granule at 10 m, a Landsat scene at
30 m, the granule again at its fourth pyramid level, and an 8000×5000
orthophoto rotated by 30° — and measures the worst drift of the drawn cell from
the binding along the surface, with the f32 split of vertices included.

## Decision

The rule the test holds: the drift stays under a thousandth of the pixel of
the level the cell is drawn with (the worst case is 1.8·10⁻⁴, at the coarse
level), under 2 mm for a raster drawn at its own resolution, and under 2 cm at
any level. The coordinates are not what limits centimetre data; the camera is.
The camera cannot come closer than `HEIGHT_RANGE_M.0`, ten `SURFACE_LIFT_M`
(`camera.rs`, `mesh.rs`): at 800 m with `FOV_Y_DEG` of 50° a frame 1536 pixels
tall covers 2·800·tan(25°) ≈ 746 m, 0.49 m per screen pixel — a 2 cm
orthophoto is drawn twenty-four times coarser than it is. That floor stays as
it is: it is visible behaviour, and lowering it is the owner's call, one
factor in one constant.

## Rejected

Reworking the coordinate arithmetic for accuracy: measured, it is three
orders of magnitude finer than the finest data. Lowering the floor as part of
this decision: the lift under the camera is also what keeps the ray from
piercing the surface (`surface_at`), and the floor is tied to it.

## Consequences

Centimetre data first need a way from a local file to the globe and a lower
floor; both are product decisions, not accuracy work. Any change to the
drawing of cells must keep the test's rule, and a change of `SURFACE_LIFT_M`
moves the floor with it.
