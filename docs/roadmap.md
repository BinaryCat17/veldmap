# VeldMap: Cleanup Roadmap

A work plan for fall 2026, drawn up on September 2, 2026 after a systematic
review: 30 readers over subsystems, documents, and tests, 14 skeptics
refuting their findings against the code, four plans from different angles
(simplicity, documentation, tests, formats and JPEG 2000), a judge for each,
and a synthesis. The `[C]` mark means a finding confirmed by a skeptic
reading the code, `[P]` — confirmed with a scale adjustment; `file:line`
without a mark was read during synthesis. Line numbers are allowed here: this
is a dated plan, it is rewritten as steps are completed and deleted when done;
the "no line numbers" rule applies to the docs/ architecture pages, not to it.

How to use: the "Work Order" section is the single queue; a step is taken as a
whole together with its check and its docs/ pages. The "What Not to Do"
section is the boundary. The first three sections explain why the queue looks
the way it does.

## Architecture Verdict

**The skeleton favors simplicity, and it is not to be touched.** One
`schema.yaml` → validator → stubs and the FLOW table; there are no string
topics in modules [C, SDK]. One read primitive `RangeSource` +
`read(offset, size)` used by both fs and network — to the reader, disk and
network are indistinguishable [C, host core]. Wrap crates with `#[path]` give
one implementation of pyramid/tiles/geodesy to the producer and both consumers
[C, scattered paths]. Exchange accounting with a terminal reply on behalf of a
crashed or killed executor (dispatcher.rs:292-320; tasks.rs::begin for every
request [critic §2]) is the right foundation for everything that runs long.

**The overhead works against simplicity — for four reasons, and all four are
of one kind: a rule is written in words where it should be derived by code.**

1. **The cost of reading has leaked into the protocol.** `Described.reach`
   (4 variants), `windowed`, `finest` are derived from `Kind` in
   `Info::reach()` (adapters/mod.rs:203-231), while `produce` parses the same
   arms with its own `match` (mod.rs:316-352); the agreement is held by an
   "edit them together" comment in three places, and `reach()` is not tested
   [P, tiler core]. `finest` is filled by a single `jp2::finest`
   (jp2.rs:254-262), and it already diverges from produce by a second refusal
   [C].
2. **Memory is assigned, not computed.** 12 constants in 6 files plus
   `tile-cache.json` and the host `POOL_LIMIT` [critic §2]; only one
   `netcdf::affordable` computes it from the terms; `budget.rs:8-11` itself
   calls the rest unfinished work; two numbers lie — "666 MiB" (module.rs:61)
   and "sixteen times" when it fires at 8× (tiff.rs:38-42) [C, numbers].
