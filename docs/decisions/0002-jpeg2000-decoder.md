# 0002 — JPEG 2000 decoder

Status: proposed (2026-09-02). Depends on the raster reading model, a later
record written with the chunk-grid driver (roadmap step 7).

## Context

Sentinel-2 granules are the catalogue's main format, and the current decoder
(`hayro-jpeg2000`) knows neither region nor tile: every pass reads and decodes
the whole file, so a TCI granule is served two levels coarser than native
(`docs/architecture/imagery.md`). `openjp2` 0.6, a port of OpenJPEG, decodes
one tile at a resolution factor and has an encoder for fixtures; as a port of
C, its asserts trap in wasm, errors arrive through a callback, and TLM/PLT
markers are parsed only to be discarded.

## Decision

Replace hayro with openjp2 inside the shared chunk-grid driver, once that
driver is extracted from TIFF: a chunk is one codestream tile at the level's
factor, and memory — one decoded tile, the strip of rows for the cascade, the
cascade's own strips — is counted by the shared budget, not by a ceiling of the
adapter's own. The tile path is `get_decoded_tile`: it hands back the codec's
i32 planes and keeps working after `decode` has left the codec in EOC state,
where `set_decode_area` is refused. Strict mode; the `unsafe` island in one
file (stream over `ResourceReader`, error callback, codec discarded after any
failure); `describe` never runs the codestream parser on a file not about to be
shown. A single-tile codestream gets its detail limit from the tile's memory
at the factor. A non-zero origin (`XOsiz`, `YOsiz`) is refused at `describe`
by name: the decoder's reduced size is up to a pixel off the tiler's halving
ladder, and a silent shift shows as seams.

## Rejected

Region decoding per cell: the unit of reading is a tile-part, and a probe on
synthetic codestreams (bytes counted on the read callback, one 512² cell at
level 0) read 0.58–1.91× of the file per cell, the walk running through every
SOT header to the end of the stream. A JP2-only loop with its own ceiling and
`finest = 0`, the plan this record replaces: bytes per level stay equal to the
file, and `finest = 0` traps on a single-tile stream. Keeping both decoders.

## Consequences

Measured before 2026-09-02 by parsing the main header with `jp2::codestream`
(the file is not kept): TCI 10980² — tiles 11×11 of 1024², origin (0, 0), 3
components, 5 resolutions against 6 pyramid levels; PVI 343² — tiles 43×43 of
8². TLM/PLT presence is not measured; roadmap step 3 measures it. Level 0
memory for that TCI, computed from the constants on 2026-09-02: strip of rows
≈ 45 MB, one decoded tile ≈ 13 MB (i32 per sample), cascade strips
`cascade::bytes(10980, 10980)` ≈ 56 MB — about 114 MB against `budget::free()`,
where `jp2::estimate` for the same level is about 3 GB today. Obligations: a
fixture from the crate's encoder; openjp2 equal to hayro byte for byte on the
5/3 wavelet and within a named tolerance on 9/7; a scenario in `uitests/` that
proves level 0 on a granule; wasm size and start time recorded here.
