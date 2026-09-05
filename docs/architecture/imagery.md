# Imagery: from a catalogue product to tiles on screen

How a raster gets from a file or a remote object to the tiles that the canvas
(`image-view`) and the globe (`globe`) draw. Terms are in
[the glossary](../glossary.md); where a raster lies on the Earth is
[georeference](georeference.md). This page describes the tree as it is; what
each format reads is a fact of the adapter, not a promise.

## Product, scene, part

The catalogue counts products; a person counts scenes. One Sentinel-1
acquisition is listed as several products (raw, GRD as a stripped TIFF, the
same as a COG, SLC, OCN); one Sentinel-5P granule is one product per gas over
the same strip. `data-provider` folds products into a **scene** with **parts**
(`veldmodules/data-provider/src/scene.rs`; `DataProduct.parts` is filled only
when there is more than one, and `Part.shown` marks exactly one). What links
products is what the catalogue itself said (`scene::acquisition`, in this
order): datatake with slice number — the one thing every Sentinel-1 product
of a slice is named alike by; the second is not, the raw and the processed
slice start apart; datatake, or the grid tile, with the second (Sentinel-2:
one cell, different passes); orbit with instrument and second (Sentinel-5P:
the gases of one granule; the instrument because Sentinel-3 carries two, and
their products of one second are two scenes). None of these — the product
stands alone, with one narrow exception (`by_name`): names that differ only
by a packaging tail (`PACKAGING_SUFFIXES`; `stem` strips it case-insensitively,
before or in place of the container extension) are one quantity in two
formats.

**What the name says.** The catalogue names `datatakeID` for a part of the
archive only; a Sentinel-1 name carries it always, a hexadecimal field after
the pair of acquisition times and the orbit number, and `datatake_in_name`
reads it there — found from the pair of times, not by counting fields
(`WV_SLC__1SSV` has an empty field, a COG a `_COG` tail), and from Sentinel-1
names only: COP-DEM has zeros in that place, and every cell of one date would
fold into one row. The grid cell of a Sentinel-1 RTC name (`tile_in_name`,
`N03W004`) names the scene as `tileId` does but goes into no catalogue query
(`Facts.named_tile`): the catalogue does not know it.

**A part without a slice number** (`IW_OCN__2S`) lies with the slice of its
datatake that starts within `BESIDE_A_SLICE_S` of it (`beside_a_slice`): the
window covers the spread of starts inside one slice and does not reach the
next. A datatake in which no part has a number is cut into slices by the same
window, in order of time (`slices`).

**Which part is shown** is a rule, not a list of missions (`scene::rank`):
raw level 0 last, a product without a declared level before it, then by
level, tiled layout before stripped (`tiled`: the product type or the name
ends in `-COG` or `_COG`, case aside), otherwise the smaller. The level is the last digit of the
catalogue's word (`scene::level`: `LEVEL1`, `S2MSI1C`); a word without a
digit is `None`, an answer of its own — service annotations (`IW_ETA__AX`)
carry none, and counted as level 1 they would outrank the image by size.

**A product asked by key** (`on_locate` in `cdse.rs`; the key is any path
inside the product, from the storage listing or the library):
`s3::product_root` climbs to the root, `catalogue::locate` asks by exact
`Name`, and when `scene::acquisition` gives the found product a key, a second
query asks its neighbours by acquisition time (`catalogue::siblings`) — names
differ by their tails. The window follows the folding rule: one second each
side where there is no datatake (such parts start at one instant, and
`time::format` asks in whole seconds), `BESIDE_A_SLICE_S` where there is one;
`tileId` narrows a Sentinel-2 second from a whole row of the pass to one
cell; the ceiling follows the window (`siblings_top`), and reaching it is
logged — no order is asked, and a cut part is lost by lot. `same_scene` keeps
the neighbours with the key `group` would give, so a row unfolds the same from
either side. `scene::about` then answers: the asked product is replaced by a
neighbour only twice — level 0, which has no image, and the same quantity
(`quantity`: the type without `-COG`/`_COG`) in tiled layout; otherwise the
asked part is the shown one, the order of parts unchanged — parts can be
different measured quantities, and ozone is no answer about carbon monoxide.
`LocateResponse.answered` tells "no such product" (knowledge about the
product, kept) from "could not ask" (knowledge about today's network, not
kept).

