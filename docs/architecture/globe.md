# Globe geometry

How the globe draws what lies on the Earth and how it measures what it
draws: by what a cell is judged visible, how the warp mesh is built, what the
lift is and why there is one lift, how outlines are drawn and cut, how the
camera measures its height, holds a place under the wheel and flies to a
focus. Everything here is `veldmodules/globe/src/`, except which outline a
click on the globe picks — that is the data-browser's
(`veldmodules/data-browser/src/footprint.rs`). Bindings — which kind places a
raster and who supplies it — are [georeference.md](georeference.md); the
mechanics the globe shares with the canvas — the level table, the ladder, the
store and its appetite cap — are [viewing-pipeline.md](viewing-pipeline.md);
what the list shows of the globe is [screen.md](screen.md). Terms are in
[the glossary](../glossary.md).

## The Earth and its units

The Earth is the WGS84 ellipsoid (`geodesy.rs`), latitudes are geodetic and
the axes are ECEF, as data arrives in them. World coordinates are fractions
of the semi-major axis (`SEMI_MAJOR_M`), `1.0` at the equator, the same
scale on all three axes so that the shape is kept. One function turns
geodetic coordinates into cartesian (`geodesy::world`), and everything with
a place on the Earth — body, graticule, camera, outlines, overlay nodes —
goes through it, in `f64`; `f32` appears once, at the edge of the vertex
buffer. The body (`mesh.rs`) is rows of `LAT_STEP_DEG` by columns of
`LON_STEP_DEG`, the graticule a line every `GRID_STEP_DEG` cut into
`GRID_SEGMENTS` pieces, both in one pair of buffers. The body is the only
thing that writes depth (`gpu::render`); overlays, outlines and the
graticule compare against it and write nothing, and are drawn in that order
— body, overlays, then the lines, which must read over a scene.

## Positions in halves

A point of the surface is of the order of one, and one `f32` holds it to a
step of 2⁻²³ of `SEMI_MAJOR_M` — under a metre on the ground, enough for a
vertex seen from orbit and not for one seen from a kilometre. So every
vertex carries two halves (`geodesy::parts`), the coarse `f32` and the
remainder from it, and the eye travels the same way (`Camera::eye_parts`).
The shader subtracts eye from vertex half by half (`relative` in
`globe.wgsl`) and the view matrix carries no translation
(`Camera::view_projection`): the difference of two close `f32` numbers is
exact, the coarse halves cancel and the fine halves bring the detail, so the
precision of a drawn point comes from the distance to the eye, not from the
radius of the Earth (`a_node_carries_a_low_half_that_matters` in `mesh.rs`:
a centimetre is invisible to the coarse half and exact to the pair).

## The lift

Everything on the surface is drawn `mesh::SURFACE_LIFT_M` above the
ellipsoid — scenes, outlines, hatching, the graticule — and "what is under
the cursor" is answered on the same raised surface. One lift for all, and
not for the sake of layer order: the order is the draw order, and only the
body writes depth. It is one for the sake of the place on the screen: a
point at height `L` under a camera at height `H` moves away from the centre
of the frame by `H/(H−L)`, so two layers at different heights over one place
drift apart towards the edge of the frame, and an outline must lie on its
scene, not beside it. A depth bias in the pipeline is no substitute —
`graphics.proto` has no such field, and one would be tuned to the depth
buffer's precision and drift with the clipping planes.

The lift is a budget for the sag. A quad is flat on four surface points and
its middle dips under the surface by `R·(1−cos(θ/2))` (`mesh::sag_m`):
metres for a granule, hundreds of kilometres for the top cell of a global
raster. What dips under the body fails the depth test and is not drawn, so
everything laid on the surface divides its edges until the sag fits under
the lift with `mesh::CLEARANCE_M` to spare — the limiting arc is
`mesh::max_chord_deg`, and for a regular lattice the limiting quad side is
that over √2 (`mesh::max_quad_side_deg`): a quad is two triangles and the
middle of the hypotenuse dips deepest. What is divided that finely sits on
the common lift; what hits its own density ceiling rises above it by exactly
its residual sag — `mesh::lift_m(chord)` is the larger of `SURFACE_LIFT_M`
and the sag plus the clearance. Each of the three applies the rule with its
own step:

- an overlay cell is cut into `segments_for(span)` segments a side —
  `span / max_quad_side_deg`, clamped to `PATCH_SEGMENTS`..`MAX_PATCH_SEGMENTS`
  (`overlay.rs`); the ceiling exists because the top cell of a global raster
  spans 360°, which the rule alone would cut into over nine hundred segments
  a side, and what the division does not reach the height does;
- an outline edge is cut to half the limiting arc (`outlines::max_edge_deg`)
  — half, not the √2 share, because the fill fan's quads are not regular and
  their diagonal comes out twice the step; the height is then the common lift
  for all three views of an outline (`an_outline_lands_on_the_common_lift`),
  so hatching cannot show past its edge;
- `GRID_SEGMENTS` is chosen so that the graticule's longest piece — a
  parallel's at the equator — fits under the common lift, and is a multiple
  of the body's columns so its nodes lie on the body's edges.

A cell that rose troubles no neighbour: the lines are drawn after the
overlays and write no depth, and the rise shows only from where such a cell
fits the frame whole, where it is under a pixel. Held by the tests of
`mesh.rs` (`the_limit_chord_sags_exactly_into_the_lift`,
`what_divides_finely_lands_on_the_common_lift`,
`what_cannot_divide_rises_by_its_own_sag`,
`the_graticule_fits_under_the_common_lift`) and
`a_divided_cell_lands_on_the_common_lift` in `overlay.rs`.

The drawn radius is one number, `mesh::drawn_radius()` —
`1 + SURFACE_LIFT_M / SEMI_MAJOR_M` — and everything that measures the
camera over the **visible** measures to it: the angular size of the frame,
metres per pixel, the near plane, the reach of a cell's ball. A measure to
the ellipsoid would be off by the lift over the height less the lift: below
a percent at ten kilometres, a tenth at the floor.

## The warp mesh

An overlay is drawn as a warp mesh (`overlay::nodes_of`): a cell of a tile
is cut into `segments²` quads, every node goes through the binding to the
height `lift_m` names for the cell, and UV runs linearly across the carrier
tile (`spread`). The raster is never resampled; the distortion of the
projection is carried by the mesh. Which frames place a node by its own two
trigonometries and which do them once per row and per column
(`Frame::axial`) is in [georeference.md](georeference.md). The span the
density comes from is the longest row or column of the cell through the
binding, middle row included (`span_deg`): the top cell of a global raster
has both edges degenerate at the poles and all its length in the middle.
Distances along the binding are degrees of the coordinates themselves
(`arc_deg`), not a chord: an edge that runs the full circle is 360°, while
its chord is zero, and on that zero a global raster would lose both its
density and the width its level is chosen by. Nodes are computed in halves
and kept per cell while the layer's binding stands (`Raster::keep_mesh`).

## Which cells are visible

Of the shared mechanics the globe brings two things — the level under a
pixel and the cells visible at it, nine probes behind a cheap ball that is
believed only when it rejects ([viewing-pipeline.md](viewing-pipeline.md));
this section is what decides them.

A cell is visible when its nine probes — corners, edge midpoints and centre,
placed by the binding and lifted to `SURFACE_LIFT_M` (`Overlay::probes`) —
projected by the very matrix the frame is drawn with (`camera::project`)
give a box that intersects the frame (`Overlay::on_screen`). Intersection,
not containment: a cell can be larger than the screen, with no corner in the
frame and nothing but it in view. Nine and not four because the corners of
the top cell of a global raster are the poles: looking at the equator, all
four face away, and the step the scene should appear whole from would be
skipped in silence.

