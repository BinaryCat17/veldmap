# Georeference

How a raster is placed on the Earth: what a binding is, who supplies each
kind, which one wins, what each format is read for, and who interprets a
coordinate system. Terms are in [the glossary](../glossary.md); what the
application does not do with a binding is on
[the limitations page](../limitations.md).

## Four kinds of binding, in rank order

The globe (`veldmodules/globe/src/overlay.rs`, `enum Binding`) knows four
kinds, and a later one of a **higher** kind replaces an earlier one; a later
one of a lower kind is ignored. Rank, not arrival order: an overlay has two
rasters, the quicklook and the detailed one, described independently, and
which describes first is the timing of two file parses — nothing meaningful.

| Kind | What it is | Who supplies it |
|---|---|---|
| Catalogue | the footprint of the product: which piece of the Earth was imaged, not which pixel goes where; its vertex order owes nothing to the raster's walk | the catalogue, through `data-provider` and `data-browser` (the quad of the imagery response) |
| Named | the UTM frame derived from the product's name (an MGRS tile): exact, but promised by the name, not said by the file | `data-provider` (`veldmodules/data-provider/src/mgrs.rs`, used by the imagery response in `cdse.rs`) |
| Projected | the projection written in the raster itself: a code and six affine numbers, linear by definition | `image-tiler` (`Placement`): GeoTIFF geokeys (`adapters/tiff.rs`), the GMLJP2 box of a JPEG 2000 (`adapters/jp2.rs`) |
| Lattice | a grid of tie points from the raster, denser than four nodes: it describes a **non-linear** layout — a radar scene lies in the geometry of its acquisition — which no six numbers can | `image-tiler` (`ties`): GeoTIFF tie points, the coordinates of a NetCDF from the file itself or from the coordinate file beside it (`adapters/netcdf.rs`) |

A lattice of exactly four nodes describes nothing non-linear — between four
corners the same linear layout is interpolated — and does not outrank a
projection (`Grid::is_dense`). Everything that leaves the tiler is in WGS84
degrees or in the raster's own projection; the half pixel of
`RasterPixelIsPoint` is removed at the source, so a node and a corner mean
the same thing to the consumer. The rank also decides what a repeated overlay
message may change: the same key with the same resource ids (the coordinate
file included) keeps the overlay and its binding, and only opacity and
hiddenness are taken from the message — its footprint is discarded, because
the layer may already lie by its raster and the catalogue is junior
(`adopt_overlay` in `veldmodules/globe/src/module.rs`).

## What a GeoTIFF is read for

`georef` in `veldmodules/image-tiler/src/adapters/tiff.rs` reads four tags:
`GeoKeyDirectoryTag`, `ModelTiepointTag`, `ModelPixelScaleTag` and
`ModelTransformationTag` — the last as `Tag::Unknown(34264)`, because the
`tiff` crate has no name for it. `GdalNodata` is read by the same adapter and
is radiometry, not a binding ([imagery.md](imagery.md)). `GTModelTypeGeoKey`
decides the kind: 2 is degrees and goes to `geo_ties`, 1 is metres of a
projection and goes to `geo_placement`; anything else — geocentric,
user-defined — yields nothing.

In degrees, more than one tie point is a lattice: a node is six numbers, the
pixel `(i, j, k)` and the place `(x, y, z)`, and every node must be a place on
the Earth (`placed`) and finite, or the whole lattice is dropped. One tie with
a usable scale (`usable_step`: both steps finite and non-zero — a zero step
would fold the raster into a line and nothing downstream could tell) gives
the four corners of an axis-aligned rectangle; otherwise the matrix gives them
(`corners_from_matrix`), and a degenerate matrix — zero determinant, a
non-finite entry (`affine_from_matrix`) — gives none.