**Not a scene is what has no ring.** Auxiliary data — calibration tables,
ephemerides — are products like any other and outrun the imagery in
freshness, the validity of a table being written as a future date; nothing
outlines them, and `group` drops them. What decides is the parsed ring, not
the presence of `GeoFootprint` (`Facts.framed`, set in `catalogue::parse`;
products with a geometry and no ring are counted into one warning per
answer). `catalogue::rings` takes a Polygon, a MultiPolygon (a scene cut by
the antimeridian) and a polyline alike — rings are drawn as lines, closure is
not needed, and a hole is a ring too; a ring goes whole or not at all
(`place`: a vertex off the Earth or shorter than a pair drops its ring, a
third coordinate is ignored), and the closing vertex is dropped (`closed`).

## The catalogue query

**Two services, one module.** Copernicus Data Space is a catalogue (OData,
`veldmodules/data-provider/src/catalogue.rs`) and a storage (S3,
`veldmodules/data-provider/src/s3.rs`). The catalogue answers metadata as
JSON and asks no keys; the storage serves bytes and asks a SigV4 signature
(`s3::signed`; the keys are in
[configuration.md](../operations/configuration.md)). What joins them is the
catalogue's `S3Path`: without its leading slash it is `DataProduct.identifier`,
the path with the bucket prefix, and the same identifier signs (`on_sign`,
`s3::object`), lists (`on_list_path`), walks (`on_imagery`, `s3::listing_deep`)
and downloads; the prefix comes off in `s3::key` alone. The provider is one
module because what is found must open at once, and the storage layout
(`s3::product_root`, `is_single_object`) is known only there.

**One address** (`catalogue::address`) for the search, the neighbours and the
product by name: `$expand=Attributes` always — mission, product type and
cloud cover live only there; `$orderby ContentDate/Start desc` for the
search; `$top`. An empty `SearchRequest` field narrows nothing (`filter`); an
area of fewer than three points is dropped with a warning — the catalogue
refuses such a polygon whole, and dropped it means the whole Earth. A status
outside 2xx answers with the catalogue's `detail` (`catalogue::failure`), not
the code alone.

**A lower bound in time, always.** It is about speed, not the result: without
one the catalogue sorts the whole archive. When the requester named neither
`from` nor `to`, `on_search` sets the floor `FRESH_DAYS` back
(`catalogue::search(request, floor)`); `to` alone counts as the requester's
own bound — a floor of fresh days under "everything before 2020" is empty by
construction. The floor is not final: when the window ran out of products and
the page of scenes is short, the same request goes once more without it
(`Asked::Search.widened`, in `cdse::on_http_result`), and there is no third
time. "Ran out" is about products, not scenes: the catalogue returned fewer
than `catalogue::asked`; a full page of products with a short page of scenes
is folding, not an empty window — widening it would not help.

**Products, not scenes.** `wanted` is the requester's `limit` in scenes
(`DEFAULT_LIMIT` when unsaid, at most `MAX_LIMIT`, the catalogue's own
ceiling); `asked` is `wanted` times `OVERFETCH` products under the same
ceiling — one acquisition is up to five products, and without the margin a
page would come short by that much. After folding the answer is cut to
`wanted` scenes; `Found.products` keeps the count the decision is made by.

## Which raster of a product is shown

The provider knows the layout of a product, the globe knows how to stretch
open resources over a binding, and `data-browser` joins the two: it asks
`data-provider` `on_imagery`, opens what is named, and sends the globe
`on_overlay`. The messages are in `veldmodules/data-provider/types.proto` and
`veldmodules/globe/types.proto`.

**The answer** (`ImageryResponse`): `rasters` with roles — `IMAGERY_PREVIEW`,
the small quicklook that gives the layer a picture at once, and
`IMAGERY_DETAILED`, the one the globe goes to when zooming in; per raster a
`geolocation`, the sibling file with its pixel coordinates when they lie apart
from it (Sentinel-3; which sibling, `imagery::geolocation` decides by name,
the tie grid before the per-pixel file); `utm`, the `UtmFrame` of a Sentinel-2
tile derived from the MGRS code in the name (`mgrs.rs`; `y1` is the northern
edge, rows run north to south); and `reason` apart from `error` — "could not
ask" is worth asking again, "asked, and there is nothing to show" is not.
`ImageryRequest.downloaded` says the product lies on disk; only the requester
knows it, and one layout branches on it (below). Each raster is opened by the
file, not by the scene (`LibraryState::local_name`, [screen.md](screen.md)):
a downloaded quicklook does not make the measurement raster beside it local.

