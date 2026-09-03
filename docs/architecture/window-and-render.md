# The window and the frame

How the desktop runner opens the window, how a frame is made, how a module
gets a place to draw and gives it back, how input reaches the markup, and how
`ui-service` and `data-browser` share the space when there is not enough of
it. The runner is `veldcore/platform/host/runners/desktop/src/`, the render
queue is `veldcore/platform/host/core/src/graphics.rs`, the markup service is
`veldmodules/ui-service/src/`, the owner of the window is
`veldmodules/data-browser/src/`. Terms are in [the glossary](../glossary.md);
what the screen is made of — panes, tabs, what is saved, one highlight — is
on [the screen page](screen.md).

## The window and its owner

A window is declared by the module that owns it, with the `window` key of its
config (`runtime/config/data-browser.json`; the fields are
`PluginWindowConfig` in `window.rs`: title, logical size, `ui_scale`,
resizable, fullscreen, position). The desktop runner leads exactly one window:
no declaration and more than one are both a refusal at start. The scale the
modules see is the larger of what winit reports and `ui_scale`
(`effective_scale`): on X11 and WSLg winit reports a unit scale on HiDPI
screens, and the config sets the floor.

The runner is the window system's half of the `app` contract
(`veldcore/interface/modules/app/app.proto`): it publishes input, frame
ticks, the size of the window and the questions of a scenario run, all
stamped with the name `app`. The inbound half — the attach of a surface, the
answer to a question, a path to reveal — is the native `app` module
(`veldcore/platform/host/modules/app/src/module.rs`), one for every runner
and free of winit. Which native modules a runner carries is listed in its
`runner.yaml`, and buildgen generates the composition crate from it.

The host does not know who renders. The owner allocates the texture itself,
hands it to its renderer and attaches it to the host — a capability, not a
configuration:

1. The runner publishes `app/on_window_resized` to the owner (targeted, so no
   other module sees it): the size in physical pixels, the scale and the
   format of the swapchain (`get_surface_format_proto`). It goes out once at
   start (`announce`), on every resize, and on a DPI change even when the size
   stays the same.
2. The owner allocates a texture of that size and format and delegates it to
   `ui-service` with `veldsdk::surface::delegate` (`handlers/window.rs` in
   `data-browser`). The window surface has no readers: the compositor of the
   host is not a module and holds no lease.
3. The owner attaches the texture with `app/on_set_surface`. The `Surfaces`
   facade (`veldcore/platform/host/util/src/surfaces.rs`) checks that the
   sender is the instance behind the window's name and may write the texture,
   then puts it into `SurfaceQueue` (`surfaces.rs` in the host core); a
   refusal is logged and dropped, the bus being fire-and-forget. The frame
   loop takes the queue once per frame and swaps between frames; an attach the
   loop did not reach is displaced by the next one, since only the last
   surface can be shown.

Until the first attach the window draws its background colour; after a resize
the old surface is blitted stretched until the new one arrives. The first
`on_window_resized` is also the owner's first move on the bus
(`nav::bootstrap`): by then every module is loaded and subscribed, and before
it the requests of the first tab would reach nobody.

## The frame

The runner asks winit for a redraw at the end of every frame, and a frame
(`Running::redraw` in `main.rs`) goes in one order:

1. The coalesced cursor position, if it moved, then the tick `Frame { dt }` —
   both as `app/on_ui_event` with `plugin_id` naming the owner.
2. The surface swap, if the owner attached a new texture.
3. Every pending render op of the modules, each as its own pass into its own
   target: colour cleared to transparent, depth — when the op has one —
   cleared to the far plane; there is no stencil.
4. The compositor pass: the attached surface blitted onto the swapchain
   texture with one full-screen triangle (`compositor.rs`, `blit.wgsl`) — no
   vertex buffers, the vertex shader computes the corners.
5. Submit under the queue lock, present without it. Under vsync `present`
   waits for the scan-out, nearly the whole frame, and held under the lock it
   would stand across every `MemoryManager::write` a module makes into a GPU
   buffer, and the module would stop keeping up with the loop. Frame order
   does not depend on the lock: only this single-threaded loop writes the
   swapchain.
6. The scenario step, if a scenario runs (below), then the request for the
   next redraw.

