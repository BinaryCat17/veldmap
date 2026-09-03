# Diagnostics

Every run writes two logs to `runtime/logs/`: `host.log` is what the console
shows, `trace.log` is the full stream. Both are overwritten at start, so they
always hold the last run. Levels are set in `runtime/config/core.json`
(`log_filter` for the console and `host.log`, `trace_filter` for `trace.log`)
or by `RUST_LOG`, which overrides the first only; a module's target is
`veldmap::<module>::<subsystem>`, the module name added by the host, so
`RUST_LOG=veldmap::ui-service::handlers=trace` works.

## Performance counters

All four are `debug`, that is, `trace.log` only.

- `veldmap::ui-service::perf` — how fast frames parse, credited to whoever
  sent the markup: module name, then the average over the window and the
  extremes per frame. The author of the markup is one (`data-browser`), so
  this line is one; `image-view` has no counter of its own. An average below
  the screen's rate means the module does not keep up with the frame loop; an
  implausibly large maximum means it is parsing a backlog.
- `veldmap::globe::perf` — what the recount of the wanted costs: the walk over
  the cell grid that picks a level and the visible tiles. Reported per burst,
  not per clock, because the walk runs in bursts with a gesture; the line has
  the burst length, frames and walks with their reasons, cells checked and
  seen, then milliseconds per moving frame and the worst frame, and in between
  the warp-mesh rebuilds — how many, how many vertices, what one cost; the walk
  is cured by culling, a rebuild by mesh density, hence counted apart.
- `veldmap::host::network::perf` — reading a remote file: bytes delivered, in
  how many requests, and what the block pool did. Reported every few megabytes
  and on close, keyed by the resource number that the opening line and every
  reader use, so the logs stitch by it.
- `veldmap::image-tiler::perf` — the breakdown of describing a raster into
  fingerprint, parse and coordinate file, needed because describing has no
  progress at all and the application is silent for all of its seconds; under
  the same target, one line per pass, one per level read directly from a
  TIFF, and the codestream layout of a JP2 together with how its tiles will
  be read — by an excerpt over the TLM index, or by the decoder's own walk
  over the SOT headers, and why; whether the excerpt can stop at a
  resolution is decided per tile, and the line only says if the progression
  allows it and if the first tile-part carries a PLT.

## Three rules of reading

1. **The first network line of a resource is "reading started", not a
   total**: readahead starts from one block, so it always says one request.
   The request length further on is the readahead at work; a length of one
   block means no readahead at all. Delivered is not what went over the wire —
   a failed range is asked again up to `ATTEMPTS` times, and only the last
   try counts. Pool hits count reads, not blocks: the reader's window is
   smaller than a block, and one block is handed out twice. A second opening
   of the same object hits from the start, and that is not a counting fault:
   blocks belong to the object, not to the opening. More blocks fetched than
   the file has means the pool evicted and refetched; the pool is per process,
   and the eviction count in the line is global too.
2. **Seconds at the fingerprint are the network, and the parse is paid by
   them**: the fingerprint pulls the head and the tail of the file, the head is
   what the decoder needs too, and after the fingerprint it sits in the host's
   block pool — so "fingerprint 3 s, parse 0 s" does not mean the parse is
   cheap. A failed describe leaves no breakdown; its own line says what the
   refusal cost.
3. **In the globe line, seen over checked says how much of the grid is in the
   frame and nothing about culling**: the denominator is all cells of the
   level, the numerator those that passed the exact test, and neither depends
   on culling. Culling shows only in "per frame": fractions of a millisecond
   mean it cuts, whole units mean it stopped. "Worst" does not serve here — it
   includes a rebuild, which does not depend on culling. No line at rest is
   not a fault: there was no burst.

## Where things live

Downloads go to `runtime/data/dem/source`, a constant `DATA_DIR` in
`veldmodules/data-library/src/storage.rs`, known to `data-library` alone.
Preview tiles are `runtime/data/tiles/<fingerprint>/`, owned by `tile-cache`;
the cache is derived, and a doubtful one can be removed as a whole — pyramids
are rebuilt from the sources at the next show. The window layout is
`runtime/state/data-browser.json`, owned by `data-browser`, written on every
tab action and read at start; deleting it is safe — the window opens on the
tab from `runtime/config/`, whose `initial_view` counts on the first launch
only. All three paths are ignored by git.

A log line saying the glyph atlas ran out means more distinct glyphs were on
screen than the atlas holds (sizes and faces count separately): part of that
frame's labels is not drawn, and the next frame starts the atlas over — it
never swaps glyphs. Repeating every frame, the line means one frame does not
fit, and only `ATLAS_SIDE` in `veldmodules/ui-service/src/renderer.rs` cures it.
