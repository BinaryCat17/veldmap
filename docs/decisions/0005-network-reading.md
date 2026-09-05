# 0005 — Network reading: prefetch, readahead by consumption, a week's signature

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
signature of a storage object was issued in the request headers, which the
storage accepts for a quarter of an hour after `x-amz-date`, so a layer kept
on the globe longer than that lost its source at the next miss — 401/403 is
not retried, and nothing signed the object again.

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
object**, keyed like the pool, so two openings continue one pass. **The
object's address is presigned for `OBJECT_LIFETIME`**, a week — the limit of
SigV4 in a query — and listings keep their header signature; the log prints
addresses without the query. A single read larger than the instance's memory
is refused by the host before allocating.

## Rejected

Re-signing on 401/403 through an exchange between `network` and
`data-provider`: the read is a synchronous ABI call on the blocking pool, and
a bus round trip inside it buys what a presigned query gives by the standard.
Guessing the access pattern from the misses alone: no rule on misses tells a
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
and every read that follows is a hit. The wire numbers of the same day could
not be taken: the storage key was revoked that afternoon
(`InvalidAccessKeyId` on the header signature as well), so the share
delivered for the coarsest level of the T31TGK granule — 84 % under 0004 — and
the count of requests for a row of COG tiles are still to be measured, and
whether the storage accepts a presigned address is unverified against it.
`uitests/jp2remote.txt` keeps its promise at `delivered 95` until then. What
the decision costs: an order is synchronous, so the first tile of a pointwise
level waits for the whole order; the pieces of a JPEG 2000 excerpt are
ordered per tile, one run at a time, so the finer levels do not fill
`IN_FLIGHT`; two unrelated readers of one object share one readahead state,
and a fingerprint read resets a pass's run to one block. What it obliges: a
source that knows its chunk offsets names them (NetCDF does not yet — its
file chunks are an index walk away); `IN_FLIGHT` connections are held only
for a named order; a layer older than a week has to be opened anew.