**Refused before the listing** (`imagery::unviewable`, computed for catalogue
products in `catalogue.rs` and for listing entries in `cdse.rs`, carried as
`DataProduct.unviewable` to the browser row): level 0 by `scene::level` —
receiver echo, no image yet; `hopeless_type` by the product type — `AUX*`
service data, and the Sentinel-5P L1B spectral cube (`L1B_RA_BD`, `L1B_IR_`,
`L1B_ENG`, `L1B_CA`: hundreds of channels per pixel, and the only
two-dimensional planes are viewing angles) — refused before any download and
for the whole product, never for a raster: an SLC keeps its quicklook, and its
complex samples are caught at describe; a product that is one object and not
a raster (`just_a_file`, listing the readable formats from `RASTER_SUFFIXES`)
— `on_imagery` answers it without a request to the storage. Two honest
answers only: "no", with its cause in words — the field is a string, not a
flag, because the causes differ and are told apart only by words;
`Part.unviewable` and `ListEntry.unviewable` carry the same — and "looks like
yes", an empty field, for everything else, a folder never listed included:
layouts are many, a listing costs a request, and a folder with no raster is
explained by `nothing_here` afterwards. A listing entry knows neither level
nor product type, so a scene root in a listing is judged by its name alone,
and an entry that is not a scene root always carries a cause: the globe takes
a whole scene.

**The layout** (`imagery::scan` over the recursive listing of the product's
subtree), from the exact to the guessed:

- Sentinel-2: `*_PVI.jp2` is the preview; the detailed raster is
  `IMG_DATA/R10m/*_TCI_10m.jp2`, else `IMG_DATA/*_TCI.jp2`.
- Sentinel-1: `preview/quick-look.png`; the detailed raster is a
  `measurement/*.tif(f)`, co-polarisation (`-vv-`, `-hh-`) before cross. A
  measurement under `_COG.SAFE/` is offered always; a stripped GRD only when
  `downloaded` — its coarse level costs a pass over the whole file, and the
  coarse level is what the canvas asks for first.
- a product that is one object is its own detailed raster when `is_raster`
  (`single`).
- any other layout (`Scan.guessed`): among the `is_raster` keys — the one list
  `RASTER_SUFFIXES`, shared with `s3::is_single_object`; the tiler decides the
  format by content anyway — the preview is the first `a_quicklook` (the
  pieces quick-look, quicklook, thumb, browse, preview, `_pvi`, and the word
  `bp` of Landsat), and the detailed raster is the `min_by_key` over the keys
  that are neither quicklooks nor under `preview/` (`a_decoration`), by five
  keys in order: not `a_whole_picture` (the words TCI, TRUECOLOR, RGB, TRUE
  COLOR — words, not pieces: `otci.nc` is chlorophyll); `a_picture_format` (a
  suffix outside `MEASURED_SUFFIXES` loses); not `names_the_measurand` (the
  product's own word repeated in the file name more often than in the
  product's); `an_aside` (ancillary, coordinates, geodetic, geolocation); the
  file name. The alphabet is the fifth key, not the fallback.

**The manifest** (`veldmodules/data-provider/src/manifest.rs`) is asked only
when the layout was guessed: the named layouts have already said which file is
the measurement, and a manifest is another signed request. `manifest::key`
finds `manifest.safe` or `xfdumanifest.xml` (`NAMES`) at the product root
only; `manifest::measurements` returns, in manifest order, the `href` of every
`dataObject` that a `contentUnit` whose `unitType` contains "measurement"
(`is_measurement`) points to; a body that is not XML or does not reach the
closing `XFDU` names nothing. `measured_by` narrows the detailed choice to the
named files, never to nothing (a level-0 `.dat` is a measurement too). Not
fetched (`Asked::Manifest`, a status outside 2xx) or empty — the names decide,
with a line in the log.

**Nothing to show** is an answer (`ImageryResponse.reason`), and after the
listing `nothing_here` words it: an empty listing; `_RAW__0S` in the name —
level 0; otherwise "no raster among N files" with the suffixes that are there
(`suffixes`). The browser shows the provider's words (`give_up` in
`veldmodules/data-browser/src/handlers/overlay.rs`), its own only when the
provider was silent.

