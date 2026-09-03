# Scenario runs

The interface is checked only by running the application. To make that check
repeatable, the desktop runner can replay a scenario: the same moves, the same
keys, a screenshot at the same moment — and its own verdict. A check that does
not hold fails the run with a non-zero exit code, so "the build passed" and
"the interface works" stay two different statements, but the second is now
also checked by a machine.

The scenario file is named by the `VELDMAP_SCRIPT` variable; screenshots go to
`runtime/logs/` next to the logs, that is, they belong to the last run. The
set lives in `uitests/` and is run as a whole:

```bash
python3 buildgen/run-uitests.py            # every scenario
python3 buildgen/run-uitests.py tabs menus # the named ones
VELDMAP_SCRIPT=uitests/tabs.txt python3 buildgen/run-native.py   # one, by hand
```

Each scenario is a separate launch, and it starts cold. Two things survive a
launch and are set aside for the run and restored at the end: the window
layout (`runtime/state/data-browser.json`), otherwise a scenario would start
"from the tab that was open", and the tile cache (`runtime/data/tiles`),
otherwise a raster shown by an earlier run would come from disk, with neither
the decoder nor the wire taking part — the runner moves the cache aside and
wipes what the scenarios themselves accumulate before each of them. A copy
of `host.log` is kept under the scenario's name, otherwise nothing would be
left of the one that failed first.

**What the set needs from the machine.** Network — for every scenario that
reaches the catalogue listing: the window opens on the catalogue tab, and it
asks the network itself. Downloaded data in `runtime/data/` — for those that
open a raster or put it on the globe. Each scenario states its needs in its
header; on an empty machine the set does not pass as a whole, and that is not
a failure but the price of checking the live application rather than stubs.
A large fixture is fetched once, by hand, with a scenario from
`uitests/fixtures/` — the runner does not look there — and the scenario that
needs it names it in its header.

One scenario takes seconds, half of it the application's start (GPU, module
loading). They cannot run at once: they share `runtime/` — logs, the tile
cache and the window layout.

## Addressing

**An element is named, not pointed at.** The name is what the markup calls it:
the handler's `method[:payload]`, or `text:<part of a label>`. The handler's
key is not part of the address — the markup uses it to name the addressee
tab, and it differs from launch to launch.

An address without a payload matches any: `preview` is every "Open" icon in
the list, `preview:<entry name>` is this one. The first is needed more often:
a row's payload is a scene key or a file name, and writing it into a scenario
ties it to what lies on the author's disk. Several matches are picked by
number (`#2`, counting from one in markup order); an address without a number
demands exactly one match and otherwise fails the run instead of silently
taking the first.

Only what is **visible** counts as found, because only that can be pressed:
scrolled past the edge of a list or a clipping box does not count, and neither
does what an open menu panel covers (a click beside the panel closes it rather
than pressing what is under it). Hence the trick: to wait for a menu to close,
wait for what it covered, not for the menu itself.

Only what `ui-service` draws is addressable by name — pressable boxes, input
fields and labels. A row's menu opens by its key (`open_menu:row:<key>`, the
provider's key of the row), and the items inside it are reached by their
label (`text:`), which depends on the row's state — a scenario that must
converge from more than one state waits for the state first. The globe, the
preview canvas and the panel dividers have no names (they are `Viewport` and
`Divider` and do not announce themselves to the walk); for those the pixel
steps remain.

## Steps

```
# <ms from window start> <action> [arguments]
1500 move 640 300      # cursor, in physical window pixels
1600 click             # press and release the left button where it stands
1700 press             # press and hold — later moves drag
1800 release
1900 scroll 0 120      # wheel; one notch is 120 units, as the window sends it
2000 timeout 10000     # wait limit for the steps below (30 s by default)
2100 wait text:on disk         # wait; the scenario clock stands meanwhile
2200 tap tab_select#2          # wait and press the middle
2250 tap preview#1             # the first "Open" icon, whatever its payload
2300 expect open_menu:sorting  # must be there now — one question, no second chance
2400 absent group:folder       # must be absent now
2500 gone text:loading         # wait until it disappears
2600 type Sentinel     # type where the caret is (a press puts it there)
2700 key enter         # a named key
2800 shot browse       # runtime/logs/browse.png
2850 delivered 75      # promise: no remote resource fetched more than 75% of its length
2900 exit              # close the window
```

`delivered` is a promise about the wire, not a step of the window: the host
only logs it, and `run-uitests.py` checks it after the run against the
`network::perf` lines of `trace.log` (see Verdicts). The share is the worst
resource of the run, in percent of its length, as the network counts it —
delivered bytes, in pool blocks.