The far side of the Earth is rejected by the probes facing away from the eye
(`faces_eye`), but facing-away is a question about a cell, not a point:
between the probes lies unchecked ground, and a cell can be larger than the
visible piece of the Earth by orders of magnitude. A cell whose nine probes
all face away is therefore asked once more with a margin (`faces_cap`) and
forgiven by the covering radius of its probe lattice (`covering_radius_deg`)
— half the diagonal built on the longest steps between probes, in degrees.
Degrees, not a chord: at the top cell both "corners" are poles, and a quarter
of the chord between them names half the true angle. Only a rejecting nine
is asked again; an admitting one no margin can change.

The exact answer costs nine inverse bindings per cell, over thousands of
cells a level of a detailed raster, on every frame the camera moves; the
ball stands before it (`cull.rs`), tested against the four side planes of the
frame, read from the rows of the view-projection matrix, and against the
horizon taken by the polar radius — the lowest horizon there is; no near or
far plane, no transcendental call. The ball is built from the same nine
probes (`Overlay::ball`), widened by the same covering radius — in world
units through `drawn_radius` — plus the cell's own rise above the common
lift: it must cover what is drawn, not what was meant, and the top cell of a
global raster is drawn kilometres above the lift. One margin for the ball and
for the exact test is a necessity: the ball may not reject what the exact
test admits, and the exact test admits exactly the covering radius past its
probes; two measures of one thing would part in silence, by cells lost at
the limb (`the_ball_forgives_the_cell_as_much_as_the_exact_test_does`,
`the_ball_covers_what_is_drawn_and_not_what_was_meant`).

Balls are built when a raster's description settles, for every raster of the
layer (`rebuild_bounds` in `module.rs`), never per frame: they depend on the
binding and the size, not on the view. Every level is probed by its own nine
points (`Overlay::bounds`), not merged from the level below: the exact test
forgives a cell the covering radius of **its** lattice, which doubles from
level to level, and a merged ball would carry the finer margin and be
stricter than the judge it may not contradict. Cells checked and admitted
are counted per level for the performance counters (`Toll::level`,
[diagnostics.md](../operations/diagnostics.md)).

## The level under a pixel

What the globe brings to the choice of level is `Overlay::sharpest`: the
level whose pixel is no larger than a screen pixel —
`⌊log₂(mpp_screen / mpp_raster)⌋`, or level 0 when the screen is finer than
the raster. The screen's metres per pixel are the camera's
(`Camera::metres_per_pixel`): the visible arc at the centre of the view over
the rows of the target. The raster's are the frame's
(`Frame::ground_m_per_px`): the ground span of the raster's width over the
width in pixels — the vector of a projected frame, the longest of three
probed rows of a quad, the widest row of a lattice, measured as
[georeference.md](georeference.md) says; a frame with no width is not
measurable, and such a layer wants nothing. The preview raster is always
wanted first; the detailed one is added over it when the preview's native
resolution is coarser than the screen and the detailed raster is finer than
the preview (`Overlay::wanted`, `detail_eclipsed`). A detailed raster whose
description fails gives its place to the next spare the provider named with
it (`Raster::spares`, `next_spare`): the same raster, a fresh file, described
anew — a night SLSTR granule's visible channel is fill, and the thermal one
behind it carries the data and the binding. The globe knows no file names;
each raster carries the sender's `ordinal`, and the one lying detailed rides
back in `OverlayProgress.detailed`, so the sender can name the file its layer
row is talking about. The words about that raster — its refusal, and the
detail limit when it decides the detail — travel apart from the rest
(`OverlayProgress.detailed_trouble` against `trouble`, `Overlay::said_split`):
the sender puts the name before them and only them, since the preview's limit
or a binding refusal under the detailed file's name would read as said of it.
The variable that raster shows (`Meta.variable`, from `Described.variable`)
rides in `OverlayProgress.detailed_variable` the same way, and the file's
candidates in `detailed_variables`, kept from the last description that named
them: the globe reports, the sender names — and names one back in
`OverlayRaster.variable`. The same resources with another variable are the
same overlay (`Overlay::revariabled`): the raster is described anew with it in
place — its pass released, the tiles of the old fingerprint forgotten unless
another layer holds them, the hatch back over the cell until the new
description lands — and its spares stay. A named variable the tiler refuses does not hand the place to a spare
and does not doom the layer: the refusal is that raster's complaint, the list
stays, and the sender names another. From there on — the
target coarsened to the detail limit and the appetite cap, the ladder, the
store — is the shared mechanics of
[viewing-pipeline.md](viewing-pipeline.md).