### The overlay set

`globe/on_overlay` (`Overlays`, `snapshot: true` in `globe/schema.yaml`) is
the only way to tell the globe about overlays, so every change — add, remove,
reorder (`shift`), opacity, hide — ends in the browser sending the whole set
(`send_set`), after it has opened every raster and its `geolocation` file
(`open_resource`, local or over the network by `local_name`) and handed
ownership to the globe (`veldsdk::resource::hand_off`; coordinates are a
property of their raster, and without it they are released). The globe
(`module::on_overlay`, `adopt_overlay` in `veldmodules/globe/src/module.rs`):

- the order of elements is the order of layers bottom to top, and it is taken
  from each message anew; what the message does not name is gone, its pass
  killed and its resources released;
- the same `key` with the same resource ids — raster and `geolocation` alike
  — is the same overlay: only `opacity` and `hidden` are taken, and the frame
  in the message is discarded (a binding read from the raster outranks the
  catalogue's footprint, [georeference.md](georeference.md); a corrected
  footprint arrives only with new resources). Nothing is reopened under a
  slider;
- `opacity` is `optional`: unsaid means 1 — proto3's zero would hide the
  layer by silence;
- `hidden` stays in the set with its resources, is not drawn, asks no tiles
  and loses its running pass (`release_pass`: production is one per source,
  and a hidden layer must not hold it from a visible one); its tiles stay in
  the store, so showing it again opens nothing;
- `Overlay.rough` says the quad is a bounding box, not a binding
  ([georeference.md](georeference.md)).

The appetite cap shared among the pyramids being drawn is in
[viewing-pipeline.md](viewing-pipeline.md).

**Downloading a scene that lies as a directory** is the browser's walk, not
the library's (`veldmodules/data-browser/src/handlers/library.rs`):
`on_download_snapshot` lists the product with `ListPathRequest.recursive`
(folders are not in the answer), and `on_snapshot_files` sends each file as
its own `data_library/on_download` under the scene's `product`. Finished
files are skipped: a re-download removes the finished file before it starts
([download.md](download.md)), and "finish the scene" would erase what it
has — a single file is re-downloaded from its row menu, where the item is
marked irreversible (`components/table.rs`). A walk that reaches
`browse::MAX_ITEMS` stops without naming the scene's count; a whole walk
reports it with `data_library/on_snapshot`. `on_pause_snapshot` pauses each
running file of the scene. Expanding a scene row in the list lists that
folder alone, lazily and once (`request_children`, `state::browse::Children`,
one structure for the catalogue and the search).

## Topics

Both consumers speak to the same two services. Topic names live only in the
modules' `schema.yaml`; this is the order of one show:

1. `image-tiler` `on_describe` → `on_described` (`Described`): fingerprint,
   size, tile side, the level table (`levels`: for every level how it is
   served, its memory peak, whether it fits), the georeference (`ties` or
   `placement`), `binding_trouble`, or an error.
   Declared fast: it decodes nothing but the sample windows of a NetCDF
   candidate (see the table).
2. `tile-cache` `on_query` → `on_tile`… `on_query_done`: what the disk cache
   already holds for this fingerprint and level.
3. `image-tiler` `on_produce` (cancellable) → `on_produced`,
   `on_produce_progress`… `on_produce_done` for the misses. Every produced
   tile is also sent to `tile-cache` `on_store`, fire-and-forget. A pass that
   fails takes every cell of the pass with it, until a later pass over the same
   source succeeds.

The consumer's side — the ladder from coarse to fine, the store of arrived
tiles, the passes in flight — is one implementation in the tiler's wrap crate
(`veldmodules/image-tiler/wraps/rust/src/tiles.rs`), which the canvas and the
globe use as a crate; only the pyramid arithmetic
(`veldmodules/image-tiler/src/pyramid.rs`: `TILE`, level sizes, level count) is
the tiler's own file, included into the wrap through `#[path]`. Which level to
want for a given screen pixel each consumer computes itself; the rest of that
side is [viewing-pipeline.md](viewing-pipeline.md).

## What is read, per format

