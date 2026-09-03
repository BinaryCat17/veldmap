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
(`veldmodules/data-provider/src/scene.rs`): by datatake and slice number for
Sentinel-1, by grid tile and second for Sentinel-2, by orbit, instrument and
second for Sentinel-3 and 5P. Which part is shown is a rule, not a list of
missions (`scene::rank`): raw level 0 last, a product without a declared level
before it, then by level, tiled layout before stripped, otherwise the smaller.
A product whose geometry yields no ring is not a scene (auxiliary data) and is
not listed.

## Topics

Both consumers speak to the same two services. Topic names live only in the
modules' `schema.yaml`; this is the order of one show:

1. `image-tiler` `on_describe` → `on_described` (`Described`): fingerprint,
   size, tile side, level count, `reach`, `windowed`, `finest`, the
   georeference (`ties` or `placement`), `binding_trouble`, or an error.
   Declared fast, and it decodes nothing — except NetCDF, which reads whole
   variables here (see the table).
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
want for a given screen pixel each consumer computes itself.

## What is read, per format

The tiler does not see whether a resource is a file or a remote object, but it
is told whether it is near (`near` in the requests), and NetCDF branches on
that. Bytes come through `ResourceReader` windows, except for NetCDF, which
reads the HDF5 file at absolute offsets through the host directly. The format
is detected by content (`veldmodules/image-tiler/src/adapters/mod.rs`), and the
difference between formats is one question: can it hand over an arbitrary tile
cheaply.

The answer is the **level table** (`veldmodules/image-tiler/src/adapters/table.rs`,
[ADR 0003](../decisions/0003-raster-reading-model.md)): one row per pyramid
level — how it is served (pointwise, or by a pass from some level), how many
source pixels a step costs, the memory peak as named terms, and whether it
fits. `reach`, `windowed` and `finest` on the wire are read from it, and so is
the branch `produce` takes; the test `reach_and_the_produce_branch_agree` in
`adapters/reads.rs` holds the table against the driver's window rule. The two
loops — pointwise `direct` over the chunks of the nearest overview, and the
sequential `pass` feeding the cascade — belong to the chunk grid driver
(`adapters/grid.rs`), behind the trait `Chunked`; a format supplies chunks.

| Format | Rows of the table | What is partial | Named ceilings |
|---|---|---|---|
| Tiled TIFF with overviews (COG) | a level is pointwise when its grid is not degenerate (`MIN_CHUNK_PIXELS`), the region under the worst tile fits `REGION_CAP` and the chunks it touches fit the chunk cache (`Grid::footprint`); with halved overviews every level is — `reach = EXACT` | every pointwise level: only the chunks under the requested tiles | `budget::CHUNK_CACHE` (a share of free memory), `REGION_CAP` (half of it, in source pixels) |
| Stripped TIFF; tiled TIFF without overviews | the fine levels pointwise, a window of strips (or tiles) of the base image per tile; the rest by a pass from level 0 that builds every level through the cascade — `reach = WINDOWED`, and the ladder has no intermediate steps; with no pointwise level at all, `PYRAMID` | the pointwise prefix (`windowed`) | the same; a pass whose row of chunks plus cascade does not fit `budget::free()` is refused by the sum, not by a ceiling of its own |
| JPEG 2000 (`openjp2`, [ADR 0002](../decisions/0002-jpeg2000-decoder.md)) | a chunk is one codestream tile decoded at a resolution factor (`adapters/codec.rs`, the only `unsafe` of the tiler); the copies are the resolution levels while the tile side divides by two; rows as for TIFF — a Sentinel-2 granule (tiles of 1024², five resolutions) is pointwise on every level, `reach = EXACT`, detail limit 0; a degenerate grid (PVI, tiles of 8²) goes by a pass | the tile-parts under the requested tiles, plus the SOT headers the decoder walks: those before the tile, and once per codec all those after it | the same chunk cache and region ceiling; zero is transparent only in a file with a GMLJP2 binding |
| JPEG | every level a pass from itself: the frame decodes at the decoder's scale (1/8, 1/4, 1/2, 1; `jpeg::decoded_size`), is brought to the level's grid, and the cascade goes down from it (`reach = COARSER`); `finest` is the first level whose decoded frame fits | nothing | `FULL_DECODE_BUDGET` on the decoded frame |
| PNG; GIF, BMP, WebP | one pass from level 0; the cascade builds all levels (`reach = PYRAMID`); a streaming PNG costs a row, an interlaced PNG and the others a whole frame | nothing | `FULL_DECODE_BUDGET` on the whole-frame paths, `MAX_SOURCE_SIDE` |
| NetCDF-4 (HDF5) | one pass from level 0 over the plane kept by describe (`reach = PYRAMID`); metadata on demand through a metadata cache; at describe, candidate variables are read whole into f32 in order of preference until one is neither empty nor flat, and the winner is kept as the heavy memo until the pass; the pass feeds the cascade in strips of `TILE` rows | the file: only the chunks of the variables probed and of the coordinate grids; a plane itself: nothing | `PROBE_BUDGET` (bytes read while probing), `TIES_BUDGET` for the coordinate grids, `WIRE_PLANE` for a remote variable |

`MAX_SOURCE_SIDE` applies to every format at describe. `windowed` counts the
pointwise levels from 0; `reach` says what one pass covers; `finest` is the
finest level the source will ever serve — the first row that fits, or the top
when none does.

## Memory

An instance has `budget::INSTANCE` of linear memory, `budget::RESERVE` of it is
always taken, and `budget::free()` is what work on a source may use
(`veldmodules/image-tiler/src/budget.rs`). Every path sums the memory of its
work into a `budget::Peak` of named terms — strip, chunk, cascade, region,
frame, plane — and asks `admit()` against `free()`; the table's `fits` column
is the same sum computed at describe, and `produce` adds the parse of a
neighbouring source still held in the memo before it starts. Two shares are
set from the budget: the chunk cache of pointwise reading (`budget::CHUNK_CACHE`)
and, from it, the region a tile may read (`grid::REGION_CAP`). The ceilings
that stay named are those of a path's own decoder: `FULL_DECODE_BUDGET` for a
frame decoded whole, and the NetCDF probe and wire limits.

## Fingerprint and cache

The identity of a source is its fingerprint: the length and `SAMPLE` bytes from
each end (`veldmodules/image-tiler/src/fingerprint.rs`), so a file on disk and
the same object over the network share one cache, and a half-downloaded
`.part` has another. The suffix `-t<TILE>q<DECODE_REV>` keys the tile side and
the decoding rules: `DECODE_REV` is bumped by hand when the stretch, nodata
keying or downscaling weights change, and the old cache directories age out.
`tile-cache` owns the directories under `runtime/data/tiles/`.