A lost or outdated swapchain is reconfigured and the frame asked again; a
timeout or a covered window skips the frame and asks again too — the event
loop waits (`ControlFlow::Wait`), and without that request the application
would stand still, silently, and the scenario clock with it.

**The screen sets the pace.** The present mode is `Fifo` (`init_wgpu` in
`setup.rs`): `get_current_texture` waits for vertical sync, the frame ticks of
the modules grow out of the loop's turns, and the one limiter of the whole
application is the screen. This is not economy. A tick is an ordinary event,
a subscriber's queue is unbounded (`Subscriber` in `dispatcher.rs`), and a
module whose frame costs more than a turn of the loop falls behind for good:
the queue grows and with it the delay between a press and what is seen, while
the module looks healthy — it handles every event, only ever older ones. What
shows this is `ui-service`'s `FrameMeter` (`frames.rs`): one per client with
a delegated surface, keyed by the owner the tick names, ticking on its own
clock rather than on the sum of `dt` — the host's `dt` summed and divided by
frames gives the host's rate however slowly the frames are parsed. It reports
to `trace.log` under `perf`, the shortest and longest gap beside the average;
how to read it is on [the diagnostics page](../operations/diagnostics.md).
`image-view` and `globe` take the same tick and only the tick; `image-view`
has no counter, and `globe`'s (`veldmodules/globe/src/perf.rs`) measures
something else — what the recount of the wanted costs, per burst.

The backend is Vulkan only ([limitations](../limitations.md)); validation
layers are switched on only by wgpu's environment variables, never by the
build.

## Recording a frame

Modules draw through the graphics ABI: `GraphicsDevice::create_resource`
makes shaders, pipelines, samplers, views, bind groups and their layouts;
`GraphicsDevice::execute` accepts a frame; the frame loop drains
`take_pending_ops`. Everything a submitted frame refers to — target and depth
views, pipeline, bind groups, buffers — is resolved and pinned at submit
(`PendingRenderOp` holds an `Arc` on each): a module lives in its own actor
and may free a texture with its very next message, earlier than the loop
reaches the queue, and a frame recorded before the release must still draw
with what it was recorded from. A bad reference therefore comes back as the
error of the submit call, where the mistake was made, not as a warning of the
frame loop some messages later.

Submit checks what wgpu would otherwise check with a validation error, whose
default handler takes the process down: a draw before the first
`SetPipeline`, a depth buffer named as the colour target or a colour texture
named as depth (`format::is_depth`), a depth buffer of a size other than the
target's. The right to draw into a view is asked of the texture behind it,
for both attachments, and in `create_bind_group` the right to sample is asked
of the texture too: a view is a reference made by whoever draws with it, and
a lease on the view alone would outlive a `transfer` of the texture (see
[resources](resources.md)). Viewport and scissor are clamped at execution to
the size of the target as it was at submit. One blend state serves every
pipeline (`ALPHA_BLENDING`); the protocol has neither stencil nor depth bias,
because nothing asks for them, and they can only be added with the thing that
uses them.

## Delegating a place

Giving a renderer a place to draw is one ritual with one message,
`core.SurfaceDelegated` (`veldcore/interface/core.proto`): the owner of the
window gives the window surface to `ui-service` with it, and the author of
the markup gives the globe its place under a `Viewport` with the same message;
the preview canvas gets it wrapped in `Canvas`
(`veldmodules/image-view/types.proto`) together with the name of the view,
because there are many canvases and the message itself carries no name.
`veldsdk::surface::delegate` (`veldcore/sdk/rust/src/surface.rs`) allocates
the texture, grants writing to the named renderer and reading to the named
readers, publishes, and only then frees the previous texture, so the renderer
learns of the new one before the old one is gone. On any refusal it returns
the previous one: the old surface keeps working, and nothing leaks.
`Delegated` is owning: the place lives as long as its holder, and a closed tab
frees its place by doing nothing. `Delegated::covers` is asked before
allocating: the place in the markup is recomputed far more often than it
changes, and a reallocation changes the texture id, which is how the renderer
decides the target changed and rebuilds what hangs on it.