## Outlines

An outline arrives as a ring of vertices with a style (`Outline`,
`OutlineStyle`) and is built by `outlines::Outlines::build` into three pairs
of buffers — lines, ribbon, hatch — each drawn by its own pipeline
(`each_style_goes_to_its_own_draw`). The ring is densified once for all three
(`ring`), to `max_edge_deg` at the common lift (the section above): a
selected outline is drawn where it stood before selection, and hatching
cannot show past its own edge. Lines write no depth and sort among
themselves only by draw order (`gpu::render`): hatch first — over the
graticule and over the overlays already shown, because the place a scene is
to take must be seen where the globe is already covered — then the plain
lines, then the ribbon, so the selected outline reads over neighbours that
cover it.

**The selected outline is a ribbon of screen width.** A line in the pipeline
is one pixel wide, and colour alone does not single one out of fifty. The
ribbon is a strip of triangles: every vertex of the ring goes to the GPU
twice, as the left and the right edge, carrying its neighbours along the
ring as offsets — offsets, not places, because neighbours share the coarse
half (`RibbonVertex`, `a_ribbon_carries_true_offsets_to_its_neighbours`) —
and the vertex shader spreads the pair apart on the screen (`vs_ribbon` in
`globe.wgsl`): `RIBBON_PX` wide, the joint along the bisector of the two
links, stretched no more than `MITER_LIMIT`. Width in pixels and not in
metres: a width in the world would blur into a blot from orbit and thin into
the same line up close; the frame size travels to the shader in the camera
uniform for this alone. While a ribbon is drawn the other outlines are drawn
dimmed (`fs_outline_dim` in place of `fs_outline`), so that they do not
compete with it for the eye.

**The area a scene on its way to the globe is to take is hatched.** Such a
scene is outlined by the plain line and filled inside it (`OutlinePending`):
the line says where, the hatching how much — between the click on the icon
and the first picture lie the catalogue, the opening of rasters and their
description over the network, and without this nothing would appear at all.
The fragment shader keeps every other `HATCH_PX` of `x + y` of the
fragment's screen position (`fs_hatch`): a step in pixels, not on the
ground, so that the hatching neither merges into a fill from orbit nor
spreads over half the screen up close, and diagonal so that it does not
coincide with edges near horizontal and vertical. The fill is a fan from the
middle of the ring — the normalised sum of its vertex directions — to its
edge: the outlines of scenes are convex, a quad or a swath, and for a convex
ring the fan from its middle is an exact triangulation. The fan is cut along
the radius with the same `max_edge_deg` as the ring: a triangle from the
middle to the edge is a chord, and its midpoint sags under the lift, where
the depth test eats it (`the_filled_area_follows_the_surface`). A ring whose
farthest vertex lies beyond `FILL_REACH` from the middle — a quarter turn
less a margin — has no middle to speak of, and a fan from any point would
cover the far side of the Earth: such a ring is drawn as an outline alone,
and the outline is what matters
(`a_gapped_band_is_whole_and_still_gets_no_fill`). A ring cut into two loops
(below) is a band and gets no fill either
(`a_ring_around_the_earth_gets_no_fill`); a polar cap, which has a middle,
is filled (`a_polar_cap_is_filled_all_the_same`,
`a_cap_written_to_the_pole_keeps_its_fill`).

