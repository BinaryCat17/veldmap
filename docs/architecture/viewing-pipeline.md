# Viewing pipeline

How a described raster becomes tiles on the screen, on the side of the one
who looks. The formats and what each costs to read are in
[imagery.md](imagery.md); the order of topics between the consumer, the tiler
and the tile cache is there too. This page is about the consumer.

## Two consumers, one mechanics

The pyramid has two consumers — the preview canvas (`image-view`) and the
overlay on the globe (`globe`) — and both perform the same dance: describe
the source, pick a level, ask the cache for the cells they lack, hand the
misses to the producer, put what arrives into video memory and evict what
has not been drawn for a while. They differ in exactly one thing: **which
cells are visible right now**. So the whole mechanics lives once, at the
pyramid, in the tiler's wrap crate (`veldmodules/image-tiler/wraps/rust/src/tiles.rs`,
included by both through the same `#[path]` as `pyramid.rs`): the store with
its budget, the choice of level, the bookkeeping of what was asked and of the
passes in flight, the validity of a description and the answer to "is work
still going on". Each consumer brings only its own two things — the level
under a screen pixel and the cells visible at that level: the canvas computes
them from its camera over the picture, the globe from the georeference and the
frame matrix. The messages to the cache and to the producer are sent by the
consumer itself: the topics are declared in its schema, and the wrap may not
publish on its behalf.

## What the consumer knows: the level table

A description (`Described`) gives the fingerprint, the size, and the **level
table** — one row per pyramid level: how the level is served (`Serve`:
pointwise, or by a pass from some level), the memory peak of that work in
bytes, and whether it fits. It is the same table the tiler itself reads to
choose how to produce a level ([imagery.md](imagery.md)); nothing about the
cost of showing is retold in scalars. The consumer reads it into `tiles::Meta`
and derives everything it decides from the rows:

- the number of levels is the number of rows;
- a level is **pointwise** when its row is (`Meta::pointwise`);
- the **detail limit** (`Meta::finest`) is the first row that fits — the
  finest level the source will ever serve; a source with no fitting row is
  not described at all;
- the **ladder** to a target (`Meta::ladder`) is read from the serving column,
  below.

`tiles::describe` refuses a description with no rows, no fingerprint, or a
row served by a pass from a finer level than its own: that is not a table but
a producer's slip, and reading further would be guessing.

## Target and ladder

`tiles::want` answers both the tile request and the frame assembly, and it
is one function so that the level ordered is the level drawn and a cell's
visibility is what it was asked by. The target level is the level under a
screen pixel, coarsened twice: to the detail limit — asking for a finer level
would be asking for a refusal — and to the **appetite cap**: the visible cells
must fit the consumer's share of the video memory budget
(`Store::cap_tiles`: half the budget divided among the pyramids being drawn,
and never above `MAX_QUERY_TILES` — the cache refuses a longer request, and the
consumer would only send the same list again; the constant is one file,
`tile.rs`, shared with the cache). The cap measures the visible cells, not the
level: a granule of 10 000² fits no budget as a whole level, yet a dozen cells
are visible at any zoom.