In metres, `ProjectedCSTypeGeoKey` must be a code, neither 0 nor 32767
(user-defined: its parameters lie in other keys, and assembling a system from
them is projection work, not reading), and becomes `Placement.epsg` as
written; the affine comes from tie and scale or from the matrix, and the far
corner of the raster must be finite too. A lattice in a projection — more
than one tie — is a refusal, not a placement from the first node: a lattice
describes a non-linear layout, and taken from one node it would lie more the
farther from it.

`RasterPixelIsPoint` (`GTRasterTypeGeoKey` = 2) is a half pixel, removed once
at the source — into the node, or into the free term of the affine — so one
convention leaves the tiler (`half_pixel`). A datum other than WGS84 in
`GeographicTypeGeoKey` (`foreign_datum`; 4326, 4979 and 4055 count as WGS84,
silence and user-defined as no answer) is a `warn` line in the log and
nothing else: the binding is taken, the numbers stay as written, and the
raster lands on WGS84 about a hundred metres off — see the limitations page.
A Sentinel-1 granule carries a lattice of 21×21 nodes, the number `TIE_GRID`
below is set to.

## A NetCDF binding from the file itself

`ties` in `veldmodules/image-tiler/src/adapters/netcdf.rs` builds the lattice
of the shown variable by two CF paths; the same two predicates rank a
variable as placeable when the variable to show is chosen (`placeable`,
[imagery.md](imagery.md)).

- **Per-sample coordinates** (`swath_pair`): the planes the variable names in
  `coordinates`, of the same shape; if none is named, the one plane of the
  same shape and the same group whose units start with `degrees_north`
  (`northing`; `degree_north` too) and the one in `degrees_east` (`easting`).
  Uniqueness is the rule: two latitudes of one shape are the question
  "which", and the file has no answer — an SLSTR granule holds `latitude_in`
  and `latitude_tx`, but of different shapes, so they do not collide. Reading
  is refused when the pair does not fit `TIES_BUDGET` (64 MiB), measured as
  the peak of reading two grids — `ties_peak`: the settled first plus the raw
  and unpacked copies of the second — the same sum the coordinate file is
  measured by, so one pair cannot pass in one place and fail in the other.
- **Grid axes** (`grid_axes`): one-dimensional variables in the variable's
  group or in the root whose length equals the raster's rows (`northing`) or
  columns (`easting`).

Either gives the lattice through `lattice` with `Seating::SAME` — node and
sample are the same thing, the node at the centre of its sample (+0.5). A
raster under 2×2 has no lattice and no complaint. Coordinates are
**unpacked** on the way (`unpacked`: `value · scale_factor + add_offset`
from `Item.packing`; Sentinel-3 writes its latitudes as packed integers), the
grid axes likewise; the shown variable is not — its stretch and its nodata
keying want the raw values ([imagery.md](imagery.md)).

## Nodes by the length of the side

The lattice does not take every sample. `count` gives the nodes on a side as
`((side − 1) / NODE_STEP + 1).clamp(TIE_GRID, TIE_CAP).min(side)`: one node
per `NODE_STEP` = 64 samples, no fewer than `TIE_GRID` = 21, no more than
`TIE_CAP` = 256, and never more than the samples themselves (a node standing
twice on one sample would break the consumer's `Grid::new`, where the axes
are built from distinct fractions). Per side, not one number for both: a
granule is long along the orbit, and one constant would mean tens of samples
between nodes across track and hundreds along it. The floor is for short
sides — a side of a few hundred samples would otherwise get corners, not a
lattice; the cap, because the nodes travel in the description and lie in the
consumer's memory. `the_nodes_are_counted_by_the_side` holds the real cases:
an OLCI tie grid of 15076×77 gives 236×21, a SYNERGY AOD raster of 4022×324
gives 63×21. What fixed `NODE_STEP` — the measured median error of linear
interpolation on the OLCI grid against a 21-node lattice and against a step
of 32 — is recorded in its comment, not held by a test. `nodes` spreads the
indices evenly over the span, rounded; a span of one sample gives no nodes.

## Stepping back from a ragged edge

