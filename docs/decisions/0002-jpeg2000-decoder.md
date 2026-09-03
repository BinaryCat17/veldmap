# 0002 — JPEG 2000 decoder

Status: accepted (2026-09-03). Rests on the raster reading model (0003).

## Context

Sentinel-2 granules are the catalogue's main format. The previous decoder
(`hayro-jpeg2000`) knew neither region nor tile: every pass read and decoded
the whole file, so a TCI granule was served two levels coarser than native.
`openjp2` 0.6, a port of OpenJPEG, decodes one tile at a resolution factor
and has an encoder for fixtures; as a port of C, its asserts trap in wasm,
errors arrive through a callback, and TLM/PLT markers are parsed only to be
discarded.

## Decision

openjp2 inside the shared chunk grid driver: a chunk is one codestream tile at
the level's factor (`get_decoded_tile`), the copies are the resolution levels
while the tile side divides by two, and memory is counted by the shared budget
(`SAMPLE_BYTES` per sample: i32 planes at the image and at the tile coder).
The `unsafe` island is one file (`veldmodules/image-tiler/src/adapters/codec.rs`):
a stream over the resource reader through the C callbacks, an error handler
that is always set, a codec discarded after any failure, strict mode; the
decoder's own parser never runs at describe — the tiler's own header parser
does. A non-zero canvas or tile origin is refused at describe by name: the
decoder's reduced grid is up to a pixel off the tiler's halving ladder. Zero
is transparent only in a file with a GMLJP2 binding, where the granule's valid
range starts at one. Samples wider than a byte go through the file's one
stretch (`radiometry::Mapping`), sampled from the coarsest copy.

## Rejected

Region decoding per cell: the unit of reading is a tile-part, and a probe on
synthetic codestreams read 0.58–1.91× of the file per cell. A JP2-only loop
with its own ceiling and `finest = 0`. Keeping both decoders. Reusing a codec
across factors: the factor is set once per codec, so a change of factor opens
a new codec over a new reader.

## Consequences

Measured on 2026-09-03. `build/plugins/veldmap_image_tiler.wasm`: 2 842 418
bytes with hayro, 3 123 704 bytes with openjp2 in its place. A reversible (5/3)
fixture from the crate's encoder decodes byte for byte at factors 0 and 2
(test in `codec.rs`), and through the driver a tile of level 0 equals the
source (`adapters/reads.rs`); no comparison against another decoder was made,
on 5/3 or 9/7 — hayro is gone. The read journal on a 16-tile fixture of 13.7
MiB: a tile of level 0 reads its own tile-part (about 0.85 MiB), the SOT
headers of the tile-parts before it, and — once per codec, on its first tile
— every SOT header after it up to EOC, which is how OpenJPEG verifies the
tile-part count; every header costs one reader window, so the first tile of a
codec reads about a third of that file, and the next tile of the same codec
reads from the last SOT. A codec lives for one `produce` call. Over the
network a header costs a pool block, so a TCI of 121 tile-parts pays about
half of its 129 MiB per `produce`; the tile-part index of 0004 replaces the
walk. On the L2A granule measured on 2026-09-02 (opened by
`uitests/jp2canvas.txt`), the TCI describes with no detail limit and is served
to level 0; the scenario asserts the limit's caption is absent. Start time of
the module was not measured; the scenarios' durations did not change. The
granule's layout — tiles 11×11 of 1024², origin (0, 0), 5 resolutions against
6 pyramid levels, LRCP, one layer, a TLM of 121 tile-parts, PLT in the first
tile-part — is as measured on 2026-09-02; PVI 343² with tiles of 8² is a
degenerate grid and reads by a pass. One rule the decoder forced on the SDK: a
module is a WASI reactor without `_start`, the main thread of its libc is never
set up, and musl's `pthread_key_delete` — which std calls when it registers
the destructor of a `thread_local!` — walks a ring of threads that does not
exist; the SDK defines the four key functions itself for the one thread a
module has (`veldcore/sdk/rust/src/tsd.rs`).