The pyramid is gathered top-down, not by a jump to the target: the top is one
tile of the smallest copy, arrives in a second and covers the whole picture,
while the target — a dozen tiles of a megabyte each — takes half a minute,
and all that time there would be nothing to show. Each step gathered becomes
the ancestor of the next (`Store::carrier`), so there are no holes between
steps. Which levels are steps is read from the table: a level coarser than the
target is a step when it does not cost more than the target itself — a
pointwise level costs its own tiles; a level of the same pass as the target
costs cache hits, because the first step pays for the rest (PNG; a stripped
TIFF beyond its window: one pass builds the whole pyramid); the top served by
its own pass (JPEG: the frame decoded straight to the top's scale) is the
smallest frame there is, and it lets the picture appear. Every other level
would cost another pass over the file — for JPEG each intermediate level is a
new decode, for a stripped TIFF a coarse step above a pointwise target is a
read of the whole file, dearer than the target — and such levels are skipped.
The step asked is the first from the top that is not complete (`rung`); a
single missing cell holds the step.

## Video memory

`Store` keeps the tiles of every pyramid shown, under one budget
(`DEFAULT_VRAM_BUDGET_MB`, one number for both consumers; each has its own
store). A cell is drawn by its exact tile, or by a piece of the nearest
ancestor that is present (`carrier`); one cell, one carrier, so nothing
overlaps and no depth buffer is needed. Eviction removes the least recently
touched, and three things touch a tile: arrival, being drawn, and being the
ancestor a drawn cell fell back to — the whole branch up, not just the
carrier, or the ancestors would age from the minute their step was complete,
and every pan would become a climb from the top again. Arrival counts as a
touch on purpose: "drawn" does not age, and a store full of tiles drawn long
ago would evict every newcomer, itself included, and never complete an order.
What protects the drawn is not the order of eviction but the appetite cap:
the visible plus the order take less than half the budget and are touched last.

## What was asked and what is being produced

`Fetch` is the bookkeeping of one requester of tiles (a canvas tab or a
raster of a layer): cells in flight are not asked twice; cells whose pass failed are
hopeless after one retry and are not asked again (`Fetch::produced`,
`stumbled`); the cells of the last order that actually went out are what
progress is measured by, not the cells visible in this frame, which move with
every frame of a gesture. Misses that come back from the cache while the
consumer's own pass is running are deferred until the pass ends
(`Fetch::missed`) — but only for a pointwise level: a pass over any other
source puts more into the cache than it hands over, and asking again is the
only way to see an uncovered edge before the pass ends. Someone else's pass is
never waited for: its end would not reach us.

`Passes` tracks the producer's passes, one per source, owned by whoever
started it. A pass is stale when a finer target has been ordered, or — only
for a pointwise level — when none of its cells is an ancestor of any visible
cell any more (`Pass::abandoned_by`); judged by cells, not by level, because a
coarser step is exactly what the pyramid is built from. Killing a stale pass
kills the tiler's work in flight, and for a pass that means reading the
source again, so a pass that fills the cache beyond its order is never
abandoned. The host settles a killed pass with the same empty terminal
reply it gives a fallen executor, so the one who killed it remembers the
correlation (`Passes::finish`).

**"Work is going on"** is three facts, not one (`tiles::working`): cells in
flight, a pass running on the source (anyone's — it will bring our cells too),
and a ladder not yet climbed. The last is not implied by the first two: a step
gathered entirely from the cache raises no pass and holds no cell in flight,
yet the next step follows at once, and calling the work finished at that
moment would say "loading is over" in the middle of loading.

## Progress

Progress is measured in steps of the ladder, and inside a step by the larger
of two shares (`tiles::inside`): how many cells of the last order are home,
and how many bytes of the source the running pass has read. The second is not
a luxury: a step can be one tile, and to hand it over a sequential source is
read whole — by cells nothing happens all that time. Bytes are honest only
where the source makes them so (`readable`): under pointwise reading the
tiler's counter is a high-water mark of the read head, and over the network it
jumps to the end of the file with the first window; a read that has reached
the end of the file says nothing about the cascade still flushing after it.
In both cases the denominator goes out as zero, and zero means "nothing to
measure by", not "zero bytes": the caption then speaks in steps and cells,
and it is the same rule for the bar and for the numbers beside it. Describing
is work with no share (`OverlayProgress.blank` on the globe): nothing is
ordered yet and no steps are counted, and over the network this is the longest
part; written as zero it would draw as nothing.

## Two kinds of refusal, and the detail limit

"Nothing to look at" — the resource did not open, the source was not
described: there is no frame, and only the reason can replace it. "The frame
is incomplete" — a pass failed, the cache refused: the coarser steps are
drawn, the picture is visible, and erasing it for one broken read would be a
lie; the reason goes into the status line and the first tile that lands
removes it (`ViewState.error` against `ViewState.trouble`, the same pair on the
globe). The **detail limit** is a third thing: a fact about the source, not
about an attempt. `Meta::capped` words it once for both consumers
("подробнее W×H из W₀×H₀ не будет", computed with `Meta::reachable` through
`pyramid::level_size`, so the caption, the log and the choice of rasters
cannot disagree by a pixel), and it is shown only when the consumer has
settled — a show is going on and nothing remains to gather — because the
consumer shows a complaint instead of progress, and a limit over an empty
canvas would read as "loading is over, this is its ceiling". On the globe's
layer row the verdict stands after the name of the file it is about
(`OverlayState.detailed`, named by the globe through `OverlayProgress.detailed`),
and only the words about that file follow the name (`detailed_trouble`); the
preview's limit comes after, on its own: a detailed raster has spares, and
"not finer than the preview" said of an unnamed file would read as a word
about the whole scene, while a preview's limit under the detailed file's name
would read as its own. The row cuts the name in the middle and the words from
the tail, each within its own budget, and hands the whole to the tooltip. A
large JPEG is
the plain case: its levels are each a pass from themselves, the native frame
does not fit `FULL_DECODE_BUDGET`, the first fitting row is the first level
whose decoded frame does, and the target is coarsened to it before the first
tile is asked — no refusal is learnt, and none is logged per frame.

## What each consumer brings

The canvas computes the target from its camera and the visible cells from the
rectangle of the picture in view; when the view has no place yet there is
nothing to want. The overlay computes the target from metres per screen pixel
and the visible cells by projecting nine points of a cell — corners, edge
midpoints, centre — through the georeference and the frame matrix, with a
cheap rejection first: the cell's bounding sphere against the frame's side
planes and the horizon, computed once per raster; a sphere that says "not
visible" is believed, one that says "visible" is not. Its appetite cap is
shared among all pyramids being drawn — a layer has up to two, preview and
detailed, and both are in the budget. A cell covered by an ancestor while its
own tile is on the way shimmers on both consumers, so that "what is loading"
is answered by the picture itself, not by the whole raster; the phase moves
only while there is something to shimmer, and it enters the frame's
fingerprint, so the animation switches itself on and off with the loading.
The mesh a raster is drawn with, the lift above the surface and the visibility
test in full are in [globe.md](globe.md). What the globe's list shows of all
this — the progress column, the globe icon, the layer line — is in
[screen.md](screen.md); the layers themselves, their order and visibility, in
[imagery.md](imagery.md) and [georeference.md](georeference.md).
