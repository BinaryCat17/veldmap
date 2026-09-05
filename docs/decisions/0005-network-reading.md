# 0005 — Network reading: prefetch and readahead by consumption

Status: accepted (2026-09-05). Rests on the excerpt (0004) and the reading
model (0003).

## Context

A remote raster is read through a pool of blocks of `BLOCK` (512 KiB) and a
readahead that doubles a request whose miss lands right after the previous
one. Pointwise reading defeats that rule twice over: the excerpt asks one
probe per tile-part, and headers two blocks apart look sequential often
enough that the coarsest level of a Sentinel-2 granule delivered 84 % of
129 MiB for the megabyte it needed (0004); a tile order of a COG asks its
chunks one miss at a time, each miss a request of half a second. The
signature of a storage object is issued in the request headers, which the
storage accepts for a quarter of an hour after `x-amz-date`, so a layer kept
on the globe longer than that loses its source at the next miss — 401/403 is
not retried, and nothing signs the object again.

## Decision

Three rules, one exchange. **The reader names what it will read**:
`veld_resource_prefetch(id, ranges)` on both sides of the ABI and
`RangeSource::prefetch`; the network fetches the missing blocks under the
ranges as runs of adjacent blocks — no run longer than `READAHEAD`, no order
larger than `PREFETCH_CAP` — `IN_FLIGHT` requests at a time, into the same
pool, and leaves the readahead alone. The chunk grid driver orders the chunks
under the tiles it was asked for before reading them (`Chunked::prefetch`):
TIFF by its catalogue, JPEG 2000 with a TLM by its probes and then by the
pieces of the assembled excerpt. **The readahead doubles only a consumed
request**: a miss right after the previous request, whose last block the
reader read to its end; a probe never does. **The readahead belongs to the
object**, keyed like the pool, so two openings continue one pass. The
signature stays in the headers, and the log prints addresses without their
query. A single read larger than the instance's memory is refused by the host
before allocating.

## Rejected

A presigned address (the signature in the query, `X-Amz-Expires` up to a
week): measured on 2026-09-05 against `eodata.dataspace.copernicus.eu` with
a key the keys manager lists as valid to 2029 — the storage answers HTTP 403
`InvalidAccessKeyId` to a presigned GET for every expiry tried (900 s to
604 800 s, with and without the empty-body hash), while the same key in the
headers answers 206; so the signature stays in the headers, and re-signing on
401/403 — an exchange between `network` and `data-provider` — remains the
way to a layer that outlives a quarter of an hour (roadmap). Guessing the
access pattern from the misses alone: no rule on misses tells a
chain of probes from a pass, while the reader knows. Parallel requests on
every miss: a pass already coalesces into one request, and only a named order
is worth several connections. Moving the block size or the pool ceiling: the
numbers below are reached without touching them.

## Consequences

On the fake host (`adapters/reads.rs`, 2026-09-05): a level-0 order of two
tiles of a tiled TIFF names exactly those two chunks ahead in one order; an
indexed JPEG 2000 names the probes of the tiles asked for in one order and the
pieces of each excerpt in another, all inside its own tile-parts. In the
in-memory network (`range.rs`): reading windows of half a block in sequence
requests 1, 2, 4, 8, 16 blocks, a chain of 16-byte probes at block starts
requests one block each time, one read of 12 blocks requests 1, 2, 4, 8; a
prefetch of four ranges over blocks 0, 1, 5 (present), 7, 8 makes two requests
and every read that follows is a hit. On the wire (2026-09-05, two runs of
`uitests/jp2remote.txt` on a cold tile cache, the T31TGK granule of
129.0 MiB): the coarsest level orders 121 probes — 112 runs, 134 blocks — and
by its end 68.0 MiB (52 %) have arrived in 115 requests of 605 KiB on average,
the same in both runs, against 109.6 MiB (84 %) in 64 requests of 1.71 MiB on
2026-09-03 under 0004; the next two levels come from the pool in 0.7 s and
1.8 s; the finer two levels order 26 single-block runs for their 60 tiles, the
other pieces already lying in the probe blocks; the resource closes at
81.0 MiB (62 %) and 141 requests against 115.6 MiB (89 %) and 76. The coarsest
level took 59 s in one run and 84 s in the other against 75 s once under 0004
— within the spread of a shared network, so bytes and requests are the
result, time is not: a probe of 64 KiB still buys a block of 512 KiB, and the
eight-fold overhead is the block, not the readahead — the next number to
spend is the block size under a named order. A row of COG tiles over the
network (a Sentinel-1 GRD COG of 382.1 MiB in the canvas, levels 6 to 1): 6
chunks under 12 tiles came in one request, 12 chunks under 40 tiles in three,
24.6 MiB (6 %) in 11 requests for the whole ladder; unmeasured under the old
rule, where adjacent chunks read in order coalesced by doubling and a chunk
apart from the last request cost a request of its own. The scenario promises
`delivered 70`. What the decision costs: an order is synchronous, so the first tile of a pointwise
level waits for the whole order; two unrelated readers of one object share one readahead state,
and a fingerprint read resets a pass's run to one block. What it obliges: a
source that knows its chunk offsets names them (NetCDF does not yet — its
file chunks are an index walk away); `IN_FLIGHT` connections are held only
for a named order; a layer older than a quarter of an hour has to be opened
anew.

## Amended 2026-09-05: a block of 64 KiB, and a two-phase JPEG 2000 order

The block is the size of a probe and of a fingerprint edge, `BLOCK` = 64 KiB,
so an order buys the blocks under it and a probe from an arbitrary offset
touches two; a read names its length, and the network fetches the blocks it
still needs in one request (`Readahead::plan`), so a long window costs no
more requests than before while a chain of small reads costs no more bytes
than it asks. The JPEG 2000 chunks order in two moves: the probes of every
tile asked for, then — the excerpts assembled from those probes — the file
pieces of all those tiles in one order, instead of one order per tile as the
excerpt opened. Measured on the wire (two runs each of `uitests/jp2remote.txt`
on a cold tile cache, the same T31TGK granule of 129.0 MiB): the coarsest
level's 121 probes bring 15.4 MiB (11 %) in 122 requests of 128 KiB, the
level passing in 26–30 s against 68.0 MiB (52 %) in 115 requests and 59 s
above; levels 4 and 3 order no pieces — their excerpts lie inside the probes —
and pass in 0.6 s and 1.8–2.1 s as before; level 2 orders the pieces of its
36 tiles as 109 runs and passes in 23–24 s against 17.8 s, since the probe
blocks used to hold most of those pieces; level 1 orders 24 runs of 130 blocks
and passes in 14–16 s against 21 s; the resource closes at 31.0 MiB (24 %) and
255 requests, 70–77 s after opening, against 81.0 MiB (62 %), 141 requests and
108 s. Bytes are the result; requests grew because a piece no longer lies in
the block its probe bought. Without the second move the same bytes cost
123–126 s: levels 2 and 1 took 91–92 s with 133 per-tile orders of one run
each, against 38–40 s with it and 39 s above. A read longer than `READAHEAD`
still goes in requests of `READAHEAD` each. The scenario promises `delivered 35`.