Coordinates are not written beyond the swath, and the edge is ragged. The
consumer needs a full rectangle (`Grid::new`), so a lattice cannot lose a
node — it steps back from the edge instead (`footing`), staying rectangular
and therefore denser. The step is chosen in two moves, in this order: any
side whose retreat reduces the count of unsound nodes — from a clean side
too, because the nodes are re-laid over the new span and a shift of one
sample takes the whole axis off a defect (a tie goes to the dirtier side);
when no side reduces it, the dirtiest side — a march through a solid band of
bad samples, where no single step improves anything. The budget is the
**finest** of the two lattice steps, per side, four sides at most: past the
lattice's own step nothing is distinguished anyway — beyond its edge the
coordinates continue linearly (`cell` in `overlay.rs`) — and the finest of
the two, not each axis's own, because along the orbit the step is hundreds of
samples and a retreat by that measure would throw away hundreds of kilometres
of track. `None` — the budget spent, or a hole not at the edge — means no
lattice: the layer falls to the catalogue footprint, and with a footprint more
complex than a quad ends in an error.

The rule is greedy by choice: the march looks at dirt, not at what remains,
and a step of it may lose more than it gains; on the edge holes it was
written for it wins, on a sparse scatter it may lose, and the comment on
`footing` records the SYNERGY AOD mask where the other choice throws away
rows and columns of good data. `a_ragged_edge_is_retreated_from` holds the
mask of a real AOD granule (4022×324): rows 1…4021 and columns 11…320
remain. `the_budget_is_the_finest_step_of_the_two`,
`the_march_goes_by_the_side_that_is_dirty` and
`a_hole_in_the_middle_is_not_retreated_from` hold the rest.

## The coordinate file beside the raster

Sentinel-3 keeps the measurement in one `.nc` and latitude with longitude in
another file of the same product. Which file it is, the provider knows — the
layout of a product is its business (`geolocation` in
`veldmodules/data-provider/src/imagery.rs`): only a sibling in the raster's
own folder, in this order — `geodetic_tx.nc` when the raster's name ends in a
grid tag (`grid_tag`: two letters, the grid from `a b c i f t`, the view
from `n o x`), then `geodetic_<tag>.nc`, then `tie_geo_coordinates.nc`,
`geo_coordinates.nc` and `geolocation.nc` (SYNERGY). The tie grid `tx` goes
first because it is far cheaper than the per-sample file; its nodes stand on
the instrument's nominal grid, and the provider's comment records the measured
offset from the per-sample coordinates — under a pixel of a kilometre
raster, and the floor of any binding taken from that file.

The name rides as `ImageryRaster.geolocation`; `data-browser` opens it as a
resource, the globe owns it together with the raster
(`OverlayRaster.geolocation` in `veldmodules/globe/types.proto`, released
with it) and passes it as `DescribeRequest.geolocation`. The tiler asks it
**only when the raster itself carries neither ties nor a placement**
(`describe` in `veldmodules/image-tiler/src/module.rs`) and does not put it
in the memo: it is a property of the pair, not of the raster. A failure does
not fail the description — the raster is left without a binding, and the
reason travels in `binding_trouble` prefixed "файл координат".