3. **One concept, several representations.** The tile-reply pipeline in
   image-view and globe is a shared skeleton with different effects and caller
   keys [P high: 7 of 25 comments in the named ranges are verbatim, not "the
   same code"]. Binding — four kinds of `Binding` (globe/overlay.rs:814-835)
   against "There are three bindings" in the file header [critic §2]. Soft
   failure — five "second fields" in five contracts and two ways to learn the
   host-acknowledged reply [P, event map]. The "library or provider" fork is
   written four times [C, data-browser]. The write ritual through fs — three
   times with identical comments [C].
4. **The screen has become the integration core.** Seven `State` collections
   keyed by product, tied by rules in comments; two devices routing replies to
   one topic [C, data-browser]; quad/extents, role translation, and a copy of
   UtmFrame are decided in data-browser [P].

Invariants are held by the wrong means: BLOCK/WINDOW — in five places as text
[C], 120 of the wheel, `.part`, log levels, FLOW ordering, `cap_tiles ≤
MAX_QUERY_TILES` — only by comments [C]; only `INSTANCE` ↔ the host limit and
`ZOOM_PER_CLICK × ZOOM_PER_STEP` are reconciled by a test.

Reply to the owner: there is nothing to rewrite; take the layers off the
skeleton. The measure is the number of "comment only" lines in the
invariants.md ledger (see "Documentation"): it must decrease with every step,
otherwise the step is not about simplicity.

**Dispute 1 — what the wire carries.** "Simplicity" puts
`Serve{Tiles|Pass{from}}` per level instead of three scalars; "formats" keep
the level table inside the producer and leave the wire alone. Decision: **the
level table goes on the wire — yes, but with a cost column and as the last
step of the tiler rebuild, not the first.** Arguments. (a) The "simplicity"
judge is right: `Serve` without cost gives a multi-tile JP2 "all Tiles →
Exact" and a six-step ladder where the coarse steps cost almost the whole
file; a level row must carry three things — how it is served, how many bytes
it costs, whether it fits in memory. (b) The "formats" judge is right about
something else: if the table lives only with the producer, the consumer keeps
the Reach(4)×pointwise(2) matrix (tiles.rs:698-711), and the table becomes a
fourth description of the same cost. (c) But changing the wire before the JP2
table becomes real (after measuring the granule and openjp2) means designing
on guesswork: `Serve::Pass{from: level}` for JP2 under hayro is a promise the
adapter does not keep (second refusal, jp2.rs:305-313 [C]); while
`Meta{reach, finest, windowed}` is assembled by hand in the tests of both
consumers — those are fixed once, not twice. Order: the table inside the
producer with the three scalars derived and a table test (step 7) → JP2 into
it (steps 8–9) → the table onto the wire, three scalars and `Reach` removed
(step 10).

**Dispute 2 — `finest`.** "Simplicity" removes it entirely (step 8); its
judge and "formats" — derive it from the memory count. Decision: **the
mechanism stays, only the manual `jp2::finest` is removed.** The field exists
so that the consumer does not request a known refusal; without it a large JPEG
relearns through a refusal on every cell — exactly the cycle that already
exists for JPEG larger than `FULL_DECODE_BUDGET` (jpeg.rs:38-43 [C low]).
After step 7 the detail limit is the "fits" column of the level table,
computed by one memory count for all paths that decode the whole frame (JPEG,
single-tile JP2); for pointwise paths it equals zero on its own. Seven tests
of the golden string "more detailed … will not be" merge into one at
`Meta::capped` (tiles.rs:131-141); they are not deleted.

**Dispute 6 — failure.** Decision: **a helper — yes, contracts — no.**
`tiles::undelivered` (tiles.rs:1028-1030: empty error ∧ `answered_by_host`)
moves into the SDK and is used in download.rs instead of guessing by empty
url, in tile-cache and at the data-provider relay [P, event map]. But
`ImageryResponse.reason` (types.proto:257-262) and `LocateResponse.answered`
(:281-289) encode a different question — "whether to re-ask on a non-empty
error" — and are not replaced by the helper; renaming `reason → trouble`
would fold two rules under one word. Two kinds of "second field" remain, and
both are named on the contracts page: `trouble` — the caption next to the
display, `answered/reason` — the re-ask policy.

## Documentation

**Target structure** (English; source in parentheses):

- `README.md` (~40 lines) — what this is, requirements in a table, two
  commands, Copernicus keys via `.env.example`, links into docs/.
- `CLAUDE.md` (~40) — process only: build.py the single command, "what not to
  do" without gigabytes and seconds, style — a link to ADR 0001.
- `docs/glossary.md` — ~15 terms, each with one English name (Sentence,
  caller, host-acknowledged reply, lease, carrier, fingerprint, step/ladder,
  binding); written before the first page.
- `docs/architecture/overview.md` — repository map, services, the schema as
  the source of truth; the topic list lives only in schema.yaml.
- `docs/architecture/bus-and-schema.md` — an actor per subscriber,
  correlation, snapshot/targeted/cancellable, exchange accounting and the
  terminal reply (once), what the validator holds.
- `docs/architecture/resources.md` — ResourceHandle, RangeSource, the reader
  window and the pool block by constant names (`WINDOW` resource.rs:214,
  `BLOCK` range.rs:28), lease, ownership transfer, block pool and readahead.
- `docs/architecture/tasks.md` — an operation in flight, kill/cancel, a trap =
  lost state; **do not carry over** "image-tiler is stateless" (README:507,
  :820) — the memo of the last two parses exists (module.rs, `MEMO_SLOTS`).
- `docs/architecture/window-and-render.md` — the runner, frame loop,
  Viewport, the data-browser Elm loop. `modules.md` — anatomy of a module and
  its wrap crate, named for what it does: a client crate with two purposes —
  a convenient API where codegen falls short (the ui-service builders), and
  shared code of producer and consumers through `#[path]` (pyramid, tiles,
  geodesy, tile); there is no third purpose, and the consumer always
  publishes.
- `docs/architecture/screen.md` — home for the screen rules (panes, singleton
  tabs, highlighting, persist, name squeezing, dropping columns):
  README 584–800/829–900 ↔ state/mod.rs verbatim [P], and window-and-render
  will not fit them in 50 lines.
- `docs/architecture/imagery.md` — product → scene → parts, a topic diagram,
  ONE table "format → how it is read by levels → what is partial → ceiling",
  cache fingerprint. Written "as is" after `enum Reach` and `Info::reach()`,
  not after README:1286-1291: a striped TIFF is Windowed (mod.rs:215-221),
  not a "full pass".
- `docs/architecture/viewing-pipeline.md` — consumer-side pyramid mechanics
  (want/ladder/Store/Fetch/Passes): home for README 1217–1276, otherwise
  after deleting README it lives only in 926 lines of tiles.rs comments.
- `docs/architecture/georeference.md` — precedence by `enum Binding` (four
  kinds), who interprets EPSG, datum as in the code. **MSK-61 — only as a
  branch verified by a test**: projection.rs:459-470 — codes 63361xx cannot
  arrive with a raster, the geokey is sixteen-bit.
- `docs/architecture/download.md` — the pipeline, `.part`, sidecar, siblings
  once.
- `docs/architecture/invariants.md` — a ledger "invariant → holder" (type /
  validator / test by name / shared constant / pytest pair / comment only);
  seeding: BLOCK=2×WINDOW, 120, `.part`, cap_tiles, reach↔produce,
  showing ∩ overlays = ∅, ripple ×6 (Rust ↔ WGSL), log levels, FLOW; the
  DECODE_REV row — about "the codec is not pinned by the key despite proto"
  (one constant, fingerprint.rs:31 [critic §3]).
- `docs/limitations.md` — the single place to declare limitations: a phrase,
  a reason in present tense, what will lift it; no shadow (renderer.rs draws
  [C]) and no "datums are not converted" (projection.rs converts 284xx [C]).
- `docs/operations/{build,diagnostics,ui-tests,configuration}.md` —
  diagnostics ≤ 80 lines: perf strings one line each and three reading rules;
  the rest — the debug-string guide from CLAUDE.md — is deleted.
- `docs/decisions/README.md` — format: context / decision / rejected /
  consequences, 10–20 lines, statuses open/proposed/accepted/superseded;
  "Superseded by" is the only allowed history.

**Dispute 5 — ADR numbering and structure.** Three plans produced three
numbering schemes. Decision: **numbers in order of writing, with no thematic
reserve**; the first three are fixed: `0001-documentation-and-comments` (rules
— before any page), `0002-jpeg2000-decoder` (replaces docs/jp2-openjpeg.md,
status proposed, subordinate to 0003), `0003-raster-reading-model` (the level
table and the two production paths; open → accepted at step 7). Further — by
steps that change the shape (memory count, pipeline into wrap, network). Work
plans and "tree state" do not live in docs/.

**Rule for comments** (ADR 0001): `//!` ≤ 10 lines — what the file owns, which
invariant it holds, where that is checked, a link to the page; `///` — the
contract of an item (inputs, units, what it returns); `//` — non-obvious
mechanics. Forbidden: a counterfactual essay longer than a phrase,
self-defense ("and this is not X"), line numbers, "the same as the
neighbor's" instead of a shared constant. The same for `///` in `.proto` —
that is the wire contract, and a lie there is confirmed (Ring [C]). No
separate pass over the 176 long blocks: they live in
overlay.rs/tiles.rs/netcdf.rs [P], which the reading model will rewrite;
compress on touch. Verbatim duplicates are folded at once: "a refusal kills
the pass" ×7 → once at Cascade, siblings ×5 → types.proto, the fs ritual ×3
→ an SDK helper.

**Numbers.** A number lives in a named constant or it is an external fact with
a source (WGS84, an EPSG code); prose names the constant, not the value; a
measurement goes only in a decision record with a date and a method. A number
is never fixed with a number ("36" → "40"): into a constant or delete. "Up to
three times" is not a lie but `ATTEMPTS = 3` (range.rs:49): name the constant.
A "cannot" without a test ("the decoder cannot be put in a test", "striped is
impossible") is lifted by a test or by deletion.

**English.** Nothing is translated in place: a distilled list of rules → an
English page with a line budget → deletion of the Russian source in the same
commit; the 129 "rather than" and 113 "would" of the README must not be
translated. The language of comments is the owner's decision before the first
page (otherwise the glossary is written twice); the recommendation: comments
stay Russian and are translated only when a file is rewritten under the rule,
there are no translation commits; a mixed language for months is a price one
developer can bear.

**Order:** rules + test_docs + ADR 0001/0002 + imagery.md "as is" —
immediately (2–3 days, they stop the rot); platform and operations pages — one
at a time, in the commit that already touches the subsystem (SDK/abi →
resources; dispatcher/range → bus, tasks; build.py/uitests → operations);
screen.md — together with the `open_resource` helper; georeference — after the
JP2 binding fix; the imagery.md table and invariants.md are updated in the
commits of steps 7 and 10. README loses sections in the same commits; a
separate "README rewrite" does not exist.

`test_docs.py` — three rules about links, with a criterion for "what counts as
a path": from the root, without `*` and `<>`, not under .gitignore; no
`file.rs:123` in docs/; no paths outside the repository. Scope — docs/ and
CLAUDE.md right away, README once it is ≤ 50 lines. Do not introduce rules
about "every adapter is named" or identifier checking by regexp.

## Tests

The tests are not redundant and barely artificial: ~10 tautologies, ~6
rewritten formulas out of 794 [C, both test readers]. The weakness is
direction: no test checks the path "bytes → describe → produce → tile" or the
count of bytes read; dispatcher.rs — 0 tests; assembling bytes from blocks and
Content-Range in range.rs — 0; catalog.rs — 0 against five asynchronous rules
[C/P]. The reason is systemic: the native SDK stubs return zero
(sdk/abi.rs:55-90), and everything that reads a resource or publishes is not
observable natively.

**Dispute 4 — a fake host versus a counting source.** Decision: **one
mechanism — a fake host in the native branch of `veldsdk::abi`**, and a
"counting source" is its read log, visible through the same `ResourceReader`.
The three conditions of the "tests" judge are mandatory: (a) the reply encoding
(len≪32|ptr, byte tag, buffer via veld_alloc) is shared with the host in one
file through `#[path]`, not written a second time — otherwise the fake proves
the SDK agrees with itself, not with the host [P, SDK: wire constants are not
pinned]; (b) the event context — `static Mutex` (abi.rs:311-320) — and the
fake live in one place: both thread_local in the native branch; (c) the log is
windows of 256 KiB (resource.rs:214), not bytes: fixtures spanning several
windows, assertions "≤ the number of windows touched", and `Metered` does not
qualify — it keeps the far position, not the sum (mod.rs:429-433).

**Remove / fold.** First fix the gauges: put `#[test]` back on overlay.rs:3848
and remove the duplicate :3803 [C]; `hash_of` into production instead of the
test copy of `of_slices` (fingerprint.rs:100-107) [C]; the hard-coded `-p`
list in build.py:468-471 → `cargo test --workspace` (five util tests do not
run) [C]; catch orphan and duplicate `#[test]` not by regexp but by showing
rustc warnings on the test target — the compiler already sees them, build.py
hides them. Fold the number fixations into rules — Lease ×7, fit() ×6,
group() ×8, belt ×6, voting ×6, the golden limit string ×7, perf.rs 10 →
3–4, budget.rs:79 duplicate affordable, five rewritten formulas — **only
after** moving the mutation harness into the tree: 9 of the 16 scripts write a
backup into a dead directory, and a "before/after measurement" is impossible
without it; do not promise "−60…−70" before the measurement. Remove
`budget_refuses_native_tci_but_takes_overview` together with DECODE_BUDGET;
keep the `Codestream` tests while its output decides something (step 3).
Regression pins with history in the doc — rephrase in present tense, do not
behead them. Do not touch the golden lists of test_project.py [P low].

**Cross-wire agreements** — order of preference: a shared constant (wrap,
`#[path]`) → a row in the single pytest-pair table → a comment, which counts
as a debt and is visible in invariants.md. Specifically: `WHEEL_NOTCH` as a
constant in the runner instead of the `* 120.0` literal ↔ `RAW_WHEEL_NOTCH`
[C]; `PART_SUFFIX` from network/download.rs:53 ↔ data-library [C];
`ZOOM_PER_CLICK × ZOOM_PER_STEP` — into globe-wrap, where data-browser already
goes through `#[path]`; delete test_wheel_step.py entirely and the two
in-crate tests of test_tiler_budgets.py [C, buildgen]; log levels — a shared
file inside the veldcore workspace, not a table (both sides are `match`
branches); FLOW ordering — in test_project.py; `cap_tiles ≤ MAX_QUERY_TILES`
— one constant through the already existing wrap inclusion in tile-cache
(tile-cache/src/tile.rs header), not by removing a refusal; ripple ×6 — the
WGSL half cannot import the constant, it stays a "comment" row in the ledger.
BLOCK = 2×WINDOW is a setting, not an invariant: it does not go into tests and
is removed from prose.

**Add by cost of error, not by coverage:** tiler bytes on fixtures from
`tiff::encoder` + `DirectoryEncoder` (a tiled IFD — 30–40 lines, not 120):
describe ≤ the windows of the head; one tile ⊆ the windows of its chunks; a
pass reads the file once; direct == pass on a fixture with copies exactly
halved — a test of the driver geometry, not a format invariant (GDAL copies
will not match this way); dispatcher — accounting for
opened/closed/acknowledged on behalf of a missing subscriber, names from FLOW,
no tokio runtime needed (publish_from is synchronous); range.rs — `read_at`
across block boundaries, a short tail, 200 instead of 206, Content-Range as a
pure function — via a `fetch(from, to)` closure, a named exception to "no
traits for tests": the host has no SDK fake; catalog.rs ingest on hand-made
FsListResponse; `abandon` → `fake::leaked()` empty. A new seam is accepted
with a mutation named in the test doc in one line.

**uitests:** a scenario targets a user action and asserts a condition, not
seconds. Add `download.txt` (also needed for granule mining), `layers.txt`,
`jp2canvas.txt` (first `wait` for a size line proving level 0, then
`absent text:will not be`); `remote.txt` — assert the delivered share via
`network::perf` by run-uitests.py itself, the way `wgpu:` and "trap caught"
are already searched for; ragged.txt — `#n` instead of a pause; the log path
from services.json. Mutations: one harness `buildgen/mutate.py` +
`mutations/<crate>.txt`, by hand. ui-service: lift the three "cannot be
started" claims with text now, the wrap with tests — as desired.

## Formats and JP2

**What reads "only what is needed" today.** A tiled TIFF with copies — at all
levels (`produce_direct`); a striped TIFF — detailed levels by window, coarse
ones by pass (`Reach::Windowed`, mod.rs:215-221 — that is three paths, not
"one format out of six"). JP2 — the whole file for every level and twice for
display (`read_to_end` jp2.rs:284-286; the Coarser ladder = apex and target)
[C high]; a 10980² TCI is no more detailed than level 2. NetCDF — metadata on
demand, but the selected variable entirely in f32 already at describe and to
the end of the pass (netcdf.rs:292-306) [C high]; meanwhile `describe` is
declared "fast" by the schema [C]. PNG/JPEG/full — an honest full pass. The
"strip/variable" axis is missing from the contract [C high]. In the network:
one request in flight per reader and readahead only on a strictly sequential
miss (range.rs:336-350 `plan`), readahead on open rather than on the object
(range.rs:110-111) [C]; the SigV4 signature is issued at open and is not
re-asked after 401/403 (range.rs:597) — for a layer living a whole session
this is a blocker of the "any size over the network" goal, and no plan except
the "formats" judge named it. `WIRE_PLANE` 96 MiB suggests downloading —
against the goal [C]. "All formats" for this household means three families:
Sentinel-2 JP2, GeoTIFF/COG, NetCDF-4; the order follows them.

**Dispute 3 — NetCDF.** "Simplicity" leaves it `Pass{0}` pointing at "the
hdf5-pure blocker"; "formats" and the "simplicity" judge — windowed reading
exists. Decision: **NetCDF is an implementer of the shared model with a row
window; degradations are rows of the level table, not an exception to the
model.** `read_raw_rows`/`read_*_rows` along the first axis exist (hdf5-pure
0.42 reader.rs:5420-5521); netcdf.rs:22-30 itself says OLCI lies in three
strips along the first axis and the crate cuts along it — while "the cascade
builds the pyramid across the full width" is a lie, the cascade accepts strips
[C]. The blockers are granule properties: the unit leading axis of S5P
([1,y,x]) and the single-chunk SYNERGY dataset (row window = the whole plane)
— the table honestly writes "the level costs the plane". Fork hdf5-pure only
when stuck without it. This is the third format in order: the main catalogue
format is JP2.

**docs/jp2-openjpeg.md.** *Important and correct* (three readers agree): the
quarter's diagnosis — `estimate = file_len + w·h·24` against `DECODE_BUDGET`
gives finest = 2 for 10980² [C]; hayro can do neither tile nor region and
returns only f32 [C decisive] — the switch to openjp2 is needed by any path;
describe does not decode, `codestream()` parses the main header and is pinned
by tests [C]; an error handler is mandatory, panic = trap; a new uitest with a
TCI in the canvas is needed either way (no scenario opens a JP2 — showsel
opens an S1 COG). *Secondary:* caveats about the crate (no_std/libc,
`Stream::new_reader`), the argument about codec reuse, the MB/MiB mix —
notes for the implementer at the unsafe island, not docs/. *Wrong or risky:*
the model "one pass over the file per step, Reach stays Coarser, wants are
ignored" — bytes per level equal the file, goal 1 does not move by the
document's own admission; a copy of `tiff::produce_pass` in jp2.rs and a fifth
memory ceiling; `finest = 0` traps a single-tile codestream (buffer w·h·3·4 ≈
1.45 GB); tile memory understated fourfold (tcd — 4 B/sample);
`cascade::bytes(10980²)` = 56 MB, not 45 (total ≈104); 1.91× explained by a
buffer top-up whose size is set by the caller; the probe measured hand-made
codestreams, not a granule; `(prec+7)/8`; `data_size` is filled only when Some
is set beforehand; the reference to test_tiler_budgets.py is false; "windowed
is not covered" — it is covered (tiff.rs:2313); "not committed" was stale at
the moment of the commit; config.yaml:33-37 justifies openjp2 by regions,
jp2.rs:12-20 promises "a region in its own phase", jp2.rs:497-499 — "the full
read method is decided there", while `codestream()` is called for the sake of
a log line [C]. TLM/PLT openjp2 reads and throws away — the traversal of all
SOT is a crate limitation, not a format one [C]; whether granules have TLM is
not measured (there are no .jp2 on disk).

*Whether to do it now:* **not as a strategy.** For now — truth in the texts
and ADR 0002 instead of the document (step 1) and a measurement of a real
granule with our own parser (step 3). The decoder — inside the shared chunk
grid driver after it is extracted from TIFF (step 8): JP2 gets a pointwise
path and per-tile memory with one piece of code, not its own loop. The choice
of path to the tile is settled by a clue from the digest: after `decode` of a
multi-tile stream the EOC state closes a repeated `set_decode_area`,
`get_decoded_tile` reuses the codec and breaks the traversal right after the
needed tile — take it, do not drag the fork into the code. Pointwise reading
over the network (step 9) — by the measurement result, with three outcomes
named out loud: no TLM (SOT traversal, a block per tile-part); TLM present,
tile-parts not by resolution (tile granularity, ~1 MB for a TCI); TLM with
division (level granularity). Degenerate grids — as a table rule before code:
chunk ≪ tile (PVI 8×8) → pass; one tile on the canvas → limit by tile memory
on the factor. Along the way in jp2.rs: one box walker instead of three, fill
`binding_trouble`, remove the `easting_first` whitelist (jp2.rs:92-94) — EPSG
is interpreted by the globe's projection.rs [C].

**Foundation for the world map (goal 2)** — requirements only, not work:
honest level tables at every source, `Placement`/grid as one binding at the
input with no separate UtmFrame path, a shared `radiometry::Mapping` (a
per-file stretch will fray at the seams), the visible ceiling across all
sources together, not evenly per pyramid (`cap_tiles`, tiles.rs:244-260 [C]).
Do not build a global tile scheme in the tiler.

## Work Order

One order for the four angles. S — up to two days, M — up to a week, L — two
to three weeks with AI; the sum is ≈ 13–15 weeks of sequential work by one
person, which matches the judges' estimates (3–4 months). The tree is green
and useful after every step; documentation rides in the commits of steps, not
in a separate lane.

**Dispute 7 — schedule and order.** Decision: the cheap truth and measurement
come first; the test support comes before the rebuild, because it is also the
equality criterion "before/after"; a visible result for Sentinel-2 — not
before the safety net, but before NetCDF and the network; docs pages — one at
a time in foreign commits, not a single week of documentation before goal 1.

1. **[S] Truth in the texts and rules** — done on September 2, 2026: false
   numbers and promises removed, `docs/decisions/0001-documentation-and-comments.md`,
   `docs/decisions/0002-jpeg2000-decoder.md`, `docs/architecture/imagery.md`,
   `docs/glossary.md`, `buildgen/tests/test_docs.py`; the openjp2 dependency
   removed before step 8.
2. **[S] Gauges and the host defect** — done on September 2, 2026: overlay.rs
   tests fixed, `hash_of` in production, `cargo test --workspace` with a rustc
   warning summary, `buildgen/tests/test_wire_pairs.py` (WHEEL_NOTCH,
   PART_SUFFIX, INSTANCE), the wheel step in `globe/src/wheel.rs` through the
   wrap, log levels via the shared file `sdk/rust/src/abi/log_level.rs`, FLOW
   ordering in test_project, an actor without an instance raises it again and
   negotiates the exchanges, `buildgen/mutate.py` with tiler mutations,
   `docs/operations/{build,ui-tests,diagnostics}.md`, CLAUDE.md — process.
3. **[S] Granule measurement** — done on September 2, 2026: `jp2::codestream`
   reads TLM, the PLT of the first tile-part, the progression and COD layers;
   a description line in trace.log; the measurement of three granule files is
   recorded in ADR 0002 (TLM one tile per tile-part, PLT present, LRCP with a
   single layer — tile granularity, and inside the tile by PLT and level);
   `uitests/fixtures/s2-tci.txt` downloads the TCI once,
   `uitests/jp2canvas.txt` opens it in the canvas, waits for tiles, zooms to
   the detail limit; the run-uitests run limit grows with the scenario
   `timeout`, and a stopped run takes the window with it.
4. **[M] Fake host in the SDK** — done on September 2, 2026:
   `veldsdk::fake::{install, mount, reads, published, logged, leaked, owner,
   readers}` under `not(wasm32)`; the native ABI stubs answer from it, without
   `install` — the old zeros; the reply encoding — the file shared with the
   host `abi/wire.rs` (via `#[path]`), replies lie in an arena of "linear
   memory"; the event context is `thread_local` natively; reader tests
   (windows exactly [0, len), a window starts at the read position),
   ownership, publishing, logging; mutations `buildgen/mutations/veldsdk.txt`;
   `docs/architecture/resources.md` instead of the README section.
5. **[M] Metric of goal 1 and host seams as tests** — done on September 3,
   2026: TIFF fixtures from the crate encoder (tiled with copies — by hand
   through `DirectoryEncoder`, striped — `ImageEncoder`) in
   `adapters/reads.rs`; four properties on the window log (describe — only the
   head and the IFD neighborhood; a tile — its own chunk; a pass — the file
   once; direct == pass on copies exactly halved) and the table
   `reach() ↔ the produce arm`; dispatcher — opened/closed/acknowledged on
   behalf of a missing one, addressed publishing, the right to kill; range.rs
   — the network trip via a `fetch` closure, statuses and `Content-Range` as
   pure functions, `read_at` across boundaries, tail, readahead, hits,
   retries; the library catalogue on a hand-made listing; `abandon` of an
   overlay → `leaked()` empty. Mutations of all four crates in
   `buildgen/mutations/`. Docs: bus-and-schema.md, tasks.md; README without
   three sections.
6. **[S] Three glue helpers** — done on September 3, 2026:
   `veldsdk::reply::undelivered` (from tiles.rs; `answered_by_host` stopped
   being a public door), with the same `accept` naming the acknowledged reply
   — that is tile-cache with the data-provider/data-library relay — while
   download.rs calls it instead of `url.is_empty()`;
   `veldsdk::resource::{region_of, read_whole}` instead of three write rituals
   and three whole-file reads (persist got `LAYOUT_CAP`); assembling
   `ResourceOpened` — the shared file `resource/opened.rs`, host/util includes
   it through `#[path]`; data-browser `handlers::open_resource(identifier,
   local, &correlation)` in five places, wait accounting at the caller — a
   helper without `state`, because the logs of the preview and the overlay
   differ. Docs: screen.md, download.md, README without two sections;
   mutations of veldsdk.
7. **[L] Shared chunk grid driver and one memory count** — done on
   September 3, 2026 in two commits. The driver `adapters/grid.rs`: `Grid`
   (base chunk, copies, sample depth), copy choice and the window rule
   (`footprint`), region geometry, chunk cache, the `Chunked` trait and the
   `direct`/`pass` loops; from TIFF only the chunk remains (`tiff::Chunks`).
   The level table `adapters/table.rs`: `Serve {Pointwise, Pass{from}}`,
   `Level {serve, pixels, peak, fits}`; `reach`/`windowed`/`finest` and the
   `produce` arm are derived from it, "edit them together" is gone; the
   `Info.finest` field and the manual `jp2::finest` are removed, JPEG `finest`
   — from the count. `budget::Peak` (terms by name, `admit()` against
   `free()`); `CHUNK_CACHE = free()/8`, `REGION_CAP` — half the cache in
   pixels, BAND_CAP removed (pass and chunk are measured by the sum),
   `MIN_CHUNK_PIXELS` — the rule for degenerate grids; `frame_fits` shared by
   the table and the whole-frame paths; `produce` checks the peak against the
   footprint of the neighbor's light memo; describe refuses if no level fits.
   Behavior shifts: cache 128→116 MiB and REGION_CAP 16.8→15.2 Mpx (a tiled
   file without copies with a side around 4096 loses its window at the apex),
   a TIFF without a window answers `Pyramid`, not `Windowed`, `Exact` no
   longer requires `random_access`; the chunk count now includes raw samples
   (u16- and f32-striped files that used to slip past BAND_CAP silently are
   now counted honestly). ADR 0003 accepted; imagery.md, invariants.md
   (created), glossary. Measuring windows on IFD in real COGs is not done — it
   stays in "Next". Check: 51 tiff.rs tests and the reads.rs properties with
   no expectation edits; grep "edit them together" is empty; the table fits on
   real sizes (table.rs); tiler mutations 10/10; uitests preview/shown/remote.
8. **[L] JP2 on openjp2 inside the driver** — done on September 3, 2026.
   One unsafe island — `adapters/codec.rs`: a stream over the resource reader
   through C callbacks, an error handler, the codec is dropped after a
   refusal, strict; the chunk — a codestream tile on the factor
   (`get_decoded_tile`), copies — resolution levels, while a tile splits in
   two; `Kind::Jp2(jp2::Layout)`, table rows as for TIFF, memory — the step 7
   count with `SAMPLE_BYTES` per sample; a canvas origin not at zero — a
   describe refusal; zero is transparent only for a file with GML; a stretch
   wider than a byte — by the sample of the smallest copy. hayro,
   `DECODE_BUDGET`, `key_margins`, `ladder_level` removed. The fixture — the
   openjp2 encoder in the island tests; read tests: describe — head only, a
   tile reads its own tile-part and the SOT headers (before it — one window
   each, on the first codec tile — everything after up to EOC: a check of the
   OpenJPEG tile-part count, lifted by the step 9 index), a degenerate 8×8
   grid — a pass without loss. On the TCI granule the detail limit is zero,
   `uitests/jp2canvas.txt` waits for `absent text:will not be`; wasm
   2,842,418 → 3,122,098 bytes (ADR 0002 accepted). Along the way found and
   fixed in the SDK: a module is a WASI reactor without `_start`, and the musl
   `pthread_key_delete` hung the first `thread_local!` (at openjp2 T1) —
   thread-local keys are now our own, single-threaded
   (`veldcore/sdk/rust/src/tsd.rs`). Not done: comparison with hayro on 5/3
   and 9/7 (hayro is gone; instead — a reversible round trip "openjp2 encoder
   → decoder" byte for byte); module startup not measured; still three box
   walkers (`header_dims`, `codestream_at`, `gml_text`); `easting_first` and
   `binding_trouble` for JP2 untouched — georeference.md is written after the
   code as it is; the criterion "bytes for a level 0 tile ≪ the file" is not
   met: a tile costs its tile-part plus one reader window per SOT after it (on
   a 16-tile fixture — about a third of the file, for a TCI over the network —
   a block per tile-part, about half the file) for every `produce`, until the
   step 9 tile-part index removes the traversal; the 12-bit strip stretch is
   verified by a fixture, not a granule. Check: codec/jp2/reads tests, tiler
   mutations 13/13, uitests jp2canvas/preview/shown/remote.
9. **[M] Pointwise JP2 over the network** — done on September 3, 2026. Its
   own tile-part index over TLM and an excerpt per tile
   (`adapters/excerpt.rs`): the main header with boxes, the tile's tile-parts,
   EOC — the codec has nothing to traverse, the codec works per tile. With PLT
   and a progression whose resolution is in the outer loop (LRCP with one
   layer, RLCP, RPCL) the tile-part is cut after the packets of the needed
   resolution by the precinct count (B.6), missing packets are appended empty,
   Psot and TNsot are rewritten in the header copy; cutting is allowed only
   when the packet count matches the PLT entries, otherwise whole tile-parts.
   Three outcomes (ADR 0004): no TLM — SOT traversal by the codec, as before;
   TLM — tile granularity; TLM with PLT — level granularity: the first decides
   describe, the second — assembling the excerpt on every tile. A 64 KiB
   tile-part probe carries the header and, for a granule, the two coarsest
   resolutions — the three coarsest pyramid levels. Granule measurement
   (Kakadu 7.4, TLM over 121 tile-parts, PLT everywhere, LRCP/1 layer, 256²
   precincts, no SOP/EPH/COC/POC): tile-part prefix at factor 4 — 9 KiB (sum
   over the granule 1.02 MiB), at factor 3 — 35 KiB (3.9 MiB), at 2 — 134 KiB
   (15 MiB), at 1 — 476 KiB (54 MiB), at 0 — the whole file (129 MiB); the
   packet count matched the PLT for all 121. The description line names the
   read path. Encoder fixtures (`Addressing`: TLM, PLT, precincts, JP2
   container): the excerpt decodes byte for byte like the whole file on all
   layouts; the log: a level 0 tile reads only its own tile-part, the coarsest
   level — one probe per tile and under an eighth of the file, TLM without PLT
   — whole tile-parts. The run: the `delivered N` verb — a guarantee for the
   wire that run-uitests.py checks against `network::perf` in trace.log (a
   copy `<name>.trace.log` stays with the scenario; the guarantee is not kept
   without a single network line), the tile cache is put off for the duration
   of the run
   — every scenario is cold; `uitests/jp2remote.txt` opens the neighboring
   un-downloaded T31TGK granule (129 MiB) over the network: the coarsest level
   75.0 s, by its end 109.6 MiB (84 %) delivered in 64 requests of 1.71 MiB
   each, four zoom levels — from the pool, closed at 115.6 MiB (89 %), the
   whole scenario 141 s. jp2canvas on a cold cache 23.3 s (15.0 s warm). Not
   done: "delivered ≪ the file" over the network is not achieved — the
   coarsest level needs 1 MiB of the file, the module asks for 121 probes of
   64 KiB (7.6 MiB), and the network brings 84 %: readahead doubles the
   request on a miss right past the end of the previous one, and tile-part
   headers every two blocks (BLOCK 512 KiB) look to it like a sequential pass;
   the scenario guarantees 95 % (the file is not fetched twice), tightening —
   step 12 with these numbers. A tile made of several tile-parts pays a probe
   for each, without PLT in each it is read in whole tile-parts, and through
   the decoder it is not checked at all — only the byte-level assembly: the
   crate encoder with `tp_on` puts all the tile's packets and one Psot into
   every tile-part, so fixtures for the third outcome cannot be obtained from
   it. A file with PPM is not rejected by describe, although random access to
   a tile with PPM in openjp2 is broken on both paths (a single running
   pointer of packet headers). `codestream()` and `Index::of` go through one
   segment walker, but there are still three box walkers, and two SIZ/COD
   parses. Check: excerpt/jp2/reads tests, tiler mutations 20/20, uitests
   jp2canvas/jp2remote/remote/preview/shown on a cold cache.
10. **[M] The level table onto the wire** — done on September 3, 2026.
    `Described.levels: repeated Level {serve, from, bytes, fits}` — the same
    table by which `produce` chooses its arm; `reach`, `windowed`, `finest`
    and `enum Reach` removed from the wire (field numbers reserved). Wrap
    `tiles.rs`: `Meta { rows }`, `finest()` — the first row that fits,
    `pointwise(level)` — the row, `ladder(target)` — from the serving column:
    a step coarser than the target is taken if it is pointwise, of the same
    pass as the target, or the apex with its own pass (JPEG); `describe`
    rejects a pass finer than its level; `want(sharpest, &meta, cap, …)`.
    Behavior versus the old `Reach` changed in one place: a striped TIFF over
    a pointwise target takes its pointwise steps, over a pass target — all
    levels of the same pass (before — only the target). `MAX_QUERY_TILES` — in
    the shared `tile-cache/src/tile.rs`, included into the tiler wrap as well;
    `cap_tiles` is cut by it. The golden string "more detailed … will not be"
    in full — one test at `Meta::capped`, the other places are checked against
    `capped()`. A large JPEG (12000², generated by PIL in a scratch scenario,
    no fixture created): describe says "6 levels, pass from its level over
    0–5, more detailed than 6000×6000 out of 12000×12000 will not be", zero
    refusals in host.log. README — 50 lines of pointers; the sections moved
    into docs/: new overview, modules (with ABI), window-and-render,
    viewing-pipeline, globe, limitations, operations/configuration; imagery
    extended (radiometry, NetCDF variable choice, product layout, overlay set)
    and georeference (rewritten after the code: tags, nodes, seating, offset,
    datum with tests, longitude unwrapping, Rough, edges). The stale parts of
    README were not carried over but named in the authors' reports (numbers
    without a holder, shadow, datum, WebP, order of magnitude, manifest).
    Along the way, stale comments removed: "a striped TIFF with a full pass" in
    README, `thread_local` on `ModuleState` in the SDK. Not done: the `bytes`
    of a table row on the wire is read only by the log (the peak in the
    describe line `Meta::note`), the consumer makes no decisions from it;
    there is no permanent scenario with a large JPEG (binary fixtures are
    forbidden), the check was done with a scratch scenario. Check: grep
    `Reach::` in consumers is empty; wrap/tiler/consumer tests, wrap mutations
    4/4 and tiler 20/20 (the mutation about `reach()` replaced by a table
    row), pytest 170, all uitests scenarios, the large JPEG zoomed to native
    scale.
11. **[L] NetCDF as an implementer** — done on September 5, 2026. The variable
    is a source for the chunk grid driver: `netcdf::Layout { grid, path, dtype,
    fill, mapping }`, the chunk — a row window across the full width
    (`rows_of`: the file's chain of chunks no longer than TILE; a unit leading
    axis and a chunk of the full height — a window as large as the plane),
    `Chunks` implements `Chunked` through `read_{u8,u16,i16,f32}_rows` into
    `Samples`; the level table for NetCDF — the same `chunked(grid)` as for
    TIFF and JP2. describe — by a sample of up to four scattered windows (step
    coprime with the width), a uniform one is remembered along with the layout;
    the plane is held neither in the parse nor in the memo. WIRE_PLANE,
    PROBE_BUDGET, `affordable`, `strip_bytes`, `Source`, `holds_samples`, the
    heavy/light memo died (now two unweighted LRU slots), `near` in three
    protos (reserved) and four modules; `total_bytes` of progress — the file
    size for everyone; NetCDF progress — from the far read position of the
    resource. Deviations: the degradations are not "exception rows" but an
    honest count of the same grid — a plane window gives a pointwise zero
    level at the price of the whole plane (`pixels == w·h`), and describe with
    produce read it twice (over the network — from the block pool); a variable
    of no more than four windows (OLCI: three chunks of 5026 rows) the sample
    also reads whole, one window at a time — it saves memory, not bytes; a
    window is admitted before reading by the same fresh-chunk gauge as TIFF.
    Not done: a variable with data only outside the sample windows passes as
    empty (the price of the sample, recorded in limitations.md); the crate has
    no window over a named axis — a unit leading axis costs the plane (lifted
    by step 13); there is no permanent scenario with NetCDF produce
    (ragged/remote bring up a layer, the detailed OLCI raster is not requested
    at their scale). *Check:* fixtures written with the hdf5-pure writer in
    reads.rs — chunked 200×12000 (describe beyond the head window reads 4
    windows of 5 chunks, ≤ a fifth of the samples; a tile reads only its own
    10 chunks; window == pass), `[1, h, w]` and a single-chunk one (window —
    the plane, `pixels == w·h`, every chunk touched), the empty one yields to
    the measured one, the uniform one is last and read once; tiler tests 205,
    image-tiler mutations 27/27 (+7 about NetCDF), pytest 173, all 18
    scenarios; on disk: S5P CO 215×372 — "window 372 rows, 1 level, pointwise
    0, peak 119 MB", direct access 1 chunk; OLCI LRR 1217×15076 (GIFAPAR u8,
    file chunk 5026 rows) — "window 5026 rows, pointwise 0–4, pass from 0 over
    5, peak 191 MB", binding from the neighboring file 77×15076.
12. **[M] Network** — done and measured over the wire on September 5, 2026.
    `veld_resource_prefetch(id, ranges)` on both sides of the ABI and
    `RangeSource::prefetch`; the network pulls missing blocks in runs of
    consecutive ones (`runs_of`: run ≤ READAHEAD, order ≤ PREFETCH_CAP = half
    the pool) over IN_FLIGHT = 4 requests in flight, readahead untouched;
    `Chunked::prefetch` — the driver orders the chunks under the tiles before
    reading: TIFF from the offset and length tags, JP2 with TLM — tile-part
    probes and excerpt pieces. Readahead doubles only on a request consumed to
    the end (`Readahead::consumed`) and belongs to the object (in `Blocks` by
    the ownership key). The host's ceiling for a single read —
    INSTANCE_MEMORY_LIMIT. ADR 0005; resources.md, modules.md, limitations.md,
    imagery.md. Week-long presigned URLs were tried and rejected by the storage
    (`InvalidAccessKeyId` for any term, measurement in the ADR) — the signature
    stayed in the headers for a quarter of an hour; re-signing on 401/403 is
    not done and remains the next network task (the network ↔ data-provider
    protocol). Not done: NetCDF does not name its chunks ahead of time; JP2
    excerpt pieces are ordered per tile — the four in flight are unused at
    detailed levels; the order is synchronous — the first tile waits for the
    whole order; BLOCK/READAHEAD/POOL_LIMIT untouched (by the rule) — and now
    there is a number against it: a 64 KiB probe buys a 512 KiB block, 121
    probes — 68 MiB out of 129, while the module asks for 7.6 MiB and needs a
    megabyte; the next network move — a block sized to the named order. *Check
    done:* clean tests of run splicing, readahead (windows, probes, long read,
    two opens, closing the neighbor), prefetch (runs, hits, parallel);
    reads.rs — TIFF and JP2 name exactly their chunks; s3 — the signature in
    headers; network mutations 7/7, image-tiler 29/29. *Over the wire (ADR
    0005):* jp2remote — the coarsest level 68 MiB (52 %) in 115 requests over
    59–84 s (two runs) versus 109.6 MiB (84 %) in 64 over 75 s (one run on
    September 3); the resource closed at 62 % versus 89 %; bytes and requests
    are deterministic, time within network spread; `delivered` tightened to
    70. Sentinel-1 COG over the network: 6 chunks under 12 tiles — one
    request, 12 under 40 — three, the whole ladder 25 MiB out of 382 in 11
    requests. A layer older than a quarter of an hour is not verified until
    re-signing exists. Found during the run: a reqwest request starts timers
    when the future is built, and one built outside `block_on` in the prefetch
    thread crashed with "no reactor running" — with `panic = "abort"` it took
    the host down; the request is now built inside `block_on`.

**Next, in this order:** the pipeline into wrap (`Pipeline<K>` → `Vec<Action>`,
an honest L — an extraction with the abstraction "the caller is in force", not
a move; the scenario test is shared by both by construction); merging the
number fixations per the harness gauge; the `band` axis in the contract for
all formats and in the fingerprint; `Binding::Named`/UtmFrame — only after
the step 3 answer about GML for PVI and B* and with the named trade-off (Named
— the only binding before describe; without it a seconds-long granule lies by
the catalogue outline); the seven collections and two routers of data-browser
— after the pipeline into wrap, which also removes most of the orchestration
from the screen. Do not introduce GeoPoint in core.proto. Wrap crates:
globe-wrap exists only for the outline geometry in data-browser (footprint.rs)
and empties out along with its move; the ui-service builders (1103 lines,
three representations of the widget model) to be cut by generating setters
from proto — only after the first test on that wrap, otherwise the cut cannot
be checked. Found while measuring the granule (step 3): in one run quitting
the application during a remote JP2 read waited up to the run limit (in others
— instantly); since step 22 a trip in flight returns on the shutdown
announcement, not on its own timeouts and retries, but that very case did not
reproduce. In the same place (step 22) once the host died with SIGILL while
bringing up modules — a jump to an address in the middle of an instruction in
the Cranelift register allocator (`regalloc2::ion::Env::init`), not a `ud2`
from code; the same day rustc crashed with SIGSEGV and twice with the ICE
"unstable fingerprints" — all under parallel load. If it repeats — the
wasmtime cache or compiling modules one at a time, and check the machine.
`globevar` takes the freshest S5P granule, while a short one (tens of rows,
nightly) has CO empty in the sample and another variable lying there — the
scenario needs a granule with data, not the first by date. The headless run
opens the door to running several scenarios in parallel (host memory ~0.5 GB
per launch): hosts need their own `runtime/` — logs, window state, tile cache
— and scenarios that write into the library (`download`) a queue; the host's
`runtime_dir` is already the parent of the config directory, so they can be
split by directories. Name filtering in the lists does not look inside
expanded folders (a granule file cannot be found by name until its path is
expanded) — a separate screen task, after the `open_resource` of step 6. The
reader window does not look back, and a catalogue lying after the data (this
is how the crate encoder for `tiff` writes, not GDAL: for a COG all IFDs are
in the file head) costs three windows per IFD (the test
`describing_reads_the_header_and_the_ifds_only`) — real COGs cost differently,
measure on them before giving the reader memory backwards (not done in step
7).

13. **[S] NetCDF window as a region** — done on September 5, 2026 on a fork of
    hdf5-pure. A region over all axes `read_*_region(start, count)` was
    proposed to the crate (upstream issue #465, PR #466: CI green, 23 checks;
    waiting on the author); image-tiler depends on the PR commit in the fork
    `BinaryCat17/hdf5-pure`
    (fa5dfc64521dfe1d554b9f1cffee836c873718ff) until the release, after it —
    `version` in config.yaml. The row window is cut by a region along the row
    axis of the plane, wherever the file holds it (`Layout::region`, `rows_of`
    without a leading axis); the unit leading axis of S5P does not widen the
    window. A plane window remains for a single-chunk variable (the chunk is
    inflated whole) and for a variable no larger than TILE. *Check:* a fixture
    `[1, 6000, 450]` chunked in rows of 100 — a window of 500 rows, the sample
    reads 20 chunks out of 60, the level-zero tile 10 out of 60;
    `window_region_lands_on_plane_axes`; tiler tests 208, image-tiler
    mutations 30/30 (+2 about the region axes), pytest 177, all 18 scenarios;
    on disk S5P NRTI CO 215×372 is not larger than a tile — the window is
    still the whole variable, "1 level, pointwise 0, peak 119 MB" unchanged; a
    full-orbit OFFL granule (4172 rows), where the window would be 0.46 MB
    instead of the 7.5 MB plane, is not on disk — not measured.

14. **[S] Network block sized to the order** — done and measured over the wire
    on September 5, 2026. `BLOCK` 64 KiB — the grain of the pool and of an
    order, exactly a JPEG 2000 probe and the edge of the fingerprint: a
    pre-order buys the blocks under its ranges and nothing more, a probe from
    a random offset touches two; a read names its own length to the network,
    and its blocks ride in one request (`Readahead::plan(index, total,
    needed)`), readahead doubles on top up to `READAHEAD`. The JP2 order in two
    passes: probes of all ordered tiles, then the excerpt pieces of all tiles
    in one order (`Chunks::planned`), not one tile at a time. ADR 0005
    extended with a dated section. *Check:* network tests (a long read in one
    request and the readahead continuation [12, 24]; an offset probe — two
    blocks; doubling to the ceiling with no baked-in number), veldmap-host-network
    mutations 8/8, image-tiler 31/31; all 18 scenarios. Over the wire
    (jp2remote, T31TGK 129.0 MiB, cold tile cache, two runs, bytes matched to
    one): coarse level 15.4 MiB (11 %) in 122 requests over 26–30 s versus
    68.0 MiB (52 %) in 115 over 59 s; level 2 slower (23–24 s versus 17.8: its
    pieces used to lie in the probe blocks), level 1 faster (14–16 versus 21);
    closing 31.0 MiB (24 %) and 255 requests after 70–77 s versus 81.0 MiB
    (62 %), 141 and 108 s; without the second pass the same bytes cost
    123–126 s — 133 per-tile orders in a row. `delivered` 70 → 35; the pair
    BLOCK = PROBE = SAMPLE in test_wire_pairs.py. Not done: the Sentinel-1 COG
    over the network not re-measured (the scratch scenario vanished from /tmp;
    expectation — the same chunk runs, no more than 24.6 MiB of bytes);
    `IN_FLIGHT` left at 4 — the coarse level is 122 requests in fours, eight
    threads — a separate measurement.

15. **[M] Grid rank, spare rasters, grids by rows of nodes** — done on
    September 6, 2026 (plan-detail-and-quality.md, steps 2 and 7). The choice
    of the detailed raster by names gained a fifth argument — the density of
    the Sentinel-3 grid (`grid_rank`: `a b c` before `i f` before `t`, nadir
    before slanted; alphabet — sixth): for an SLSTR L1 granule the top takes
    the half-kilometer `S1_radiance_an.nc`, not the kilometer `F1_BT_fn.nc`
    by alphabet. The provider names spares after the chosen one
    (`Scan.alternates`, `spares`) — the best file of each coarser grid, without
    the reference one — because a name cannot see emptiness: a nightly granule
    holds the visible channels as solid "no data". The spares ride as the same
    `ImageryDetailed` rasters in order; the browser pairs coordinates with a
    raster by number in the reply (`paired`), the globe folds them into
    `Raster::spares` and after a describe refusal puts the next one in the same
    place (`next_spare`). The half-kilometer grid gets its binding from its
    per-sample `geodetic_<tag>.nc`, the kilometer one — the reference `tx`, as
    before. The coordinate grids the tiler reads as rows of nodes, a region of
    one row (`Rows`), not as a plane — a set opened once with a chunk cache
    under a row of chunks across the plane (for the half-kilometer grid two
    chunks of 1.6 MB — the crate's default cache would not hold them, and every
    row of nodes would inflate them again): the binding budget `TIES_BUDGET`
    with its peak is removed, a failed row read is cached and named in its own
    words. *Check:* provider tests (RBT with real names: S1_radiance_an, the
    spare F1_BT_fn, without half-kilometer ones — F1_BT_fn, OLCI without
    spares; geolocation by grid), browser (`paired` — spares each with their
    own coordinates), globe (`next_spare`), tiler
    (`ties_read_the_rows_of_their_nodes_not_the_plane`: a 300×600 grid read in
    rows of one chunk — 21 rows out of 600 for latitude and for longitude, 441
    nodes; `a_coordinate_chunk_is_inflated_once…`: 267×1500 f32 chunks read
    once per 21 rows of nodes); mutations data-provider 2/2, image-tiler
    32/32; all 18 scenarios, after the skeptic's fixes — scratch RBT, ragged,
    remote. On disk (RBT of August 24, nightly): `S1_radiance_an` was chosen —
    empty (0 out of 1,599,000 in the sample) — the spare `F1_BT_fn` 1500×266
    described itself, binding from `geodetic_tx` 441 nodes, 42 rows read; the
    detailed is no more detailed than the 2422 preview — the quicklook lies on
    the grid, the layer lives (without spares the layer was dropped: "the
    catalogue outline is complex"). Not done: there is no daytime SLSTR granule
    on disk — the gain of the half-kilometer channel over the quicklook (3000
    versus 2422) is not shown on the globe, and the seating of the per-sample
    `geodetic_an.nc` by rows of nodes has not run on the globe (only as a
    fixture); "name the chosen one" on the screen (step 3 of the plan) and the
    variable choice (§4) untouched.

16. **[S] Name the file lying as detailed** — done on September 6, 2026
    (plan-detail-and-quality.md, step 3). The globe knows no names: a raster
    carries the number of whoever sent it (`OverlayRaster.ordinal`), and the
    number of the one lying as detailed rides back in
    `OverlayProgress.detailed`; the browser remembers the provider reply keys
    by number (`OverlayState.files`) and puts the file name before the words
    about it and only before them: the globe splits the caption into what is
    said about the detailed one (its refusal, its limit —
    `OverlayProgress.detailed_trouble`) and the rest (the preview limit, the
    binding refusal), otherwise the preview limit under the detailed file's
    name would read as something said about it. The name and the words are cut
    separately ("F1_BT_fn.nc · no more detailed than the preview: detailed…",
    in full — in the tooltip); the sentence itself is rearranged so the gist
    comes first. With spare rasters the name changes along with the file — the
    number moves to the spare (`next_spare`). Along the way, the `.origin`
    sidecar without a file from the step 12 queue was examined: this is not a
    defect but an intention record (`LIB_PAUSED`, "Resume"); the download
    scenario then started from a clean library. *Check:* globe tests (the
    number moves with the spare), browser
    (`the_one_made_detailed_is_named_by_the_file_name`: number → name, an
    empty number and a number outside the reply — no name; `paired` with
    numbers); mutations data-browser 2/2, globe 2/2; a new permanent scenario
    `uitests/slstr.txt` (the RBT granule on disk): waits for `F1_BT_fn.nc` in
    the layer line — both the spare and the name at once, 44 s; all 19
    scenarios.

17. **[S] Name the displayed variable** — done on September 6, 2026
    (plan-detail-and-quality.md, §4, first sub-step). A NetCDF file carries
    dozens of variables, one is displayed — by the CF rules the tiler chooses
    it (`netcdf::preferred`), and the choice was visible to the viewer only in
    the log. Now the choice is named: `Info.variable` → `Described.variable`
    (path in the file, `long_name`/`standard_name`, `units` as written;
    comparisons with CF use its own lower-case copy) → `Meta.variable` at both
    consumers → `ViewState.variable` of the canvas and
    `OverlayProgress.detailed_variable` of the globe (the variable of the one
    lying as detailed, not of the preview). The browser names it in words
    (`format::variable`: name, units, file words) first in the properties strip
    of the canvas — into the remaining panel width from the other facts and
    the space for the display progress, file words only if they fit whole —
    and in the layer line after the file name, until there is something worse
    to say about the file; the line tooltip — one line for the file name, the
    variable, and the trouble, split so that the short parts stay whole
    (`format::share`). The choice is still the tiler's:
    `DescribeRequest`/`ProduceRequest` carry no variable (limitations.md).
    *Check:* tiler tests (the chosen one, not the first; units as written, CF
    comparisons case-insensitive), wrap (`Meta.variable` rides and is named in
    the log), canvas (`ViewState.variable`), globe (the variable of the
    detailed one, not the preview), browser (the variable after the name,
    before the words about the trouble; the tooltip splits; `format::variable`,
    `format::share`); mutations: new image-tiler 1/1, globe 5/5 and
    data-browser 6/6 in full; a new permanent scenario `uitests/s5p.txt` (the
    L2__CO granule on disk): waits for `carbonmonoxide_total_column` in the
    properties strip — on screen 5 s after the scenario starts (s5p.log); all
    20 scenarios.

18. **[S] The file's variable list** — done on September 6, 2026
    (plan-detail-and-quality.md, §4, second sub-step). The tiler returns in
    the description all candidates in its own order of preference
    (`Described.variables`, by headers — an empty or uniform one is present in
    the list), the wrap puts them into `Meta.variables`, the canvas into
    `ViewState.variables`. In the canvas properties strip the variable became
    a button: under it the list of what was chosen from, the displayed one
    checked and always among those listed, a long list truncated by number
    (`VARIABLES_LISTED` = 16, an "and N more" note — not an item). List only:
    an item closes the panel, there is no choice behind it — tiler requests
    carry no variable (limitations.md). The dimensionless CF unit "1" is not
    named in the caption ("qa_value — data quality value"). *Check:* tiler
    tests (the list — all candidates in order, with the empty one), wrap and
    canvas (the list rides as-is), browser (`variable_items`: order, check
    mark, truncation; `format::variable` without "1"; the `preview_variables`
    message in the walk-through); mutations: new image-tiler 1/1, data-browser
    8/8 and globe 5/5 in full; the `s5p` scenario opens the list and waits for
    `qa_value`; all 20 scenarios on the final build.

19. **[M] Variable choice in the canvas** — done on September 6, 2026
    (plan-detail-and-quality.md, §4, third sub-step). A list item under the
    frame names the variable to the canvas (`VariableRequest`), the canvas
    re-describes the same resource with it (`ShowRequest.variable`,
    `DescribeRequest.variable`, `ProduceRequest.variable`); the tiler checks
    the named one with the same sample as its own (empty — a refusal in words,
    uniform — displayed, not from the list — a refusal by name; for a scene —
    a refusal: there is one variable), the parse in the memo and the
    fingerprint are keyed by the variable (`fingerprint::hash_of`: into the
    hash, not into the key text; an empty one does not enter — the cache of
    past displays is intact, `DECODE_REV` not bumped). The choice belongs to
    the tab (`PreviewState.variable`), it is written into `SavedTab::Preview`
    when the canvas displays it (a rejected one is not written — it would open
    with the same refusal and no list), and it opens with it; the variable list
    is kept from the last report that carried it, and after a refusal of the
    named one the button keeps it, with no check mark in it. The camera
    survives while the plane of the variable is the same
    (`View::plane_changed`), otherwise the frame is fitted anew. The globe
    knows nothing of the choice (limitations.md). *Check:* tiler tests (the
    named one displayed/rejected by sample/by name, for a scene — a refusal;
    fingerprint — the variable in the hash, empty — as without it; memo — the
    same variable its own parse, a different one — the neighbor), canvas (the
    same plane — the camera intact), browser (the choice is remembered, the
    displayed one by default is not re-named; the list survives a refusal; a
    confirmed variable survives the write, a rejected one is not written, an
    old record reads; the `preview_variable` message in the walk-through);
    mutations: image-tiler 6/6 under the "variable" filter (three new),
    data-browser 10/10, image-view 1/1 (new set), globe 5/5; the `s5p`
    scenario chooses `qa_value` and waits for it in the strip without the old
    one; all 20 scenarios (in one of the three full runs `remote` exited with
    a non-zero code when closing a remote resource — unrelated to the step, in
    the queue).

20. **[M] Variable choice on the globe** — done on September 6, 2026
    (plan-detail-and-quality.md, §4, fourth sub-step; four globe items of the
    plan closed). The layer caption "On view" for a multi-variable file is a
    button with a list (the same `components::variables` as under the canvas;
    the list rides in `OverlayProgress.detailed_variables` from the last
    description that named it). An item names the variable to the globe: the
    set is resent with `OverlayRaster.variable` on the raster of its number,
    the globe recognizes the previous overlay by resources and re-describes the
    raster with it in place (`Overlay::revariabled`: the previous pass is
    released, tiles under the new fingerprint, spares remain). A refusal of the
    named variable — the raster complains in the tiler's words: a spare does
    not take its place, the layer is not buried, the list stays. What is named
    sticks to the file by number and is removed when a spare becomes the
    detailed one; the layer's choice is not saved (limitations.md). *Check:*
    globe tests (only the raster with a different variable is re-described, not
    the spare; the list survives a refusal), browser (the variable rides to the
    raster of its number, is removed with the spare, the list is stored;
    messages in the walk-through), the shared list (order, check mark,
    truncation, without a displayed one — no check mark), globe natively (the
    same set with a different variable re-describes only that raster;
    `Raster::refused` — a refusal of the named one keeps the file and spares;
    `Overlay::doomed` — a rejected choice does not bury the layer); mutations
    globe 9/9 (four new) and data-browser 11/11 in full; a new scenario
    `uitests/globevar.txt` (network: S5P CO onto the globe, choosing
    `qa_value` in the layer line, 22 s); all 21 scenarios on the final build —
    three times.

21. **[S] Put on the globe with this file** — done on September 6, 2026
    (plan-detail-and-quality.md, §4, last item). For a file inside a scene the
    row menu has "On the globe with this file" (`Msg::GlobeFile`): the layer's
    rasters are requested from the provider with the named file
    (`ImageryRequest.wanted`), the provider puts it as detailed, and its own
    choice and its spares — behind it as spares (`imagery::preferring`); not a
    raster or not from the product — as if unnamed, with a log entry. For a
    layer already in place the rasters are requested anew (the globe replaces
    the set by resources), for a layer being assembled the request waits for
    its end, for an absent one — waits for the product from the catalogue
    (`State::file_wishes`), while a cancelled display, a catalogue refusal, and
    a dropped layer carry it away. A refusal of the named one (not in the
    product, not a raster by name, a quicklook) — in words in the reply
    (`ImageryResponse.wanted_refused`) and in the status line, the request is
    dropped; whether the file is really a raster, the tiler will say. The file
    keys of the reply go into the layer along with the rasters
    (`Assembly.files`): before replacing the set the globe reports on the old
    one. The layer line names the first described file. *Check:* provider tests
    (`preferring`: the named one first, layout choice as the first spare, the
    same choice is not duplicated; manifest, a foreign file, and a quicklook —
    a refusal in words), browser (`imagery_request` carries the request; the
    request waits for the assembly, waits for the product, leaves with a
    cancellation and with a dropped layer; the same file — nothing to do; the
    product at hand — at once; the `globe_file` message in the walk-through);
    mutations data-provider 3/3 and data-browser 12/12 in full; a new
    permanent scenario `uitests/slstrfile.txt` (the RBT granule on disk:
    expand the scene, the menu of the file F2_BT_in.nc → "On the globe with
    this file" → the layer line names F2_BT_in.nc, 18 s); all 22 scenarios.

22. **[S] Crash on exit with an open remote resource** — done on September 6,
    2026. The `remote` scenario ended "did not converge" three times a day in
    full runs — with no panic in stderr and no line about closing the resource
    in the log. The panic was invisible because the launcher merged the host's
    stderr into its own stdout, while the run sends stdout nowhere: the run's
    panic catching (step 12) never worked under the run, and its test held on
    to code 250 — a leftover of `sys.exit(-6)`. Now the launcher keeps the
    host's streams its own, death by signal is returned like the shell
    (128+N), the run names the signal in the verdict ("DID NOT CONVERGE
    (SIGABRT)"), a panic word in stderr counts as host-side only on SIGABRT (a
    guest panic goes to the same place through WASI, and the host survives it
    with a trap), gives the host `RUST_BACKTRACE` and keeps `<name>.stderr`
    for the non-converged one. With this the crash reproduced in words: "A
    Tokio 1.x context was found, but it is being shutdown" — `Runtime::drop`
    tears down the timer driver before waiting for the blocking threads, while
    the windowed read of the remote resource sits on a blocking thread in the
    `block_on` of the request whose poll first touches the `read_timeout`
    timer; the shutdown flag (step 12) closed only trips started after the
    announcement, not the one in flight. Shutdown became a flag with a
    notification and a trip count (`core::Shutdown`): a trip waits for the
    announcement on equal terms with the server reply and on it drops the
    request without polling the timer (`range::awaited`, `biased`), the runner
    announces shutdown both from `exiting` and after the event loop whatever
    its outcome, and before tearing down the runtime it waits for in-flight
    trips to return (`Shutdown::settled`, up to 2 s) — so that a thread
    preempted mid-poll does not finish the request on a torn-down driver.
    *Check:* reproduction before the host fix, already with captured stderr —
    a temporary scenario exiting one second after opening the resource: 1 out
    of 6 "DID NOT CONVERGE (SIGABRT) + HOST PANIC: A Tokio 1.x context was
    found, but it is being shutdown" (tokio thread, entry.rs:539); after the
    fix — 16 out of 16 and another 10 out of 10 on the final build, in every
    log the read in flight returned with a "host is shutting down" refusal and
    the resource closed normally. Tests: network (a trip in flight is dropped
    by shutdown, after the announcement — no trip, shutdown counts the trip
    until it returns), core (the announcement wakes a waiter and is visible to
    the one asking afterwards; waiting for trips until return and until the
    limit), buildgen (the launcher keeps the host's stderr its own and returns
    128+N; the verdict names the signal, the killed middleman, and the panic
    only on SIGABRT). Mutations: veldmap-host-core 5/5 (a new file on top of
    the two previous dispatcher mutations), veldmap-host-network 11/11 (three
    new). The exit scenario became permanent — `uitests/remoteexit.txt`
    (network, 15 s). Full runs on the final build: 23 scenarios converged, but
    not in one run — in one `slstrfile` ended "DID NOT CONVERGE (SIGILL)" on
    module bring-up before the first step (the fault address — the middle of
    an instruction in the Cranelift register allocator, unrelated to the exit,
    see "Next"), in another `keepscroll` did not wait for the line to leave
    (alone it converged at once); the previous run, without the host fix — all
    22.

23. **[S] Running scenarios without a window** — done on September 6, 2026 on
    the owner's word: the runs "get in the way by popping up windows and
    taking focus". The runner needs a window for the swapchain and mouse
    input, while in a run the input is synthetic and the screenshot is taken
    from its own texture (`capture::shoot`). Under `run-uitests.py`
    (`VELDMAP_HEADLESS=1`) the host creates no window (`Screen::Offscreen`):
    modules draw into their own textures as before, the frame tick gets its own
    60 Hz clock instead of vsync (`Pace`: no more often than the period, an
    overdue one — immediately, the next one from the fact), the blit to the
    swapchain and the presentation are skipped, the frame format is assigned
    (`Bgra8UnormSrgb` — the same one the Vulkan swapchain offers first among
    sRGB); without a scenario the host does not start without a window. To
    watch a scenario with your own eyes — run it by hand with a window.
    *Check:* the clock test (`frames_keep_the_period_and_do_not_catch_up`),
    veldmap-host-gui mutations 2/2 (a new file); a full headless run on the
    final build — 22 out of 23 in 7.8 min, not a single window on screen; the
    screenshots against the windowed run of the same day matched byte for byte
    for 21 out of 27 (six differ: catalogue output by time — search, remote,
    unviewable; a screenshot mid-camera-flight — jp2canvas-zoom,
    jp2remote-zoom, globe). `globevar` did not converge, twice: for the
    freshest granules of the hour (0.8–1.3 MB, 46 and 107 rows) the CO
    variable is empty in the sample ("0 valid out of 9890", the tiler trace),
    and `solar_azimuth_angle` lies there — data, not a window; the scenario
    clings to the freshest granule, and that is its weakness (see "Next").
    Host memory per run (once a second across all launches): peak 566 MB,
    average peak 437 MB.

24. **[S] Re-sign on 401/403, quad by raster corners, `globevar` on a granule
    with data** — done on September 6, 2026, the last step of this lane. The
    step's premise — "a SigV4 signature in the headers lives a quarter of an
    hour, and the globe layer is blind after it" — turned out to be an AWS
    rule, not this storage's: the measurement of September 6 (GET of two bytes
    with `x-amz-date` pushed back by 14 minutes … 30 days) — 206 every time,
    and two runs with a layer older than sixteen minutes saw no refusal (ADR
    0005). Re-signing is done anyway — as insurance for a storage with expiry
    checking and for a revoked key: a 401/403 mid-read now has its own outcome
    (`Attempt::Unsigned`), on which the network asks whoever holds the keys to
    sign the object again — `network/on_resign` broadcast,
    `data-provider/on_resigned` keyed by address (`s3::resign`: its own host
    only and TLS only) — waits up to ten seconds, replaces the headers shared
    with the trip, and repeats the same request once; empty — the old refusal.
    The open probe and the resumed download are not refreshed (limitations.md,
    ADR 0005 extended). The quad of the fallback binding from the catalogue
    outline now rides by the raster's corners (`oriented`: north up, upper-left
    first, longitudes unwrapped toward the first vertex), not by the vertex
    order of the catalogue, from which the scene could land mirrored or rotated
    (no observed case — the mechanism was named by the review); a strip skewed
    more than forty-five degrees stays as listed, and a Sentinel-1 quicklook in
    acquisition geometry slips past the "north up" assumption unchecked.
    `globevar` looks for an OFFL granule, not the freshest NRTI: for a short
    nightly one CO is empty in the sample, and the tiler put another variable
    there. Of the three binding defects (the review memo, September 1), two
    turned out to be already closed by steps 9–12: GMLJP2 is read
    (`gml_placement`), an unknown projection is named in words in the layer
    line ("EPSG:N, and we do not know such a system — the binding stays by the
    outline"); no new projection families were added — EPSG:3035, polar, and
    EASE-Grid do not occur in this household, and their formulas cannot be
    checked without reference points. *Check:* network tests (an expired
    signature is re-issued and the reading continues, a live one is not asked
    for; without a usable new one — a refusal with no retries, with a bad one
    — exactly one retry and a word about it; the by-address queue wakes
    everyone and is torn down), provider (`resign`: the same address, its own
    host and TLS only), browser (a quad from the lower-left counterclockwise
    and from the upper-right clockwise lands upper-left first, a skewed strip
    — as listed, across the antimeridian — west at 170°); mutations
    veldmap-host-network 13/13 (two new), data-provider 4/4 (one),
    data-browser 13/13 (one); the skeptic — an `Arc` cycle (the closure held
    the context, and the context held the carrier through the registry: the
    resource did not close on exit) — a weak reference; a full headless run
    23/23 (`globevar` on OFFL — 58 s), after the skeptic's fixes `jp2remote`,
    `remoteexit`, `globevar` — converged, the resource-closing line in place.
    Three temporary scenarios with a layer older than sixteen minutes (S5P
    twice, JP2 in the canvas with zooming) converged; re-signing never fired:
    the storage accepted the old signature, and the live 401/403 path was
    never seen — it is held by tests.

## What Not to Do

- Do not accept docs/jp2-openjpeg.md as the JP2 reading model (its own loop
  "a row of tiles → strip", a ceiling by `data_size`, `finest = 0`); do not
  do JP2 before extracting the driver "and lay it in later" — keep the order
  3 → 5 → 7 → 8; do not decide about regions before measuring the granule; do
  not fork openjp2/hdf5-pure before being stuck without a fork.
- Do not remove `finest`/`Meta::capped` and the limit caption; do not leave
  `Reach` and the three scalars on the wire "for compatibility" after step 10.
- Do not rename `ImageryResponse.reason` and do not remove
  `LocateResponse.answered`; do not make describe cancellable instead of fast.
- Do not touch BLOCK/READAHEAD/POOL_LIMIT and the pool keys without numbers
  from a large remote read; do not introduce parallel requests "just in case".
- Do not add a field to `Described` for a new format, do not introduce a
  thirteenth ceiling in an adapter, do not make a strip selector for NetCDF
  only.
- Do not rewrite the bus, the schema with codegen, the resource registry, the
  actor model; do not merge the relay into a new service; do not build a
  global tile scheme.
- Do not introduce a second test path (traits "for tests" in adapters, copies
  of production functions, regexp guards over rustc), frameworks, edits to
  rustflags/profile, binary fixtures; do not turn settings into invariants.
- Do not translate the README "as is", do not make translation commits, do
  not keep the README in two languages; do not fix a number with a number; do
  not put plans, "tree state", links to directories outside the repository,
  and CHANGELOG into docs/; do not introduce documentation generators and
  length metrics; do not make a separate pass over the 176 long comments.
- Do not describe MSK-61 as working and a striped TIFF as a full pass.
- Do not clean target/ and generated/; after .proto — only
  `veldcore/target/release/incremental`.