The tiler does not see whether a resource is a file or a remote object, and
no request tells it: what a level costs is the level table's to say, row by
row. Bytes come through `ResourceReader` windows, except for NetCDF, which
reads the HDF5 file at absolute offsets through the host directly
(`netcdf::Resource`, below). The format is detected by content
(`veldmodules/image-tiler/src/adapters/mod.rs`): JPEG 2000, NetCDF-4 and
BigTIFF (`tiff::BIG_MAGIC`, version 43 in the header) by their own signatures
ahead of the `image` crate, which knows none of them; a classic NetCDF-3
(`netcdf::CLASSIC`) is refused by name. The difference between formats is one
question: can it hand over an arbitrary tile cheaply.

The answer is the **level table** (`veldmodules/image-tiler/src/adapters/table.rs`,
[ADR 0003](../decisions/0003-raster-reading-model.md)): one row per pyramid
level — how it is served (pointwise, or by a pass from some level), how many
source pixels a step costs, the memory peak as named terms, and whether it
fits. The table itself travels in the description (`Described.levels`), and
the branch `produce` takes is read from the same rows; the test
`the_level_table_and_the_produce_branch_agree` in `adapters/reads.rs` holds
the table against the driver's window rule. The two
loops — pointwise `direct` over the chunks of the nearest overview, and the
sequential `pass` feeding the cascade — belong to the chunk grid driver
(`adapters/grid.rs`), behind the trait `Chunked`; a format supplies chunks.

| Format | Rows of the table | What is partial | Named ceilings |
|---|---|---|---|
| Tiled TIFF with overviews (COG), classic or BigTIFF | the chunks under the tiles asked for are named to the carrier ahead by the offsets and byte counts of the catalogue (`Chunked::prefetch`); a level is pointwise when its grid is not degenerate (`MIN_CHUNK_PIXELS`), the region under the worst tile fits `REGION_CAP` and the chunks it touches fit the chunk cache (`Grid::footprint`); with halved overviews every row is pointwise | every pointwise level: only the chunks under the requested tiles | `budget::CHUNK_CACHE` (a share of free memory), `REGION_CAP` (half of it, in source pixels) |
| Stripped TIFF; tiled TIFF without overviews | the fine levels pointwise, a window of strips (or tiles) of the base image per tile, named to the carrier ahead by the offsets and byte counts of the catalogue (`Chunked::prefetch`); the rest by a pass from level 0 that builds every level through the cascade — a pointwise prefix of rows, then passes from level 0; with no pointwise level at all, a pass from level 0 on every row | the pointwise prefix | the same; a pass whose row of chunks plus cascade does not fit `budget::free()` is refused by the sum, not by a ceiling of its own |
| JPEG 2000 (`openjp2`, [ADR 0002](../decisions/0002-jpeg2000-decoder.md), [ADR 0004](../decisions/0004-jp2-excerpt.md)) | a chunk is one codestream tile decoded at a resolution factor (`adapters/codec.rs`, the only `unsafe` of the tiler); the copies are the resolution levels while the tile side divides by two; rows as for TIFF — a Sentinel-2 granule (tiles of 1024², five resolutions) is pointwise on every level, detail limit 0; a degenerate grid (PVI, tiles of 8²) goes by a pass | with a TLM index, the codec reads an excerpt (`adapters/excerpt.rs`): the main header, the tile's own tile-parts — cut after the packets of the level's resolution when PLT and the progression allow it — and EOC; a granule's coarsest level costs one 64 KiB probe per tile, and the probes of all the tiles asked for go to the carrier ahead in one order, the file pieces of an excerpt in another (`Chunked::prefetch`, `veld_resource_prefetch`). Without TLM, the tile-parts under the requested tiles plus the SOT headers the decoder walks: those before the tile, and once per codec all those after it | the same chunk cache and region ceiling; zero is transparent only in a file with a GMLJP2 binding |
| JPEG | every level a pass from itself: the frame decodes at the decoder's scale (1/8, 1/4, 1/2, 1; `jpeg::decoded_size`), is brought to the level's grid, and the cascade goes down from it — a pass from the level itself on every row; the detail limit is the first level whose decoded frame fits | nothing | `FULL_DECODE_BUDGET` on the decoded frame |
| PNG; GIF, BMP, WebP | one pass from level 0 on every row; the cascade builds all levels; a streaming PNG costs a row, an interlaced PNG and the others a whole frame | nothing | `FULL_DECODE_BUDGET` on the whole-frame paths, `MAX_SOURCE_SIDE` |
| NetCDF-4 (HDF5) | the chunk grid driver over a grid of row windows: a chunk is `rows` rows across the full width, read as the reader's region along the plane's row axis (`read_*_region`) and typed as `Samples`; no copies, so rows as for a stripped TIFF — the fine levels pointwise, the rest by a pass from level 0. The window is cut along the plane's row axis, unit axes such as Sentinel-5P's time aside (`rows_of`): a bundle of the file's chunk rows up to `TILE`, the chunk itself when taller, `TILE` for a contiguous layout; a single-chunk variable (SYNERGY) makes the window the whole plane, and the row then says a level-0 tile costs the plane. Metadata on demand through a metadata cache (`METADATA_CACHE`); describe reads headers and up to `SAMPLE_WINDOWS` windows of each candidate in order of preference until one is neither empty nor flat in the sample | the chunks under the requested tiles; at describe, the sample windows of the variables probed and the coordinate grids whole | `TIES_BUDGET` for the coordinate grids; the chunk depth counts the reader's copies (`depth_of`) |

