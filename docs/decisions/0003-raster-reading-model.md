# 0003 — Raster reading model

Status: accepted (2026-09-03)

## Context

Every format answers the same question — can it hand over an arbitrary tile
cheaply, and at what price in memory — but the answer was written three
times: a `match` over the kinds in `Info::reach()`, another in `produce`, a
comment asking to edit them together, and twelve memory ceilings assigned by
hand in six files, none of them summed against the instance's limit. The
tiled path (chunks of the nearest overview) and the sequential pass were TIFF's
own loops, so JPEG 2000 could get neither.

## Decision

A **level table** (`veldmodules/image-tiler/src/adapters/table.rs`) is the one
statement of the model: for every pyramid level of a source, how it is served
(`Serve::Pointwise`, or `Serve::Pass { from }`), what one step costs in source
pixels, the memory peak of that work as named terms (`budget::Peak`), and
whether it fits. The three scalars on the wire — `reach`, `windowed`, `finest`
— and the branch taken by `produce` are all read from that table. A **chunk
grid driver** (`adapters/grid.rs`) owns the two loops over any source that
reads in chunks, behind the trait `Chunked`; TIFF, JPEG 2000 and NetCDF
supply chunks. **One memory count**: `budget::free()` is the only
limit, every path sums its terms into a `Peak` and asks `admit()`; the cache of
chunks is a named share of it, and the region ceiling follows from the cache.
A level is pointwise when its chunk grid is not degenerate (a chunk of at least
`grid::MIN_CHUNK_PIXELS`), the region under the worst tile fits
`grid::REGION_CAP`, and the chunks it touches fit the cache; otherwise it is
served by a pass from level 0. A decoder that produces a frame at the level's
own scale (JPEG, JPEG 2000) is a pass from that level. The detail limit is the
first level whose row fits.

## Rejected

A `Serve` column on the wire now: the table for JPEG 2000 becomes true only
with the tile decoder (0002), and a promise the adapter does not keep is worse
than three scalars. Deriving the chunk cache from what is left of memory: it
would make the window rule depend on what else the instance holds, and the
same file would be pointwise on one day and not on another. Removing the
per-frame ceiling of the whole-frame paths: it is named in the table instead.

## Consequences

The scalars and the branch cannot drift: `reads.rs` checks the table against
the driver's own window rule on four layouts; the table's tests hold the real
sizes. Computed from the constants on 2026-09-03: `budget::free()` is 928 MiB,
the chunk cache 116 MiB, the region ceiling 15 204 352 source pixels per tile.
Sentinel-1 GRD 25309×17408 as one-row strips: levels 0 and 1 pointwise (level 1
holds 1025 strips, about 99 MiB of RGBA), levels 2–6 by a pass whose peak is
about 123 MiB, almost all of it the cascade's strips; the same raster as a COG
with halved overviews: every level pointwise. Sentinel-2 TCI 10980² as JPEG
2000 read by codestream tiles (0002) is pointwise on every level, detail limit
0; as a JPEG it would be level 1, the first whose frame at the decoder's scale
fits `FULL_DECODE_BUDGET`. PVI 343² with 8×8 chunks is a
degenerate grid and reads by a pass. What the record obliges: the wire
carries the table itself (`Described.levels`), and a new format adds rows,
not a `match` arm elsewhere.