The ritual has an end, `veldsdk::surface::revoke`: the same message with an
empty surface, published before the texture is freed. Freeing alone is not
enough — the view the renderer made over the texture keeps it alive past the
registry, and the renderer would draw into a place nobody sees until the
process ends. On a revoke `ui-service` drops its cached view and the bind
groups of shown images, which are what hold GPU memory, and keeps the layout
and the widget cache: those belong to the client, not to the place, and a
client that comes back with a surface continues from the same state.

The format of the texture travels in the same message: it is a property of
this texture, and a second source of it would diverge exactly when the places
to render became more than one. For the same reason the platform injects
nothing into a module's init config — `ui-service`'s `Config` is empty. The
window surface has the swapchain's format, an sRGB one; a `Viewport`'s
texture has `SURFACE_FORMAT` (`handlers/mod.rs` in `data-browser`), UNORM
with sRGB numbers, because the markup samples it as an ordinary image and
linearises it itself — encoded a second time by the GPU, a tab would come out
darker than its neighbour.

## Input

The runner publishes input as `app/on_ui_event`, addressed in the data by
`plugin_id`: a click with the button numbered as in `ui-service`'s
`pointer.rs`; a key as its physical code, the text it produced with the layout
applied, and the modifier mask (`keyboard.rs`); a scroll in window units, one
notch being `WHEEL_NOTCH` — the same number `ui-service` knows as
`RAW_WHEEL_NOTCH`, held equal by `buildgen/tests/test_wire_pairs.py`; and the
cursor. Cursor moves are coalesced to one per frame: each is a separate call
of the wasm actor, and the stream a mouse produces would otherwise pile up a
backlog that delays clicks by seconds. A scenario moves the cursor at once,
not coalesced, because a `move` and a `click` landing in one frame would press
at the previous point.

`ui-service` handles the events of a client only while that client has a
delegated surface: without one nothing renders, and rendering is the only
thing that drains the pending input. Scrolling is smoothed into a velocity
(`smooth`) that decays by time, not by frame count (`SCROLL_LEFT_PER_S`), so
one notch rolls the same distance on any refresh rate; a reversal cancels
what is left on that axis. What one notch amounts to in smoothed pixels is
`wheel_notch()`, computed beside the smoothing, and it is the unit a
`Viewport` hands its owner, so no owner keeps a copy of the number.

## The frame of ui-service

`ui-service` keeps one `PluginUiState` per client (`state.rs`): the layout as
last sent, the iced widget cache, the pending input, the geometry buffers and
the delegated target. It draws on four occasions — a new layout, a newly
delegated surface, a location question, and the frame tick — and on the tick
only with a reason: pending input, a changed layout, a widget's own redraw
request (the blinking caret, taken from what `ui.update` returns), or a live
image in the last frame, whose owner redraws it without the markup changing.

A frame (`render_plugin` in `handlers.rs`) converts the layout to iced widgets
(`converter.rs`), builds the interface over the cache and feeds it the input
in batches cut at every cursor move — iced takes the cursor as a parameter of
`update`, and one position for the whole batch would judge a press where the
cursor ended up. Then, in this order: it remembers where every named
scrollable stands and puts back one that a rebuilt tree reset
(`keep_offsets`); aims the scrollables the markup asks to aim
(`aim_scrollables`); answers a pending location question (`answer_locate`);
draws. The frame is closed by `RedrawRequested`, which is what makes iced
widgets remember hover, press and focus — without it everything draws as
disabled. The geometry is compared with the previous frame's, and only a
changed one — or one with a live image, whose geometry matches while its
content moved — is written into the delegated texture (`graphics.rs`); the
pipeline is built for the surface's format and rebuilt when the format
changes. The messages iced captured go to the client right after the frame as
the targeted `ui-service/on_ui_event`, never deferred to the next `set_view`,
which the client sends in reply to these very messages.

Text is shaped by cosmic-text into a glyph atlas of `ATLAS_SIDE`
(`renderer.rs`), uploaded whole when it changes; an atlas that runs out starts
over on the next frame, and the log line for it is read on
[the diagnostics page](../operations/diagnostics.md). Colours are named in
sRGB by the markup, as in a mock-up or in CSS, and turned linear in
`Vertex::color`: the target is an sRGB format and the GPU encodes what is
written, so an sRGB number passed as it is would be encoded twice.

## Scenario runs