`MAX_SOURCE_SIDE` applies to every format at describe. The table travels
whole, and the consumer derives from its rows the ladder of steps to a target,
which levels are pointwise, and the detail limit — the first row that fits
([viewing-pipeline.md](viewing-pipeline.md)); a source with no fitting row is
refused at describe.

## Display radiometry

How samples become RGBA8 (`veldmodules/image-tiler/src/adapters/radiometry.rs`).
It is a rule of display, not the radiometry of the data: nothing is measured
from the bytes of a tile. The rules key the disk cache through `DECODE_REV`
(below).

**What is accepted.** Samples are `radiometry::Samples`: u8, u16, i16, f32.
A TIFF is refused at describe, from the header, not on the first chunk
(`tiff::ensure_readable`): bits per sample below 8 — the crate hands them
packed, several to a byte, and the tile assembly counts whole samples per
pixel; a compression outside `DECODED` (`ensure_decodable`, naming the
foreign one — Huffman, Fax3, old JPEG, WebP, LERC, LZMA, JPEG XL — and
listing the readable: none, Fax4, LZW, JPEG, Deflate, PackBits, old Deflate,
ZSTD); a `SampleFormat` and bit depth outside `sampled` (`ensure_sampled`:
unsigned 8 and 16, signed 16, float 32; the complex sample of a Sentinel-1
SLC is refused here, before the gigabytes). `DECODED` follows the features of
the `tiff` crate in `veldmodules/image-tiler/config.yaml` — its defaults plus
`zstd` for the Sentinel-1 COG; `zstd-sys` is C, built for wasm with the clang
that `build.py::wasm_cc_env` finds — and `buildgen/tests/test_tiff_decoders.py`
holds the table and the features equal, so a compression not built (WebP) is
refused by the adapter with its name, not by the crate on a chunk. `typed` is
the pair of `sampled` on the chunk: u32, u64, f16, f64 are refused there.
Colour models (`tiff::model`): Gray, GrayA, RGB, RGBA are `Pixel::named`;
`Multiband` — several samples and no colour interpretation, how GDAL writes
any stack of bands — is not a refusal but `Pixel::stack`: the first band is
shown, the rest are stepped over, and its second sample is not alpha, though
a grey-with-alpha pixel has as many. Palette, CMYK, YCbCr are "colour model
not supported"; a planar layout (`PlanarConfiguration` other than 1) is
refused by `chunk_grid`. A BigTIFF takes the same path (`BIG_MAGIC`).

**Stretch.** Bytes are identity. Wide samples are stretched by the
percentiles [2 %, 98 %] of a sample of the file itself
(`percentile_stretch`), edges clamped: a DEM lies in metres, radar amplitude
in DN with a handful of hot outliers, and the high byte of either is a black
frame. The mapping is one per file (`Mapping`, kept in `Layout::stretch` of
the TIFF and JPEG 2000 layouts from first need): stretched each by itself,
neighbouring tiles would meet at seams, and which brightness went into the
disk cache would depend on the order of orders. The sample is
`STRETCH_SAMPLES` values (2²⁰), one threshold for everyone who samples: a
TIFF takes up to four chunks spread over its smallest overview
(`Layout::stats`), a JPEG 2000 of precision above 8 bits four tiles at the
coarsest factor, a NetCDF variable up to four row windows at a stride coprime
with its width (`netcdf::sampling_step`), so that a swath's unmeasured edge
columns do not make the sample. Only the colour samples of a pixel go in: alpha and unshown
bands would move the percentiles. A flat field — equal percentiles — lands in
the middle of the scale, not at its bottom, where a black frame is
indistinguishable from emptiness; the half-width is taken from the value's
own magnitude, `|lo|.max(1)·1e-6/2`, because past 2²⁴ an f32 does not change
by one, and such values are exactly what fill marks are. A sample with no
valid value at all is identity: no limit is invented, since any would white
out everything above it.