**A click on the globe picks the smallest outline covering the place.** The
globe answers a click with a place (`on_probe`, the section on the camera);
which outline that is, the data-browser decides (`pick` in its outline
handler): of the outlined scenes whose rings cover the place
(`footprint::covers`), the one with the smallest angular radius
(`footprint::frame`) — the catalogue gives one acquisition as several
products with nearly one footprint, and a radar swath covers a whole tile,
so only the smallest answers "what did I click". A click past every outline,
or past the Earth, clears the selection. `covers` walks the ring the way the
globe draws it (`geodesy::along`, `traced`): the edge of a swath is an arc,
the edge of a lat/lon box a parallel or a meridian, and arcs between the
vertices alone would pick a box outside its drawn top edge
(`covers_agrees_with_the_drawn_box`). Inside is decided by the turn of the
bearing from the place around the ring, not by crossings along a latitude:
a ring around the pole lies entirely south of a point inside it
(`covers_a_ring_around_the_pole`); on a sphere a closed line has no outside,
so the sign of the turn is compared with that at the ring's own middle; a
band, whose turns cancel everywhere, is recognised by `geodesy::encircles`
and asked by latitude (`covers_a_whole_earth_band`). What the selection means
on the list side is [screen.md](screen.md).

## A band has no sides

A global raster's footprint is written as a rectangle from −180° to 180°: its
left and right sides differ by a full turn of longitude, that is they are one
meridian walked up and down, and drawn as written they are an extra arc
across the band from edge to edge. So an outline is drawn as loops, not as
one polyline (`ring`): the edges that only cross from one edge of the ring
to the other are cut (`seam_cuts`), and each loop between them is closed on
itself (`a_whole_earth_band_keeps_its_parallels`: a band is two parallels,
and no vertex is left on the seam).

An edge of zero longitude sweep (`geodesy::sweep`) is only a candidate — a
zero edge can be a real side. What is asked is the runs between such edges,
and two things of each: whether the run closes on itself — a seam joins what
is one place of the sphere without it — and whether it went round the Earth
(`geodesy::FULL_CIRCLE_DEG`, so that a global grid written half a pixel
short of 360° still counts). A run of zero sweep, a spur walked there and
back, abstains. One run failing either question keeps every side: a band of
four vertices is cut; a grid with a ten-degree gap is not — its sides are
real (`a_band_with_a_gap_keeps_its_edges`); a ring that went round a
parallel and came down is not (`a_run_that_does_not_close_is_no_seam`). The
walk starts after the first cut, so the loops do not depend on which vertex
the supplier began with (`a_band_is_cut_the_same_wherever_its_list_begins`),
and a repeated corner — two cuts in a row — adds no loop
(`a_repeated_corner_does_not_split_the_band`).

The rule is independent of how many vertices write a side, but not of where
they stand: `sweep` unwinds every edge to the nearer branch, so the sum of a
run is kept only while no edge is longer than a half turn. A band written in
quarters is cut (`a_band_is_cut_the_same_however_densely_it_is_written`);
one written in halves is not, and is drawn as written. A loop shrunk to a
point — the top parallel of a rectangle up to the pole — is cut lawfully and
drawn by nothing (`a_loop_shrunk_to_a_point_draws_nothing`).

## The camera and its floor

The camera (`camera.rs`) is a frame above the surface — a unit normal `at`
under it, an `up` for the top of the screen, a height over the ellipsoid —
looking at the ellipsoid's centre with a vertical field of view `FOV_Y_DEG`.
Height by itself means nothing: gestures and focusing speak in arc, how much
of the Earth the frame holds, computed by `Camera::visible_deg` by the sine
rule against `drawn_radius` and inverted by `height_for`
(`height_inverts_visible_arc`). A drag across the whole height of the view
turns the Earth by exactly the arc the view holds (`Camera::orbit`,
`a_drag_moves_the_arc_it_promises`), so the ground follows the cursor at any
height. `Focus` names a place and the angular radius that must fit, and the
camera turns the radius into a height by `height_for` with `FRAME_MARGIN`
around it — an outline touching the edge of the frame reads as cut off — and
never higher than where the limb reaches the edge of the frame, past which
the Earth only gets smaller (`focusing_on_the_whole_earth_fills_the_frame`).
The clipping planes are derived from the height too
(`Camera::view_projection`): the near plane at half the distance to the
drawn surface, the far plane past the far edge of the ellipsoid.