The runner's side of a scenario run (`capture.rs`; the language, the verdicts
and the set are on [the scenario page](../operations/ui-tests.md)): the steps
are replayed by the frame loop after the present, so a screenshot catches the
frame already drawn, and the scenario's clock stands while a step waits. The
runner cannot turn a widget's name into a rectangle: it asks with
`app/on_locate_widget`, whoever laid the markup out answers with
`app/on_widget_located` from inside a frame (`locate.rs` in `ui-service`),
and the answer lands in `PlaceQueue` (`places.rs`), taken once per frame like
a surface. The question number rides as a field of the message, not as the
envelope's correlation — the question is `app`'s output and the answer
`app`'s input, a shape the schema does not pair — and an answer to an older
question is dropped, the screen having changed since. A screenshot blits the
attached surface again into a texture of its own, because the swapchain's
textures have no `COPY_SRC`.

## Sharing the space

The layout is derived from the content, so a widget short of space does not
clip itself: it changes shape and drags its neighbours along. The rules below
keep one long string from unfolding the window. The tree of panes, the turning
of a dragged border into shares, the dropping of columns and the trimming of
names are on [the screen page](screen.md).

- **What to do with too little width, the text declares** (`Wrapping` in
  `veldmodules/ui-service/types.proto`). The default is `WRAP_WORD`, and a
  file name, a path or a key has no spaces: such text does not wrap into its
  width, it runs past it over the neighbour. Only `WRAP_WORD_OR_GLYPH` keeps
  it inside, breaking the word by letters, and the label then grows downward.
  Labels in fixed-width columns, on buttons and on tabs declare
  `single_line()` (`NO_WRAP`, in `widgets.rs` of the wrap crate): one line,
  and the box cuts the rest.
- **Chrome of constant size fixes its height**: the tab strip
  (`TAB_STRIP_HEIGHT` in `view/mod.rs`), the status bar (`BAR_HEIGHT` in
  `theme.rs`) and the bar of controls (`CONTROL_HEIGHT`) are `Length::Fixed`,
  so no child stretches them.
- **Content wider than its place scrolls; it does not shrink.** The tab strip
  is a horizontal `Scrollable`: flex would drive the tabs to zero width, and
  text of zero width takes height.
- **A scrollable can be aimed, not held.** `Scrollable.scroll_to` sets it
  once per request: the markup is rebuilt on every event, and an aim applied
  every frame would hold the list still under the wheel. The request is
  numbered, not expressed by the offset — the same row is led to twice in a
  row and the second offset equals the first; how many points a row is, the
  client counts, since the row height is its own. `ui-service` remembers the
  applied number per named scrollable (`aimed`) and counts a request done only
  when the scrollable was found; the scroll positions themselves it keeps
  across a rebuilt tree (`offsets`, the frame order above): iced matches
  widget state by place in the tree, and a pane that becomes the child of a
  row would otherwise lose its scroll.
- **A real wrap needs `Length::Fill` on both levels** — on the text and on
  the row around it: `Fill` text inside a `Shrink` row gets no real width from
  its parent and collapses to its minimum. The same holds for a button in a
  row (`view/shown.rs`) and for `theme::spacer`.
- **A child's name (`Widget.key`) is not for swapping content.** It says
  where an insertion or a removal happened in a list; a list of the same
  length is matched by position, and only `Column` honours keys.
- **A box clips its content by itself, always** (`clip(true)` in
  `converter.rs`): a box is the boundary of what is shown, not only a place in
  the layout; otherwise a tight window draws a label over its neighbour.
  Drawing over neighbours is what `Popover` is for. The cut is not an ellipsis
  — it lands mid-glyph — and the meaningful tail is set by the client: a
  monospace glyph has one width, `format::mono_fit` counts how many fit, and
  `format::ellipsize` cuts the middle (`components/format.rs` in
  `data-browser`).
- **Clipping lives in the renderer, not in the layout.** Neither a container
  nor a row opens a layer: iced narrows the children's bounds and passes them
  to whoever draws text as `clip_bounds`, so only `ui-service` can apply them
  — `renderer::clipped` sets the frame's scissor for the text that did not
  fit, in frame pixels and shifted with the current translation, or a
  scrolled list would be cut where its content stood before the scroll.