**No data.** `radiometry::is_data` — finite and not the mark — is the one
predicate for the stretch sample and for the display: a value the sample
threw out and the frame showed would be opaque and, above the top of the
stretch, white. The mark is `GDAL_NODATA` of the base IFD for a TIFF,
`_FillValue` for NetCDF, zero for a JPEG 2000 with a GMLJP2 binding (the
table above). It is compared raw, before the stretch; a pixel whose every
colour channel is the mark, or any channel of which is not a number, becomes
transparent black (`Mapping::rgba`). The alpha of a GrayA or RGBA file is
scaled to a byte, not stretched.

**Transparent is not mixed into colour.** Downscaling — `halve` for the
pyramid step, `resample` for an overview or a decoded frame brought to a
level's grid (`veldmodules/image-tiler/src/resample.rs`) — averages colour
with alpha as the weight; a wholly opaque input is unchanged to the byte.
Under a transparent pixel lies the colour of its nearest opaque neighbour
(`adapters::bleed_alpha`, `BLEED_STEPS` rings), the alpha untouched: both
consumers filter linearly with unpremultiplied alpha, so black under the edge
of a `nodata` field gives a dark halo a texel wide, doubled by every ancestor
step a cell is drawn from. It is applied once, in the tiler's sink
(`Sink::emit` in `veldmodules/image-tiler/src/module.rs`), where the tiles of
every adapter converge, on a copy and only when the tile has a transparent
pixel at all.

## NetCDF: choosing the variable

A NetCDF-4 file is a set of measured quantities — surface temperature, column
gas, quality, precision, time, viewing angles — and to show it is to choose
the one that is the measurement
(`veldmodules/image-tiler/src/adapters/netcdf.rs`). The rules are CF, the ones
the file describes itself with; there are no mission name patterns. Where the
file's coordinates come from — `swath_pair`, `grid_axes`, the sibling file —
is [georeference.md](georeference.md).

**Reading.** The file is HDF5, addressed by absolute offsets, and it is read
on demand: `netcdf::Resource` is the `hdf5_pure::Source` — the resource id,
its length and a high-water mark of the read (`reached`, the pass's progress)
— whose `read_at` is `veldsdk::abi::resource_read` at an offset; a short
answer is `UnexpectedEof`, not "what there was". The reader's metadata cache
is `METADATA_CACHE`; raw chunks do not go into it. The file's length enters no
ceiling: gigabytes not reached cost neither memory nor wire. Samples are read
as regions along the plane's row axis (`read_rows`, `Layout::region`):
`read_u8_region`, `read_u16_region`, `read_i16_region` typed as `Samples`,
everything else through `read_f32_region`.

**The sieve** (`survey`, `describe_item`). A candidate is two-dimensional
with unit axes dropped (`Item.plane`) and numeric; not marked `flag_values`
or `flag_masks` (a state code, not a measurement); not named by another
variable's `coordinates` or `ancillary_variables` ("where" and "how precise",
not "what"); and not itself "where" or "when" — `northing` and `easting` by
units (`degrees_north`/`degree_north`, `degrees_east`/`degree_east`),
`timing` by the first word of the units, from `s` to `years`, so that
`m s-1` stays a speed. The walk stops at `MAX_DATASETS` (512) variables and
`MAX_DEPTH` (8) groups.

