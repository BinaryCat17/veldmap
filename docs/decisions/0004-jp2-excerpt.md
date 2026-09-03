# 0004 — JPEG 2000 tile excerpt

Status: accepted (2026-09-03). Rests on the decoder (0002) and the reading
model (0003).

## Context

openjp2 finds a tile by walking SOT headers from the last one it read, and
once per codec walks every header after the tile to verify the tile-part
count; each header costs a reader window, over the network a pool block, and
a tile-part is read whole at any resolution factor. For a Sentinel-2 TCI of
121 tile-parts that meant about half of its 129 MiB per `produce`, and the
coarsest level cost the whole file. Measured on 2026-09-03, a granule written
by Kakadu carries a TLM in the main header (one tile-part per tile, in index
order), a PLT in every tile-part, LRCP with one layer, precincts of 256², no
SOP, EPH, POC or COC; the packets of the coarse resolutions lie first, and the
two coarsest resolutions of a tile take under 40 KiB of its 1.1 MiB, the third
under 156 KiB.

## Decision

The tiler builds its own **tile-part index** from the TLM and hands the codec
an **excerpt** per tile: the bytes before the first SOT (JP2 boxes included),
that tile's own tile-parts, EOC — a stream with nothing to walk; a codec is
opened per tile. When the PLT is there, the progression is resolution-major
(RLCP, RPCL, or LRCP with one layer) and the tile header overrides nothing,
the tile-parts are **cut after the packets of the wanted resolution**, counted
by the precinct rule of Annex B; the missing packets are written as empty
ones, and Psot and TNsot are rewritten in the copy of the header, so the
strict decoder sees a complete codestream. The cut is taken only when the
tile's PLT count equals the tiler's own packet count; otherwise the tile-parts
go whole. A probe of 64 KiB per tile-part carries the header and, for a
granule, the two coarsest resolutions — the three coarsest pyramid levels.
Three outcomes: describe decides between the walk and the excerpt — no TLM,
or a main header past the 64 KiB head, and the decoder walks as before — and
names it in its `perf` line; between tile granularity and level granularity
the excerpt decides per tile, by what its tile-parts carry. Index, cut and
reader are one file, `veldmodules/image-tiler/src/adapters/excerpt.rs`.

## Rejected

Trusting the TLM without the sum check (the addresses are sums of lengths,
one wrong entry sends every later tile into foreign data); feeding the J2K
codec the codestream alone (the container's palette and channel boxes would be
lost); truncating without padding — a tile-part that ends before its last
packet is not a codestream of the standard, and whether a decoder tolerates
it is that decoder's business (openjp2 happens to read zeros past the end);
moving the block size of the network here (roadmap step 12).

## Consequences

Measured on 2026-09-03 on the L2A granule of `uitests/fixtures/s2-tci.txt`
(T31TGJ, 135 041 962 bytes), `scratchpad/jp2probe.py` over its TLM and PLT.
Prefix of a tile-part by factor, median over 121 tiles / sum: factor 4 —
9.0 KiB / 1.02 MiB; 3 — 35 KiB / 3.9 MiB; 2 — 134 KiB / 15.1 MiB; 1 — 476 KiB
/ 53.7 MiB; 0 — whole, 128.8 MiB. What the module asks for the coarsest level
is 121 probes of 64 KiB, 7.6 MiB, of which it needs 1.02 MiB; a tile of
several tile-parts pays a probe per tile-part. The fake-host journal on a
16-tile fixture: a tile of level 0 reads only its own tile-part, the coarsest
level costs one probe per tile and under an eighth of the file, and both
decode byte for byte as the whole-file path does; a JP2 container and a TLM
without PLT decode the same (`adapters/reads.rs`). A tile of several
tile-parts is verified on bytes only: the crate's encoder cannot write one
(with `tp_on` it puts every packet of the tile, and the same Psot, into each
tile-part), so no fixture reaches the decoder with a dropped tile-part.
The scenarios below ran with a cold tile cache (`run-uitests.py` sets it
aside). `uitests/jp2canvas.txt` on the local granule: 23.3 s (15.0 s when
its tiles were cached). Over the network (`uitests/jp2remote.txt`, the
neighbouring granule T31TGK, 129.0 MiB, the copy `jp2remote.trace.log` kept
by the runner): the coarsest level took 75.0 s, and by its end the network had
delivered 109.6 MiB (84 %) in 64 requests of 1.71 MiB on average; the four
finer levels shown next came from the pool (697 hits) in 0.6–8.1 s each; the
resource closed at 115.6 MiB (89 %), 76 requests, 231 of 259 blocks; the
scenario took 141 s. So over the wire the excerpt changes what is asked, not yet
what is delivered: the pool block is 512 KiB and the readahead doubles a
request whose miss lands right after the previous one, which is what tile-part
headers two blocks apart look like. The scenario promises `delivered 95` —
the file is not fetched twice — and the tightening belongs to the network
(roadmap step 12), with these numbers.