`netcdf::geolocation` takes the densest pair of `northing`/`easting` planes
of one shape, unique per shape (the file holds altitude beside latitude, and
SLSTR's the coordinates of neighbouring grids); refuses a grid denser than
the raster — those are someone else's coordinates — one under 2×2, and one
over `TIES_BUDGET`; unpacks; then seats the grid on the raster.

**Seating** (`seating`). The grid may be sparser than the raster (OLCI's tie
grid) and, for SLSTR's `tx`, wider than it on both sides; how it relates to
the raster is asked of the files, in an order from what is read to what is
inferred, and the order is mandatory:

1. The instrument frame named by **both** files (`Frame`, from the global
   attributes: `track_offset` → `across_at`, `start_offset` → `along_at`,
   `resolution` as a string `[ 16000 1000 ]` → metres across and along
   track, each number parsed whole by `resolution`, not digit by digit). The
   step is the ratio of resolutions, the origin the raster's offset minus the
   grid's at that step — with opposite signs on the two axes, because
   `start_offset` counts from
   the start of the orbit, not of the file. `covers` checks that the seated
   grid reaches the raster within one cell: an attribute can be about the
   product rather than this pair (OLCI's `resolution` is the same in both
   files), and this is where it shows.
2. A grid the size of the raster sits sample on sample (`Seating::SAME`) —
   an inference, hence second.
3. `ac_subsampling_factor` and `al_subsampling_factor` (`subsampling`, both
   required) last, and they must match the sizes within a pixel: they
   describe the product's tie grid, not the file they are written in —
   OLCI's per-sample `geo_coordinates.nc` carries them too.
4. Nothing said — a refusal in words, and the scene lies by the catalogue
   footprint. A seating inferred from sizes alone is silent exactly where it
   is wrong; the comment on `seating` gives the SLSTR `tx` case, stretched
   across track.

`the_olci_tie_grid_sits_by_its_subsampling`,
`a_grid_the_size_of_the_raster_ignores_the_subsampling_attribute` and
`a_subsampling_that_misses_the_sizes_is_refused` hold the order.

## Who interprets a coordinate system

The tiler does not: `Placement` is the EPSG code as written in the file plus
the affine transform, and "zone 38 north" is already an interpretation. The
one thing the tiler asks of a code is the axis order of a GMLJP2 grid
(`easting_first` in `adapters/jp2.rs`, a list of codes whose first axis is
easting), because the grid's two offset vectors carry no axis names. The
globe interprets, because the projection mathematics exists once in the tree
and a second copy would agree by eye and disagree in numbers.

`Frame::from_placement` (`veldmodules/globe/src/overlay.rs`) calls
`Projection::from_epsg` (`veldmodules/globe/src/projection.rs`): 3857 is
`WebMercator`, computed on a sphere of radius `WEB_MERCATOR_RADIUS_M` (the
semi-major axis); every other code goes to `System::from_epsg` and becomes
`Transverse(System)`. A code the globe does not know is an `Err` naming the
code — the raster lies by its catalogue footprint, not by a guess, and the
reason reaches the layer. The six numbers are rescaled from pixels to
fractions of the raster (`placed`): the overlay's two rasters differ in size
and lie alike. A frame without extent — a `NaN` in the six reaches here too —
is an `Err` as well (`measurable`), because a zero width would break the
choice of level.

**A system is parameters, not a name.** UTM, six-degree Gauss-Krüger zones
and local systems such as MSK-61 are one transverse cylindrical projection
with different numbers (`System`): the datum, the central meridian, the scale
on it, the false easting and northing. `System::utm` is WGS84, meridian
`6n − 183`, scale 0.9996, `UTM_FALSE_EASTING` (500 km) and, in the south,
`UTM_FALSE_NORTHING_SOUTH` (10 000 km). `System::gauss_kruger` is Pulkovo
1942, meridian `6n − 3`, scale exactly one, false easting
`zone · 1 000 000 + 500 000` — the zone number is written into the
coordinate. `System::msk61` is three zones on Pulkovo 1942 with meridians
37°59′, 40°59′ and 43°59′ (the definition of the system, not a typo), scale
one, false easting `zone · 1 000 000 + 300 000` and false northing
−4 811 057.628 m. `from_epsg` checks band bounds instead of dividing:
32601–32660 north, 32701–32760 south, 28401–28432 Gauss-Krüger,
6336101–6336103 MSK-61; 32600 and 32700 are not zones, 32661 and 32761 are
polar stereographic and taken for "zone 61" would put a scene thousands of
kilometres away (`epsg_names_a_system_only_inside_its_bands`). The series
are Krüger's in the third flattening to the fourth power, good within a
zone; forward and inverse are a pair with a convergence test across the zone,
and only the inverse is called by working code — the warp grid goes from
metres to degrees.

**MSK-61** (the local system of Rostov oblast, zones 1–3) exists as a branch
of `System::from_epsg` verified by a test, not by a raster: the codes 63361xx
are not EPSG's own, a GeoTIFF geokey is sixteen bits wide and cannot carry a
seven-digit number, and the local system is usually written nowhere in a
file. It waits for whoever can name it with a number.

## The datum

Datums are translated inside `projection.rs` and only WGS84 leaves it: a
latitude on Krasovsky's ellipsoid looks like one on WGS84 and differs by a
hundred metres in silence, and outside the module there is no honest place
to do it. `Datum` is an ellipsoid plus seven Helmert elements — a shift of
the origin, three rotations, a scale correction; `WGS84_DATUM` has them all
zero, `PULKOVO_1942` stands on `KRASOVSKY` (a semi-major axis 108 m longer
than WGS84's, by the two constants). Almost all of the difference between
the systems is the shift of the centre, not the shape (the doc of `Datum`),
which is why swapping ellipsoids would not do. The seven parameters of
Pulkovo 1942 are those of GOST R 51794-2008, because the local systems the
cadastre is kept in are described with them, and the converters checked
against on the ground use them. The forward is `helmert`, the inverse
`helmert_back` in closed form — not a sign flip, which differs from the true
inverse by the product of rotation and shift and would hide a real error
inside its own tolerance. `to_wgs84`/`from_wgs84` short-circuit on the whole
`WGS84_DATUM`, not on the ellipsoid
(`a_shifted_datum_on_the_same_ellipsoid_still_moves`).

What the tests hold: `the_datum_shift_is_worth_a_hundred_metres` — a point
in Rostov oblast moves between 100 and 130 m;
`the_helmert_and_its_inverse_are_exact` — under 1e-6 m in geocentric
coordinates, for a datum with three non-zero rotations too;
`the_datum_shift_and_its_reverse_converge_where_it_is_fitted` — under 1 mm
in degrees at four points across the country: the pair is exact only in
three dimensions, a raster binding carries two, and the dropped height
leaves a residual proportional to how far the ellipsoids are apart in height
at that point; `the_local_grid_matches_an_outside_converter` — four MSK-61
points in zones 1–3 land where an outside converter (geobridge) puts them,
under 2 mm, all inside their zones. Separately, the ellipsoid, the series,
the zone offsets and the seven elements are unverifiable; agreeing, they
verify one another.

## A projected frame on the warp grid

`Frame::Projected { system, affine }` goes from fractions of the raster to
metres by the six numbers and to degrees by `to_geodetic`, on **every node of
the warp grid** (`nodes_of` in `overlay.rs`), not between corners: there is
no interpolation error at all. Between corners it would not close — a line
of constant northing is not a line of constant latitude, and whoever walks
between corners cuts that bow; the doc of `Frame::Projected` gives its size
on a Landsat scene. `the_edges_and_the_affine_name_one_frame` and
`a_placement_in_pixels_becomes_a_frame_in_fractions` hold that the frame at
any fraction equals the inverse projection of the affine point. Six numbers,
not an origin and a step: the same field carries a rotated raster
(`ModelTransformation`), an axis-aligned one is the case with zeros off the
diagonal, and the MGRS frame builds the same affine (`Frame::utm`, `y1` the
northern edge). Longitude is not unwound here: the inverse projection gives
it as the central meridian plus an offset, already one branch. A transverse
frame is never axial — latitude varies along a row — so the warp grid does
its two trigonometries per node; an axial frame (Web Mercator without
rotation, a quad of parallels and meridians, an axial lattice) does them per
row and per column (`Frame::axial`,
`only_a_frame_of_parallels_and_meridians_is_axial`).

## A lattice on the globe

`Grid::new` groups the ties into axis lines with the `SAME_LINE` tolerance
(fractions arrive by dividing a pixel by a side and may differ in the last
digits), requires a full rectangle, unwinds the longitudes, decides
axiality after unwinding, and computes `widest_row_m` once at build, since
`Frame::ground_m_per_px` asks for it on every frame. A lattice whose widest
row or column is zero on the Earth is refused: a nadir instrument with no
across-track extent lays its nodes on a line, and its quads would have no
area. Between nodes the show interpolates linearly (`lerp`) — nodes stand
tens of kilometres apart, where an arc and a chord differ by metres; beyond
the edge the cells continue linearly (`cell`). `is_dense` is more than four
nodes.

Longitudes are unwound **along the lattice**, not to the first node
(`Grid::unwind`): a global raster's grid spans 360°, and unwinding to one
vertex would fold it into a meridian. Within a row each node is unwound to
its left neighbour (`geodesy::unwind`: exactly a half turn stays where it
is; the turns to remove are computed in one step, so an infinite longitude
does not hang the module — `an_endless_longitude_does_not_hang_the_unwinding`).
A difference of a whole circle — `FULL_CIRCLE_DEG` = 359°, because a global
grid's edges fall half a pixel short of 360 — is left as it is **only in a
row of two nodes** (`whole_circle`): that is how a global raster is written,
−180 → 180, and folded to the near branch it would have zero width. A row of
three nodes and more does without the gate and would be hurt by it: the chain
of a polar row crawls past 180°, and the tolerance would tear it by 359°.
Then the whole row is moved to the previous one by a shift **voted by all
columns**, in turns of `TURN_DEG`: near the pole the first column is the
polar edge and runs a whole circle within a few dozen rows while the far
edge stays; whole turns, because the mean of two disagreeing votes would be half a
circle; a tie goes to the smallest by modulus, equal moduli to the negative
one — so the answer does not depend on where the file's zero meridian is.
`the_vote_is_counted_over_all_columns_at_once`,
`an_even_vote_still_shifts_by_whole_turns`,
`unwinding_does_not_care_where_the_zero_meridian_is` and
`a_grid_around_the_whole_earth_does_not_fold` hold this.

## A footprint more complex than a quad

`quad_of` in `veldmodules/data-browser/src/handlers/overlay.rs` takes one
ring of exactly four vertices (the closing vertex dropped) whose longitudes
span less than `WHOLE_EARTH_DEG` (350°) as the corners, `rough: false`.
Anything else — a swath of dozens of vertices, a scene the antimeridian cuts
into two rings, a rectangle around the whole Earth whose four corners are one
meridian — is the box of the circle inscribed in the footprint
(`footprint::frame`), `rough: true`: a place, not a binding. The globe builds
`Frame::Rough` for it (`adopt_overlay`), rank `Catalogue`, and
`binding_pending` decides what waits: Rough always; Quad while any raster's
description is pending — drawn by the guess, the scene would turn a second
later, and tiles asked by it would be the wrong cells, since visibility is
computed by the same binding; Projected and Grid, and so the UTM frame,
never. While pending, `wanted` asks for nothing and nothing is drawn. When
the descriptions are over (`on_described` in `veldmodules/globe/src/module.rs`):
a layer still on a Quad gets a log line only — a quicklook or a picture
without geokeys lying by the footprint is the normal outcome; a layer none of
whose rasters described gets the collected reasons as its error; a frame
without extent, an error; a Rough frame, the error "привязки нет" with
`binding_trouble` in brackets (`said_with`). Refusing earlier, at the
footprint, would be too early: the binding lives in the raster more often
than in the catalogue — a Sentinel-5P granule carries per-sample
coordinates — and the raster can be asked only by opening it.

## Edges

Between two corners there is nothing but the surface, and an edge is led the
way it is named (`along` in `veldmodules/globe/src/geodesy.rs`): equal
latitudes of the ends — a parallel; equal longitudes — a meridian; anything
else — the shortest arc (`between`, interpolating normals; every kind of
edge lands on its own corners, `every_kind_of_edge_lands_on_its_ends`).
`edge_point` gives the point at a fraction,
`edge_span` the length to densify by (a parallel by its sweep times the
cosine of its latitude, a meridian by the latitude difference, an arc by
`separation`), `sweep` the run in longitude — a full circle, at least
`FULL_CIRCLE_DEG`, stays a circle, everything else goes to the near branch,
since a 20° box across the seam is written 170 → −170. One rule for everyone
who walks between corners: the quad and rough frames (`Frame::geodetic`: the
top and bottom edges, then the vertical between the two computed ends, its
kind asked of those ends and not of the side edges), the outline
(`outlines::edge` in `veldmodules/globe/src/outlines.rs`, densified by
`edge_span` against `max_edge_deg`, capped at the steps of a full turn) and
the click test (`traced` in `veldmodules/data-browser/src/footprint.rs`). The
lattice goes its own way (`lerp`) — its nodes stand tens of kilometres apart.

Both ways are right for their own edge. A straight line in degrees is a rhumb
line, and a satellite leads the edge of a swath by the great circle:
`between_follows_the_arc_not_the_degrees` holds 12–14 km between the arc
midpoint and the degree midpoint of the top edge of a Sentinel-1 EW GRDM
granule (421 km at 73°N; `an_edge_is_measured_by_the_way_it_goes` holds its
3.79° of arc), and the gap grows as the square of the edge. A cell of a
geographic grid — Copernicus DEM, anything written in degrees — runs exactly
along parallels, and led by an arc it would move by the same kind of amount
the other way: `a_quad_leads_its_edges_the_way_the_outline_does` holds
100–110 m on a one-degree cell at 60°. What decides is the edge itself: a
cell's end latitudes are equal, a swath's are not.

The price is one and named: the kind is decided by exact equality, and the
catalogue rounds its coordinates. Two corners of a swath agreeing by accident
lay it as a parallel instead of an arc — off by kilometres, but off together
with its outline and its click, which read the same rule. The match is not
identical: the outline ring is densified more coarsely than the warp grid,
and its links bow by themselves between vertices (the comment of that test
records the remainder). This is also why an overlay on a quad waits for the
descriptions before drawing (above).

## When a binding cannot be read

A GeoTIFF or a NetCDF that speaks of a binding the tiler could not take
answers with `binding_trouble`, and the tiler puts the reason in the log at
the same place. A GeoTIFF says one sentence (`binding_trouble` in `tiff.rs`)
whenever the file carries any of the three binding tags and neither ties nor
a placement were taken: the coordinate model, the system code and the number
of tie points, as numbers — a geocentric model, a system 0 or user-defined, a
lattice in a projection, a degenerate matrix, a tie that is not a place or
not finite all end here. A foreign datum does **not**: it is about a binding
that was taken, and in this field it would tell the viewer "place unknown" of
a scene lying in its place. A NetCDF words its reasons separately (`ties`):
per-sample coordinates over `TIES_BUDGET`, coordinates that could not be
read, nodes that make no lattice (`nodes_unfit`, one text for the raster's
own grid and for the coordinate file), or no coordinates at all for the named
variable — and its coordinate file adds its own, prefixed "файл координат".
When nothing was placed, both halves travel, each naming its file; when
something was, none does. A JPEG 2000 does not complain: a GML grid it cannot
use is silence, and the raster falls back to the catalogue footprint without
a word.

The globe adds its own reasons (`describe_settled`): ties that make no
lattice, a system it cannot translate, a frame without extent — with the
code in the words. The consumer shows the reason next to the layer, because
by an empty binding it cannot tell "the file says nothing" from "the file
said something we could not read", and a scene that falls back to the
catalogue footprint would look real. `Overlay::complain` keeps a reason only
while the layer lies below `Projected`; reasons from two files accumulate
and are not repeated; a later success of the other raster of the overlay
clears them (`Overlay::relay`). For a rough frame the reason becomes part of
the layer's error.