The floor, `camera::HEIGHT_RANGE_M`, is ten lifts, `SURFACE_LIFT_M × 10`:
derived from the lift and not set by hand, because coming down to the lift
means passing through what is drawn; ten is a choice, not a derived value —
nothing breaks at five, the lift merely becomes a fifth of the height. From
height `h` the frame holds `2·(h − SURFACE_LIFT_M)·tan(FOV_Y_DEG/2)` metres
of surface vertically (`near_the_floor_the_frame_is_plain_trigonometry`);
with these constants the floor gives about seven tenths of a metre per pixel
on a thousand-row window — finer than any scene that arrives, and enough to
trace a field boundary by hand. The lift distorts nothing: it is an offset,
not a magnification, and every measure of the camera over the visible is
taken to `drawn_radius`. Nor does the precision of coordinates bound the
floor — with positions in halves it comes from the distance to the eye.

The place under a point of the frame (`Camera::probe`, answered by
`on_probe` in `module.rs`) is where the ray of that point meets the surface
raised by `SURFACE_LIFT_M` (`geodesy::intersect_at`), read back as latitude
and longitude (`geodesy::surface_at`): what is seen under the cursor is what
is drawn, and it is drawn on the lift. A ray meeting the ellipsoid would
name a place away from the seen one, the further the closer to the edge of
the frame (`a_probe_names_the_place_that_is_drawn_there`). The intersection
is computed on a sphere by stretching the polar axis by `1/(1−FLATTENING)`;
"past the Earth" is an answer like any point, and `Probed.at` is empty then.

## Zooming at what is looked at

The place under the cursor stays under the cursor (`Camera::zoom_at`). A
wheel notch arrives with the point of the frame to hold — an offset from the
middle in fractions of the view's **height** on both axes, the measure the
drag is made in (`hold` in the data-browser's globe handler,
`a_pointer_holds_the_place_the_probe_names`) — and multiplies the height by
`ZOOM_PER_STEP` per notch: the canvas's wheel step inverted, from the one
`wheel.rs` both include, because one wheel has one notch. The held place is
taken before the height changes, the way a click's probe takes it — on the
lifted surface, in `f64` — and the frame is swung by the difference of two
arcs, from the point under the camera to the held place at the old height
and at the new (`Camera::arc_at`).

That swing is a first pass, and the second is no luxury: the arc is
spherical while it turns a geodetic normal, and the height is measured along
the normal while the frame looks at the centre; together the two errors move
the view by a pixel per notch, compounding, because every next notch
magnifies what has already slid. So the held place is asked again where it
actually is in the frame (`seen_at`) and the frame is turned by the
residual, twice (`zoom_keeps_the_place_under_the_cursor`: under a hundredth
of a pixel on a 1300-row frame; `a_zoom_holds_whatever_it_took_hold_of`:
under five thousandths across the height range, the frame and three
latitudes). The residual is split along the radius of the frame and across
it, because a degree of angle is worth a different arc in the two
directions: along, the slope of arc over angle (`slope_at`); across, the
tangent of the arc over the angle, since the turn goes about an axis in the
plane of the arc and the place walks a circle around it. The same second
pass removes the sideways drift of a zoom into the middle of the frame, where
there is nothing to hold.