`expect` demands "right now" and gives no second chance: it checks what the
previous step already guaranteed. Everything that comes from disk or network
is awaited with `wait`; otherwise a scenario passes on a fast machine and
falls apart on a slow one.

**Wait for conditions, not for seconds.** `wait` holds the scenario clock
until it is satisfied, so the times below it mean "this much after we waited",
not "this much from start". A fixed delay instead of a wait is either extra
seconds on every run or a failure on a slow machine.

Steps are replayed per frame, so neighbours closer than a frame land in one
and run in file order. For dragging this means `press`, the moves and
`release` must be spaced apart: a stuck-together triple arrives as "pressed
and released in place".

A screenshot has to be separated from what it captures: between a press and
what is seen lies the whole chain — `ui-service` catches it, sends it to the
owner of the markup, that one rebuilds the view and sends it back, and the
next frame shows it. Put a `wait` on what must appear before the `shot`; a
short delay is the fallback where there is nothing to wait for. Two things
keep moving across a step: scroll inertia (one `scroll` decays for about a
second) and the camera's flight to a raster; a `shot` right after such a step
catches the middle of the way.

## Verdicts

A run fails not only on a check that did not hold: an unparseable scenario
line, a runner that did not come up, and a run cut short before the end are
failures too. A silent zero would declare the unchecked checked.

**Two failures the scenario cannot see are failures of the run as well.** Both
are found in `host.log`, because no step could name them: they do not touch
the markup, and the scenario has no elements to notice them by.

The first is a **GPU refusal** (`ОТКАЗ ВИДЕОКАРТЫ`). A shader that did not compile, or a vertex
layout that drifted from it, does not crash the application: the window lives,
the markup answers, tabs switch — and an empty place is drawn. The globe and
the canvas have no names, so the refusal is visible only as a `wgpu:` line in
the log (`run-uitests.py::gpu_refused`).

The second is a **module trap** (`ТРАП МОДУЛЯ`). The host rebuilds a trapped module, and that
costs it all its state: the instance is built from scratch and passes init,
and only what reached the disk survives. The application keeps answering; the
module starts from a clean slate, and how visible that is depends on whose it
is — a module drawing the screen loses its whole markup, a background one
loses nothing visible. Exchanges the module had started are settled by the
host with a terminal reply; requests still queued survive the fall and are
answered by the raised instance. The trap is looked for by the line
`поймал трап`, not by
an instance coming up in general: a module killed mid-handler is raised the
same way, and that is normal work (`run-uitests.py::module_trapped`).

The third is a **broken delivery promise** (`ДОСТАВЛЕНО N% ПРИ ОБЕЩАННЫХ M%`),
looked for only in a scenario that made one with `delivered`. What went over
the wire is not in the markup either: the network module counts it and writes
it to `trace.log` as a running total per resource, so the last line of a
resource is its total and the largest share over all lines is the worst
resource of the run (`run-uitests.py::over_delivered`). The format of that
line lives in `range.rs` and the runner's parser next to it; the pair is held
by `buildgen/tests/test_uitests_outcomes.py`. Such a scenario keeps its full
stream next to its log, as `<name>.trace.log`: the numbers behind the verdict
stay at hand, and the next run does not take them away.

The log is read whatever the exit code, and not as a precaution: a trap takes
the module's state, and with it whatever `wait` is waiting for leaves the
scene — so a fall fails a scenario at least as often as it goes unnoticed.
Called merely "did not hold", it would send you to look for the cause in the
scenario, where it is not. So what is found in the log complements the verdict
rather than replacing it: a scenario that held is `сошёлся`, and
`НЕ СОШЁЛСЯ + ТРАП МОДУЛЯ` is two facts, both needed. The same goes for
`ЗАВИС`, where the log matters most: steps are
replayed per frame, so a stalled frame loop also holds the scenario clock —
its own wait limit never fires, and the run hits the global one. That global
limit is `LIMIT_SECONDS` of the runner plus the longest `timeout` the scenario
declares, so a scenario that honestly waits minutes for a download is not
killed for waiting. When the limit does fire, the runner stops the whole
process group: the host is a grandchild, started by `run-native.py`, and that
script hands a `SIGTERM` it gets over to the host — so neither a stopped run
nor a `timeout` around `run-native.py` leaves a window behind.

The order of failures in the line is fixed, so the same pair of runs is always
named the same; there is no causality between them.

Without the variable none of this is active, and an ordinary run does not
know about it (`veldcore/platform/host/runners/desktop/src/capture.rs`).
