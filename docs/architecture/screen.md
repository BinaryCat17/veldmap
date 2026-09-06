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
`Scrollable.scroll_to` in the layout, once per numbered request (`ScrollTo.request`; the offset alone does not repeat). Otherwise the
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

## The preview tab

A preview tab is a frame the canvas (`image-view`) draws into a texture the
screen delegates to it, a toolbar with the name and the scale, and a bar under
the frame (`view::preview`). What the canvas shows it reports back
(`ViewState`), and the bar only names it: the variable first, for a file of
many (`ViewState.variable`, in the file's words and units), then the source
size, the scale, and the library's size and date for a downloaded file. The
bar is one line and does not wrap, so the variable gets what the other facts
and the room kept for the progress phrase leave of the pane's width: the
file's words only when they fit whole, else the name and units alone. The
variable is a button: under it lies the list of every variable the file could
show (`ViewState.variables`, in the tiler's order of preference), the shown
one ticked and always among the listed, a long list cut to `variables::LISTED`
and a count. An item names its variable to the canvas (`VariableRequest`),
which describes the same resource anew with it; the choice is the tab's own
(`PreviewState.variable`), saved with the tab once the canvas shows it and
reopened with it. The list is kept from the last report that carried one, so
a variable the tiler refuses — empty in its sample — leaves the refusal in
place of the frame and the list under it to pick another, no item ticked, and
is not saved: a reopened tab would meet the same refusal with no list yet. The
layer row offers the same choice for a layer on the globe (see below).

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

## A row's three states

**Selection, outline and show are three independent states of a row**
(`handlers::outline`, `handlers::overlay`). The checkbox is selection — the set
a batch action works on. The outline icon says whether the scene is outlined on
the globe; the globe icon whether it lies there as a raster. Folded into one,
each would answer for the other: removing an outline would take the scene out
of the set to be deleted, and showing it would put it into the next batch
action nobody asked for. So showing does not tick the checkbox, "Clear the
globe" and removing one outline leave a selection alone (`outline::clear`,
`outline::drop_one`), and ticking draws nothing.

Outline and show are toggles with one mechanism: pressed once, the scene is on
the globe; pressed again, it is off (`outline::toggle_outline`,
`overlay::on_toggle_pressed`). Neither moves the camera. **Focus is a third
intent and a third icon** (`outline::focus`, sent as the one `Focus` command by
`handlers::globe::focus_on`): "show it here" and "take me there" in one press
would take each other's answer — a second scene could not be laid next to the
first without flying to it. Focus takes the frame it already has — the drawn
outline, or the frame computed when the layer was shown — and never asks the
catalogue for it; it also picks the scene, because one goes there to be told
where it is.

Outlines are one set per application, not per tab (`State::outlines`): the
globe is one, lists are many, and "the outlines of this tab" does not answer
"what is on the globe". The set is a **request, not what is drawn**: geometry
belongs to the catalogue, the way to it is the network, and seconds pass
between the press and the ring. So the icon lights at once and the tooltip
says what stands between (`OnOutline` in `components::row`): asking the
catalogue; no geometry for this scene; could not ask — the same press then asks
again instead of cancelling; drawn. What is drawn lives apart in
`State::outlined`, rebuilt from the requests by `outline::refresh`: geometry
is taken from a search result that holds the product, otherwise asked of the
provider (`on_locate`), and every answer is cached in `State::located` so one
key never travels twice. Nothing is outlined without a request — a page of
results is a grid over the Earth behind which no scene is seen. Selection, by
contrast, is per list (`selected` in `state::listing`, see
[limitations](../limitations.md)).

A scene row carries at most three quick icons, all about the globe: outline,
raster, focus (`table::quick`). Everything else — download, pause, open,
preview — is a menu item: the row is narrow, a pane is half a window, and a
fourth icon would take from the name; these three earn their place by saying
their state in colour, which a menu item cannot. A disabled icon keeps its
place, drawn in the line colour rather than the ink (`IconTone::Idle`): focus
on a scene that is not on the globe, the globe on a scene the provider says
cannot be viewed — hidden, it would let the neighbours slide under the cursor.
A row that is not a scene — a file inside one — gets "show the scene on the
globe" as a menu item acting on the scene's key; a path folder gets nothing,
since the catalogue answers its name with "no such product".

## Row tints

**Four tints, precedence in the role** (`theme::RowTint`): plain; dim — a
hidden layer; marked — ticked for a batch action; picked — the one highlight
of the screen. The renderer picks the whole state of a box and does not mix it
with rest (`Interaction` in `veldmodules/ui-service/types.proto`), so "marked
and under the cursor" is not expressible in the protocol; whoever names the
colour folds them, and names the precedence there too: the highlight is one
per screen and outranks marks, of which a list holds fifty. Marked is weaker
than picked — an equal fill would erase the difference between "the main
thing now" and "what I gathered". **Hover is never lighter than rest**: each
tint carries its own hover pair (`row_faces`), because the common `ROW_HOVER`
is lighter than the accent fills and a picked row would lose its highlight
under the cursor. The table (`table::tint`) and the layer list (`view::shown`)
read the same role.

## Batch actions

Batch buttons stand in the list heading as icons and **appear only when there
is something to do** (`handlers::library::batch`, drawn by `list_screen`): a
row can be selected that is not on disk, and one that is downloaded whole, and
a visible button with nothing to do promises an action and does not perform
it. Whether there is something to do is answered by the same analysis that
later performs it (`fetch_of`, `deletions_of`) — a second answer to "what will
be done" would drift from the first.

Both unfold a scene into files: the library keeps files and knows no scenes,
so deletion takes its entries (`files_of`) and download first asks the
provider what the scene consists of (`on_download_snapshot`). A file is judged
exactly — complete and running are skipped, interrupted is resumed; a scene
is skipped only when walked and complete, because what it lacks is known to
the provider, not to us. **Batch "download" is not the row's item**: on a row
the download is a toggle whose second press pauses (`on_download_pressed`);
the batch action is named by one word and does one thing.

**Selection leaves only what deletion actually took, and only files**
(`on_delete_selected`): in "Downloaded" the row behind a deleted file is gone,
and a selection left on it would be counted in the heading to the end of the
session. A scene stays selected — its files are gone from disk, the scene
lives in the catalogue. "Clear selection" clears the whole selection of its
list, not the shown page (`outline::unmark_all`): a selection survives a
change of folder, and the heading counts it.

## Progress in the list

**The progress of a show is seen in the list, not on the globe.** There a
scene is either drawn or still absent, the wait runs to tens of seconds, and
"pressed, nothing happened" looks broken. The globe reports
`on_overlay_progress` — the whole set, a row per overlay; the topic is a
snapshot (`veldmodules/globe/schema.yaml`), so an unchanged set does not reach
the bus. The browser stores the figures without interpreting them (`Progress`
in `state::overlay`); an overlay missing from a report means "the globe does
not know it yet", not "done", and its figures stay.

It is shown in one place: the ЗАГРУЗКА column of the scene's row — a caption
and a bar under it (`table::progress_cell`). The scene's row in whichever
list it stands, and not the file rows under it (`load_of`). The column is one
for two kinds of work, because the question is one — "is something going on,
and how much is left" — and a download to disk outranks a network show in it:
the disk changes what lies on the machine, a show is derived and repeatable.
**Numbers belong to this column**: while it is shown, the neighbouring
СОСТОЯНИЕ column answers with a word — "downloading", "interrupted" — two
cells with the same fraction do not say twice as much; when the column is
gone, the numbers return there (`status_look`).

The caption is built in parts, senior to junior (`Progress::parts`): the
pyramid step, bytes read, tiles of the step. The step names what the bar
measures, bytes are the only thing moving during a long sequential read, tiles
are what moves within a step. Squeezed, the caption drops parts from the tail
(`fit_label`); the whole phrase is in the tooltip, and the layer line in "On
view" says the same phrase from the same parts — two places cannot disagree
about one piece of work.

**The strip under the row is a fallback, not a second voice** (`strip_shown`):
it is drawn exactly where the column is not. The column gives way after date
and size and before state and format (`DROP_ORDER`); in a pane of half the
window — the "list plus globe" layout in which one watches a show — it is
gone (asserted by `the_loading_column_outlives_the_reference_ones`), and
without the strip nothing about the network load would be visible without
hovering. While the raster is being described there is no share to draw, and
the strip is filled whole at half strength (`Pace::Unknown`) rather than left
empty: an empty track says nothing, and it is exactly how every show begins.
**Its height is taken, not added**: the row pitch is computed in one place
(`theme::ROW_PITCH`) and the list is scrolled to a row by it, so the strip is
cut out of `ROW_HEIGHT`. The place is taken while the scene is on the globe,
not while work is going on — otherwise the row would jump by `ONTO_GLOBE` at
the very moment the work finished, on every layer.

## The globe icon

**The globe icon is lit when the scene is on the globe** (`table::quick`):
lit — lies as a raster and is seen; half — lies but cannot be seen now, on
its way or hidden; rest — not there. What the scene is to the globe is
computed in one place for all three lists, `components::rows::onto_globe`, by
the scene's key and not the row's: the globe is one, lists are many, and
three expressions by place would drift silently. The same place answers the
strip under the row, the layer line and the hatching of the place of a scene
on its way (`outline::under_way`): the question of all four is "what is
happening to this scene on the globe now". Two more faces: warning — the last
show failed, the reason is in the tooltip, and the icon is still pressable,
since the failure may have been the network (`State::unshowable`); disabled —
the provider says the product cannot be viewed (`Row::unviewable`).

**The label names what the press does, not what is**: on a lying scene it is
"remove from the globe", not a second show; on one still asked of the
catalogue, "cancel". The icon stands in the row of every scene — search,
catalogue, downloaded. A found product carries its footprint; a catalogue or
downloaded row has only a key, and the product is restored by the provider
(`on_locate`) — one request answers both the show and the outline. The answer
arrives seconds later and is first asked whether it is still wanted
(`State::showing`): "Clear the globe" is pressed in that time, and laying the
scene after it would put back what was taken off.

## "On view" and the strip under the globe

**"On view" shows what covers the globe** (`view::shown`): every layer, top of
the screen being top of the globe, with opacity, hide, focus, remove, and in
the menu reorder and the two transitions to the scene — preview and
catalogue. **Shown is everything on the globe, not the rasters alone**: a
scene can be there as an outline only, and then this is the one place to
remove it from without walking back to the list. Layers and outlines are two
groups — two independent states, asked different things — and a scene lying
as a raster is not listed twice. A hidden layer stays in the set with its
resources and is not drawn; the header's one button names the action it does —
hide all, or show all. Where a layer stands is the layer line: assembling,
hidden, the progress phrase, or — once there is nothing left to wait for — the
name of the file lying as the detailed raster, followed by the words about it
(its refusal, its detail limit), then the rest (the preview's limit, a binding
refusal); the name and the variable it shows (`OverlayProgress.detailed_variable`,
a NetCDF file holds many) when there is nothing to say about it. The globe
draws that line between the two (`OverlayProgress.detailed_trouble`); the row
cuts the name and the words separately, and the tooltip carries the same
parts on one line of `TOOLTIP_CHARS`, shared so that the short ones stay whole
(`format::share`): the variable is there whatever the row shows, its name and
units before the file's words, which are cut from the tail. For a file of
many variables the caption is a button: under it the same list as under the
canvas (`components::variables`, `OverlayProgress.detailed_variables`), and an
item names its variable to the globe (`OverlayRaster.variable`, the set sent
again). The choice is pinned to the file by its ordinal and dropped when a
spare takes the file's place; a refused variable leaves its complaint in the
row and the list under it. The file itself is chosen from the list: a file
row inside a product — in the catalogue and among downloads alike — carries
"put on the globe with this file" in its menu (`Msg::GlobeFile`), and the
layer's rasters are asked again with it (`ImageryRequest.wanted`,
`OverlayState.wanted_file`) — for a product not yet on the globe the wish
waits for the catalogue's answer (`State::file_wishes`), for a layer still
assembling it waits for the assembly's end; a cancelled show, a refused
catalogue answer or a removed layer take the wish with them. A refused file
(not a raster by name, the quicklook) comes back as a notice, and the layer
lies with the layout's choice.

The strip under the globe tab goes the other way (`view::globe`): it names
the scene — the picked one, else the top visible layer — leads to it by the
same two transitions, and **"Clear the globe" takes everything at once**,
rasters and outlines together (`overlay::clear_all`, `outline::clear`): a
person sees one globe and has nothing to split them by. The named scene
displaces the size of the area and the hint about the controls: the name
takes what is left of the strip, and the layout gives the buttons their room
first — in a half-window pane it is exactly they that would slide off the
edge.

**An overlay from a search result lives the life of the result**
(`OverlayState::source`): it leaves with the product when the result set
changes (`overlay::keep_only`) and with the closing of the tab
(`overlay::source_closed`), and its leaving is said aloud in the notice, since
no one pressed anything. What was shown by key — from the catalogue or from
downloads — depends on no result set and is removed only by hand. Outlines
are tied to no tab: the product a search held is cached at the first draw
(`State::located`), so an outline outlives its result set.