**The order** (`preferred`), from "what is this file" to "what is main in
it": placeable first — the variable has coordinates of its own, a
`swath_pair` or `grid_axes` (`placeable`; a variable with nowhere to lie is
not the image of this granule whatever it measures: in a Sentinel-5P L1B
granule the instrument's wavelength table — fractional, not angular, at the
same depth — wins over the viewing angles by every other argument; the test
is `место_спрашивается_раньше_всех_прочих_доводов`); then closer to the root
(`depth`: details of the computation lie in subgroups); floating point before
integer (`real`: an integer in CF is a counter, a code, an index); not angular
before angular (`angular`: degrees, deg, degrees_t, radians, rad — a closed
angle does not stretch, 359° and 1° are neighbours); then the path
alphabetically, which is determinacy, not choice. Selection, not a sieve: a
file that says nothing about place is still shown, on the catalogue's
footprint.

**A sample chooses** (`describe`, `probed`, `sampled`). Candidates are
sampled in that order: up to `SAMPLE_WINDOWS` row windows spread over the
height, the first and the last at the edges, each subsampled at a stride
coprime with the width (`sampling_step`) so that the whole sample stays under
`STRETCH_SAMPLES`. One with no `is_data` value in the sample is skipped — SLSTR
carries quantities measured over the ocean only, solid `_FillValue` over land,
and shown it would be a transparent frame without a word; a flat one
(`spread`) is the answer of last resort, kept with its layout and never reread.
A variable measured only outside the sample windows is skipped as empty: that
is the price of not reading the plane. A variable of no more than
`SAMPLE_WINDOWS` windows — the plane-sized window of Sentinel-5P and SYNERGY,
the three chunks of 5026 rows of OLCI GIFAPAR — is read whole by the sample,
one window at a time: the sample saves the memory of a window, not bytes, and
the first tiles read the same rows again (over the network from the block
pool). Skipped candidates are named in the
log (`skipped`), and a file with no candidate at all is refused with
`explain`, in the words the decision was made with. The winner leaves describe
as a `Layout`: the row grid, the sample type, the fill mark and the stretch
computed from the same sample (`mapping`); no samples stay in the memo.

**Marks and packing.** `_FillValue` (`Item.fill`) is the nodata mark of the
display. `scale_factor` and `add_offset` are not applied to the shown quantity
(`mapping`): the transform is linear and increasing, the stretch is by
percentiles of the same values, and "no data" is compared with the raw value
as written. Coordinates are unpacked (`unpacked`): Sentinel-3 writes them as
scaled integers, and unscaled they are not degrees.

**Memory is the grid's count.** A window of rows is a chunk of the grid, and
its cost is the driver's: `Grid::direct_peak` for a pointwise level,
`Grid::pass_peak` for a pass, with the chunk depth from `depth_of` — the raw
window, the decompressed file chunk beside it while the window is assembled,
and the typed copy the reader makes for 16-bit and f32 samples (or the f32 it
unpacks the rest into). A variable whose window is the plane asks for the
plane in every term, and the row's `fits` says whether that is admitted; there
is no ceiling of NetCDF's own. The coordinate grids are still read whole
(`read_f32`), against `TIES_BUDGET` (`ties_peak`). The Sentinel-5P L1B cube is
never described: the provider extinguishes the product type before a download
(`hopeless_type`, above), and inside such a file its tables lose by
`placeable`.

## Memory

An instance has `budget::INSTANCE` of linear memory, `budget::RESERVE` of it is
always taken, and `budget::free()` is what work on a source may use
(`veldmodules/image-tiler/src/budget.rs`). Every path sums the memory of its
work into a `budget::Peak` of named terms — strip, chunk, cascade, region,
frame — and asks `admit()` against `free()`; the table's `fits` column
is the same sum computed at describe, and `produce` adds the parse of a
neighbouring source still held in the memo before it starts. Two shares are
set from the budget: the chunk cache of pointwise reading (`budget::CHUNK_CACHE`)
and, from it, the region a tile may read (`grid::REGION_CAP`). The ceilings
that stay named are those of a path's own decoder — `FULL_DECODE_BUDGET` for a
frame decoded whole — and `TIES_BUDGET` for the coordinate grids of a swath.

## Fingerprint and cache

The identity of a source is its fingerprint: the length and `SAMPLE` bytes from
each end (`veldmodules/image-tiler/src/fingerprint.rs`), so a file on disk and
the same object over the network share one cache, and a half-downloaded
`.part` has another. The suffix `-t<TILE>q<DECODE_REV>` keys the tile side and
the decoding rules: `DECODE_REV` is bumped by hand when the stretch, nodata
keying or downscaling weights change, and the old cache directories age out.
`tile-cache` owns the directories under `runtime/data/tiles/`.
