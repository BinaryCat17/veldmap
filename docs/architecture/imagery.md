# Imagery: from a catalogue product to tiles on screen

How a raster gets from a file or a remote object to the tiles that the canvas
(`image-view`) and the globe (`globe`) draw. Terms are in
[the glossary](../glossary.md). This page describes the tree as it is; what
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

| Format | How levels are read | What is partial | Ceilings |
|---|---|---|---|
| Tiled TIFF with overviews (COG) | each level from the nearest overview at or above it, only the chunks under the requested tiles (`produce_direct`) | every level, as long as each level's region per tile fits `REGION_CAP` and its chunks fit `CHUNK_CACHE_BYTES` (`tiff::windowed`) — then `reach = EXACT`; otherwise the covered low levels are pointwise (`windowed`) and the rest go by a pass | `REGION_CAP` (source area per tile), `CHUNK_CACHE_BYTES` (decoded chunks kept) |
| Stripped TIFF; tiled TIFF without overviews | fine levels pointwise, a window of strips (or tiles) of the base image per tile; coarse levels by one sequential pass that builds every level through the cascade (`produce_pass`) | the lowest `windowed` levels, as many as `REGION_CAP` and `CHUNK_CACHE_BYTES` allow; `reach = WINDOWED`, and the ladder has no intermediate steps | `REGION_CAP`, `CHUNK_CACHE_BYTES` (how many levels are windowed), `BAND_CAP` (one row of chunks in a pass), `MAX_SOURCE_SIDE` |
| JPEG 2000 (`hayro-jpeg2000`) | the whole file is read on every pass; the frame decodes at the requested level by skipping DWT levels — or finer, when the file allows fewer skips — and the cascade goes down from the level actually decoded (`reach = COARSER`) | nothing: no region, no tile | `DECODE_BUDGET` against `jp2::estimate` (file plus decoder planes), which sets `finest` at describe |
| JPEG | the whole frame decodes at the smallest of the scales 1/8, 1/4, 1/2, 1 that is not coarser than the level; the cascade goes down from it (`reach = COARSER`) | nothing | `FULL_DECODE_BUDGET` per request; no `finest`, so a frame that does not fit is refused on every request |
| PNG; GIF, BMP, WebP | one pass from level 0; the cascade builds all levels (`reach = PYRAMID`) | nothing | `FULL_DECODE_BUDGET` on the non-streaming paths, `MAX_SOURCE_SIDE` |
| NetCDF-4 (HDF5) | metadata on demand through a metadata cache; at describe, candidate variables are read whole into f32 in order of preference until one is neither empty nor flat, and the winner is kept as the heavy memo until the pass; the pass feeds the cascade in strips of `TILE` rows (`reach = PYRAMID`) | the file: only the chunks of the variables probed and of the coordinate grids; a plane itself: nothing | `PROBE_BUDGET` (bytes read while probing), `affordable()` against `budget::free()` (plane, cascade strips, RGBA strip), `TIES_BUDGET` for the coordinate grids, `WIRE_PLANE` for a remote variable |

`MAX_SOURCE_SIDE` applies to every format; it is named where it is the only
ceiling by side. `windowed` counts the levels from 0 that are read pointwise;
`reach` says what one pass covers; `finest` is the finest level the source will
ever serve. `Info::reach()` and `produce` branch on the same kinds and must
agree; today that agreement is held by a comment, not by a test.

## Memory

An instance has `budget::INSTANCE` of linear memory, `budget::RESERVE` of it is
always taken, and `budget::free()` is what work on a source may use
(`veldmodules/image-tiler/src/budget.rs`). Only NetCDF adds its terms up
against `free()`; the other ceilings in the table are assigned numbers.

## Fingerprint and cache

The identity of a source is its fingerprint: the length and `SAMPLE` bytes from
each end (`veldmodules/image-tiler/src/fingerprint.rs`), so a file on disk and
the same object over the network share one cache, and a half-downloaded
`.part` has another. The suffix `-t<TILE>q<DECODE_REV>` keys the tile side and
the decoding rules: `DECODE_REV` is bumped by hand when the stretch, nodata
keying or downscaling weights change, and the old cache directories age out.
`tile-cache` owns the directories under `runtime/data/tiles/`.