At the limb the hold weakens — not for the arithmetic, but because there is
nothing to hold: the surface slides from under the ray and one pixel covers
kilometres. The measure is the cosine of the angle the ray meets the ground
at (`Camera::grazing`) — how many times more ground a pixel covers than at
nadir, inverted; the grip is that cosine times `GRAZE_LIMIT`, capped at one,
and the held point is pulled towards the middle by the grip
(`the_hold_lets_go_at_the_limb`: the turn per notch grows away from the
middle and falls off towards the limb, while what is still held is held
exactly). Twice is a choice, made by the width of the band where the hold
weakens: a steeper limit narrows it to a few pixels of cursor, and the limb
jerks the view as before. The grip is asked at both heights, the current and
the one zoomed to, and the smaller is taken: zooming out shrinks the disk,
and a place that was held ends up beyond the limb. The pair is symmetric,
and that is the point: a notch back has the same two heights, so the same
grip, so it undoes a notch in (`a_notch_back_undoes_a_notch_in`); a grip
from one height would not. A notch stretched by scroll inertia into a
hundred parts lands where a whole one lands
(`a_notch_arrives_whole_or_in_a_hundred_parts`), and a stream of parts to
and fro comes back exactly (`alternating_parts_bring_the_camera_back`). A
notch that hits the floor or the ceiling of `HEIGHT_RANGE_M` is no zoom and
turns nothing (`zoom_at_the_edges_only_stops_moving`); a cursor past the
Earth holds the middle of the frame (`zoom_past_the_limb_holds_the_middle`).

## A frame, not a pair of angles

The camera stores two unit vectors — `at`, the normal under it, and `up`, the
top of the screen — and a height; latitude and longitude are derived from
`at` when asked (`geodesy::angles`), never the other way round. A gesture is
therefore one rotation about an axis composed of the frame's own axes
(`Camera::orbit`: the up vector for a horizontal drag, the side vector for a
vertical one, both for an oblique), scaled by the visible arc. Three things
follow. The arc is exactly the arc asked for at any latitude, with no
1/cos(latitude) to apply (`a_drag_moves_the_arc_it_promises`). The pole is
crossed: the camera goes over the top and comes out on the far meridian
instead of stopping at a guard short of it
(`a_drag_carries_the_camera_over_the_pole`). And the basis never degenerates:
`up` is stored, not derived every frame from the world axis, which over the
pole coincides with the line of sight; `settle` re-orthonormalises the pair
after every turn (`orbit_keeps_the_frame_orthonormal`).

The one price is honest: past the pole the frame is north-down, as from an
aeroplane window. The gesture does not turn it back — that would spin the
world half a turn before the eyes; north comes back by the same drag back or
by focusing on any scene: `Focus` puts north up (`frame_up`, `north_up`),
and over the pole itself, where north is undefined, keeps the previous top
(`focus_reaches_the_pole`). A gesture and its exact reverse cancel, an
oblique one too, because the rotation is one
(`the_gesture_is_a_rotation_and_undoes_itself`,
`an_oblique_gesture_undoes_itself`); split into yaw and pitch they would
cancel only in the limit of small steps. A cursor led round a circle brings
the view back rotated by the area enclosed — a property of the sphere, which
no choice of axis removes.

## Focusing flies

`Focus` names the target; the frame tick drives the camera to it
(`Camera::focus`, `Camera::advance` from `on_ui_event`) in `FLIGHT_S` — one
duration for any distance, because a flight explains where the view went and
does not measure the way — easing in and out. A jump across half the globe
does not read as a movement: the view is simply another one. The orientation
is interpolated as a quaternion between the two ends (`Flight`, `slerp`) —
the only place in the tree where quaternions are kept, and kept because here
they beat the frame: leading a pair of unit vectors component-wise is
walking a chord and straightening it by Gram–Schmidt, which is not the path
pretending to be it. The height is led in ratios, not metres: zooming is
multiplication, and a linear flight would spend itself in the first frame and
crawl the last kilometres (`a_flight_passes_between_its_ends`: the midpoint
is the geometric mean). The flight lands on exactly the camera asked for,
not on what rounding left (`a_flight_lands_exactly_where_asked`); a repeated
focus on the same target does not restart it
(`a_repeated_focus_does_not_restart_the_flight`); any gesture calls it off
(`a_gesture_calls_off_the_flight`) — the camera is not pulled two ways at
once. While the camera flies, the overlay tiles are asked again on every
frame, as on every gesture: the level and the visible cells are computed
from the camera, and what was ordered before the flight describes the place
it left.