- **Padding from the edges is assigned by one party.** Set by both parent and
  child it narrows the columns twice, and whoever computed the trim by its
  width draws over the neighbour. In the table of `data-browser` the row
  button sets it — it must be pressable on the margins too — and the header
  and the group title get it from `gutters` (`components/table.rs`).
- **A pop-up is a `Popover`, not a neighbour in the markup.** A panel laid
  beside its button takes space from the neighbours and is cut by the nearest
  scrollable; `Popover` draws it as an overlay (`popover.rs`), without a place
  in the layout. Whether it is open the client decides; a click beside it
  comes back as `on_dismiss` and presses nothing underneath.
- **A place for someone else's render is a `Viewport`, not an image.**
  `WgpuImage` shows something finished: it fits the texture into its place by
  the texture's own proportions and ignores the pointer. A `Viewport`
  (`viewport.rs`) is a place the owner of the markup draws itself; its texture
  has no size of its own, the layout assigns one. Hence it fills its place
  whole, is redrawn every frame, and returns its size in pixels of its texture
  and the pointer in the same pixels. There is no feedback from texture size
  to layout: the place comes from the markup, so the owner's reply does not
  move it, and the cycle converges in one step.
- **The gesture belongs to the markup, its meaning to whoever draws.** The
  area hands out a `PointerEvent`, not commands: which button orbits and what
  the wheel does is a decision about the interface, made where the whole
  screen is seen, and what a turn becomes only the drawer knows. So
  `data-browser` turns a drag into `globe/on_camera` in fractions of the
  area's height (`handlers/globe.rs`), and `globe` knows nothing of mouse
  buttons. The units need no conversion: coordinates in pixels of the area's
  texture, scroll in notches of the wheel. A pressed button keeps the moves
  coming past the edge, and the release is reported wherever it happens.
- **A button is a box that can be pressed** — `Container.interaction`, not a
  widget of its own, so padding, size, alignment, clipping, background and
  border work the same way; `hovered`, `pressed` and `disabled` are named one
  by one, and an unnamed state falls back to rest.
- **Being carried is a property of the box too** (`Container.drag`,
  `Container.drop`). `ui-service` leads the whole drag (`drag.rs`), and the
  owner gets one event, how it ended (`DropEvent`: what was brought and which
  edge of the zone it hit) — while a tab is carried all that happens is "the
  cursor over this zone, now over that", and where the zones are only the one
  who laid them out knows.
- **The border between panes is a widget, not a line.** `Divider`
  (`divider.rs`) reports the cursor's shift in points while dragged and one
  event when released — a shift, not a new size, because the sizes were set
  by the owner of the markup, and only it can turn points into them
  (`State::divide`, on [the screen page](screen.md)).

## The cycle of data-browser

`data-browser` is an Elm loop over the bus. After every message the generated
runner calls `hook_event` (`module.rs`), which builds the whole view from the
state (`view::build_root`) and sends it as `ui-service/on_set_view` through
`render::render` of the wrap crate; the topic is a snapshot, so an unchanged
layout does not travel — the stub keeps the fingerprint of the last body
(`veldsdk::snapshot`, see [the bus page](bus-and-schema.md)). Widget events
come back as one topic, `ui-service/on_ui_event`, targeted at the sender of
the markup: `method`, `key` and a payload, which `Msg::decode` turns back into
a message — the one place where the strings become one. Messages the person
did not send — a divider's shift, a pointer or a size from a `Viewport` — are
`idle`: they neither put out the notice in the status bar nor write the layout
file; the release of a divider does write it, being where the drag ends
(`settled`).

The root is the tree of panes: a leaf is a pane with its tab strip and body,
a split is a `row!` or a `column!` of two containers with
`Length::FillPortion` in thousandths of the share and a `Divider` between them
(`portions` in `view/mod.rs`), and under them the status bar. The globe tab
and the preview tab put a `Viewport` in their body and delegate a texture to
`globe` or to `image-view` when the area reports a new size
(`handlers/globe.rs`, `handlers/preview.rs`; the reader is `ui-service`, the
format `SURFACE_FORMAT`, the scale the window's). A notice that would not fit
the single-line status bar is trimmed by `format::ellipsize`; the full text
is in the log.
