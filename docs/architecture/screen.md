# The screen

How `data-browser` lays out its window: panes, tabs, what is saved, how a
message finds its tab, and the rules that keep one thing highlighted and one
menu open. The code is `veldmodules/data-browser/src/state/` for the model,
`veldmodules/data-browser/src/handlers/` for the changes, and
`veldmodules/data-browser/src/view/` with
`veldmodules/data-browser/src/components/` for the drawing. Terms are in
[the glossary](../glossary.md).

## Panes and tabs

The screen is a tree of panes (`state::layout`): a leaf holds tabs, a split
divides its space in two along an axis at a share. A pane is a place where
tabs live — its own tab strip, its own active tab, its own "plus". A tree
rather than a pair of halves, because splitting in two is not the only way to
arrange work: a list on the left, the globe top right and an image below it
are two splits nested one in the other.

The tree is folded by moving a tab, not by a "new pane" command: a menu item
on the tab, a drop on the edge of another pane or on its strip. Hence there
are no empty panes: a pane is created by the tab that moves into it, and one
that has emptied leaves the tree by itself. A place where nothing has been
chosen yet is a tab (`ViewKind::Empty`), not a pane: it has its place in the
strip and its own close button like everything else.

Pane borders are dragged. The layout reports the cursor's shift in points, and
what that becomes in shares is computed by the owner of the shares
(`State::divide`): splits nest, and the share of a nested one is not measured
against the window. No pane shrinks to nothing — `MIN_PANE` holds the limit
below which neither the tabs nor their content would be visible, and there
would be nothing left to grab to give the pane its space back.

## What is saved

The window opens as it was folded: the layout is written to a file under
`runtime/state/` (`PATH` in `handlers::persist`) on every action with tabs
and read at start. What is saved is what determines **what is shown**: the
shape of the tree, the shares, the kind of each tab and the little from which
its content is asked again — the catalogue folder, the search conditions, the
image of a preview. The content itself arrives as a reply and has no place in
the file: a folder listing is different a week later, and one shown from the
file would be yesterday's truth. The file is a cache, not a document: if it
does not parse or is missing, the window opens with the tab from the config
(`initial_view`), and only then is the config asked. The file is read whole
through `veldsdk::resource::read_whole` under a ceiling of `LAYOUT_CAP`:
anything larger under that name is not a layout.

## How a message finds its tab

One routing rule follows: **a message from the body of a tab names its tab.**
There are any number of panes, so "the active view" does not answer "whose
click is this" — the click may have been in a pane that is not in focus — and
everything born inside a tab travels wrapped as `Msg::In(ViewId, ViewMsg)`,
the tab naming itself in the `Handler.key` field. In the key, not in the
payload: for an input field, an area or a slider the payload is made by the
renderer, and there would be no room left for the addressee (see `Handler` in
`veldmodules/ui-service/types.proto`).

What is shared and what is not follows from whose property it is. The overlays
on the globe, the footprints and the catalogue of downloads are properties of
the application, and a pane does not divide them: "On view" in any pane shows
the same set of layers, and a message acting on them carries no source tab at
all — there is nowhere to address it, it is one per window.

For the same reason a tab whose content belongs to the application and not to
the pane is a singleton: an already open one is shown instead of a second.
Search and downloads, because their content does not depend on the tab; "On
view", because the layers live in the module's state; the globe, because the
drawing module is one and it accepts one render target (`on_set_surface`
carries no view name, unlike `Canvas` of `image-view`). Two kinds duplicate —
the catalogue and the empty tab: two folders side by side are ordinary, that
is what tabs are for, and the empty one is a place, not a view.

A singleton summoned from the "plus" menu moves into the pane it was summoned
from: the request was to show it here, not to send the eye there. Showing an
image on the globe is not a summons: it does not move an open globe tab, it
only turns the eye to it (`nav::on_new_globe`). The image is placed while
looking at the globe, and dragging it to the list would take off the screen
the thing the action was for.

**A transition is a show, not a summons.** "Show in catalogue" from a row,
from the strip under the globe and from "On view" lead to an already open
catalogue, and open a new tab only when there is no catalogue at all
(`nav::catalog`). It is searched from near to far — shown in this pane, any in
this pane, any in the others — and one found in another pane stays where it
is, as the globe does. Opening a tab per transition would pile them up until
the work is hidden behind them; tabs are opened by the "plus" menu, which is
where they are asked for.

The row led to is named three times: the catalogue opens its folder, the list
goes to its page and highlights the row, and the scroll is aimed at it —
`Scrollable.scroll_to` in the layout, once per change of value. Otherwise the
row would lie below the edge of the screen, and "led to" would mean "opened
the same folder".

## One highlight, one menu

**One row is highlighted per screen.** Two things highlight it — a transition
to a row and picking a footprint by a click on the globe — but they answer one
question, "what is the main thing here now", and it has no two answers. So the
highlight is one field per screen (`state::Highlight`): the scene key, the tab
whose transition led to it, and whether it is outlined on the globe. Each of
the two writes the whole field and so puts out the previous highlight; spread
over the carriers, it would be held by a reset in every handler, and both rows
would stay lit — the one led to half an hour ago and the one picked now.
Neither fades by itself: the transition mark lives until the folder is left,
the ribbon on the globe until the next click. By the same rule and for the
same reason lives the open menu (`state::Open`): only one is open, and that is
a property of the screen, not of a pane, a tab or a layer.

## Opening a file

The screen opens a file by one rule, in `handlers::open_resource`: a file that
lies on disk whole is opened by the library, everything else by the provider
over the network. Which it is says `LibraryState::local_name`, by the file and
not by the scene — a downloaded quicklook does not make the measurement raster
next to it local. Both openers answer with the same `core.ResourceOpened`,
ownership passes to the screen, and the screen hands it on to the canvas or
the globe (see [resources](resources.md)). The bookkeeping of the wait is the
caller's own — a preview tab, an overlay assembly — and so is the correlation.

## Names, page numbers, columns

**A name in a list is trimmed by what tells rows apart.** Cutting the middle
is the default, but Copernicus names agree exactly at the edges: mission and
product type at the start, processing baseline and extension at the end. Cut
in the middle, a catalogue page becomes a column of the same string with
nothing to choose. So the list computes, over the shown page, what the names
share (`format::shared`) and cuts that first; what is freed beyond the
distinguishing part goes back to the start of the name, by which it is
recognised (`format::distinct`). The rule steps back on its own: one name,
names that agree whole, names that fit whole, an edge shorter than two
characters — cut as before.

**As many page numbers as fit** (`controls::numbers`): a window around the
current page, the edges always named, gaps marked with an ellipsis. A
catalogue folder has dozens of pages, they do not fit in a row, and a box
cuts by itself — cutting the tail of the row, that is the "next" arrow. A list
with no visible way off its first page looks shorter than it is.

A table in a pane does not shrink, it drops columns: the pane is narrower than
the window, the columns do not slim for that, and the stretching name would
collapse to nothing. The order in which they give way runs from reference to
necessary: date, size, loading, state, format and last the icon
(`DROP_ORDER`); the name and the buttons never leave (`components::table::fit`).
