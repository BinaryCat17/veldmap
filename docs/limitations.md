# Limitations

What the tree does not do, declared in one place. Each entry names the
limitation, its cause as the code stands, and what removing it needs — the
thing that must appear, not a promise of when. A restriction another page
explains in full gets one line here and a link. Terms are in
[the glossary](glossary.md).

## Catalogue and search

- **A search cannot be limited to an area from the screen.** The contract
  takes one (`SearchRequest.area`, turned by `data-provider` into
  `OData.CSC.Intersects` in `veldmodules/data-provider/src/catalogue.rs`), the
  globe answers a screen point with a place (`on_probe`), and the right mouse
  button over the globe is left free for exactly this (`on_pointer` in
  `handlers::globe` of `data-browser` rotates with the left button only). What is
  missing is the frame drawn on the globe. Without it a search carries the
  name part, the mission, the time window and the cloud ceiling, and the
  catalogue answers with the newest products anywhere on Earth (the query
  orders by `ContentDate/Start desc`); where they are is seen only by
  outlining them, one row at a time — there is no "outline all". Removing it
  needs a frame gesture on the globe that fills `SearchRequest.area`.
- **Which part of a scene is shown is a rule, not a choice.** The parts of
  one acquisition can be different measured quantities — one product per gas
  — and the catalogue says nothing about their precedence, so `scene::rank`
  (`veldmodules/data-provider/src/scene.rs`) shows the cheapest. Pressing a
  part shows that part (`scene::about`); the choice exists only through
  unfolding the scene.
- **A folder has no size and no date.** In S3 a folder is a common prefix of
  keys (`on_list_path` in `veldmodules/data-provider/src/cdse.rs`), and
  nothing stands behind it; the columns stay empty, and the only thing said
  about the contents is what the library knows — how many files of the folder
  are on disk. Filling them would cost a listing of the whole subtree.

## Placing a raster on the Earth

The kinds of binding, their rank and who interprets a coordinate system are
in [georeference](architecture/georeference.md); these are the edges.

- **A raster without a binding of its own lies on the catalogue footprint,
  and that is a guess.** The footprint says which piece of the Earth was
  imaged, not which pixel goes where, and its vertex order is the catalogue's.
  A ring of exactly four vertices is taken as the corners (`quad_of` in
  `veldmodules/data-browser/src/handlers/overlay.rs`); a strip of dozens of
  vertices, a scene the antimeridian cuts into two rings, or a rectangle that
  spans the whole globe is not a quad — the globe holds the place
  (`Frame::Rough`) and draws no raster until the raster brings a binding. A
  product none of whose rasters carries a usable binding can lie turned: its
  shape is right, its orientation is unknowable. A raster that names a system
  the globe cannot translate is refused by name (`Frame::from_placement` in
  `veldmodules/globe/src/overlay.rs`), and the layer falls back to the
  footprint with the reason beside it. Removing it needs a code known to
  `System::from_epsg`, or a binding named at the input.
- **The camera aims at the catalogue footprint, not at the raster's
  binding.** `focus_on` (`veldmodules/data-browser/src/handlers/globe.rs`)
  flies to the circle inscribed in the footprint (`footprint::Frame`); "the
  raster's binding wins" decides where the raster lies, not where the camera
  goes. Where the file disagrees with the footprint — the case the binding is
  read for — the raster lies by the file and the camera arrives at the
  footprint. Removing it needs a focus computed from the overlay's binding
  once the raster is described.
- **A system not named in the file cannot be told to it.** The binding is
  what the raster carries: an EPSG code and a transform. Data in a local
  system — cadastral parcels are kept in them; for Rostov oblast that is
  MSK-61 — usually name the system in words in a document and keep bare
  numbers in the file. The parameters of MSK-61 exist (`System::from_epsg` in
  `veldmodules/globe/src/projection.rs`, held by a test), but a GeoTIFF
  geokey cannot carry its seven-digit code, and there is no place to name a
  system for a file from outside. Removing it needs a named binding at the
  input.
- **Geographic coordinates on a foreign datum are placed as WGS84.** A raster
  whose tie points are degrees on a datum other than WGS84
  (`GeographicTypeGeoKey`; Pulkovo 1942 is EPSG:4284) lands about a hundred
  metres off in the middle latitudes: the ties and the frame leaving the
  tiler are declared in WGS84, and nothing downstream has a field for a
  datum. The tiler names the datum aloud in the log (`foreign_datum` in
  `veldmodules/image-tiler/src/adapters/tiff.rs`) and leaves the numbers as
  in the file; silence and `user-defined` are not a foreign datum. Projected
  systems the globe knows are not concerned: Gauss-Krüger zones (284xx) stand
  on Pulkovo 1942, and their datum shift is computed in `projection.rs`.
  Removing it needs a datum on `ties` and `placement` for the globe's `Datum`
  to apply.

## Reading a raster

What each format reads per level is the table in
[imagery](architecture/imagery.md).

- **A NetCDF variable is judged by a sample, and some variables cost the
  plane.** `describe` reads up to `SAMPLE_WINDOWS` row windows of a
  candidate (`veldmodules/image-tiler/src/adapters/netcdf.rs`) to tell an
  empty or flat variable from a measured one and to set the stretch; a
  variable measured only outside those windows is skipped as empty. The row
  window is cut along the plane's row axis wherever the file keeps it
  (`rows_of`), so a unit leading axis (Sentinel-5P, `[1, scanline,
  ground_pixel]`) does not widen it; a variable written as one chunk
  (SYNERGY) does make the window the whole plane, since a chunk decodes
  whole, so the level table says a tile of level 0 costs the plane, and
  describe and every tile read it whole. Any variable of no
  more than `SAMPLE_WINDOWS` windows (OLCI GIFAPAR: three chunks of 5026
  rows) is likewise read whole by the sample and again by the first tiles —
  one window at a time, so it costs the memory of a window, not of the plane.
  A variable no taller than a tile (`TILE` rows) is one window whatever its
  layout, as the Sentinel-5P NRTI slices in the catalogue are.
- **A JPEG 2000 over the network pays a pool block per tile-part.** The
  excerpt ([ADR 0004](decisions/0004-jp2-excerpt.md)) asks one probe of
  `PROBE` bytes per tile-part (`veldmodules/image-tiler/src/adapters/excerpt.rs`),
  and the network delivers a whole block of `BLOCK` for each
  ([ADR 0005](decisions/0005-network-reading.md)): the coarsest level of a
  Sentinel-2 granule needs about a megabyte and receives one block per
  tile-part — 68 MiB of 129 for 121 tile-parts, measured in the ADR. A tile
  of several tile-parts pays a probe per tile-part. Removing it needs a block
  sized by the read, which the ADR leaves untouched.
- **A JPEG 2000 whose canvas or tile grid does not start at zero is refused
  at describe** (`veldmodules/image-tiler/src/adapters/jp2.rs`): the
  decoder's reduced grid is up to a pixel off the tiler's halving ladder.
  Zero is transparent only in a file with a GMLJP2 binding, and a GML grid
  the tiler cannot use is silence — the raster falls back to the footprint
  without a reason.
- **Remote viewing needs range requests.** A server that answers a range with
  the whole file (HTTP 200 in place of 206) is refused at opening (`probed` in
  `range.rs`). Sequential formats — PNG, JPEG, GIF, BMP, WebP — are read by a
  pass over the whole file, so the first remote show of one costs its
  download in traffic; afterwards the tiles come from the cache.
- **A signature is issued once.** The authorisation of a remote object is
  issued in the request headers when it is opened, and the storage accepts
  it for a quarter of an hour after `x-amz-date`; a 401 or 403 in the middle
  of a read is a definite refusal, not retried (`ranged` in `range.rs`), and
  the object is not signed again. A layer that outlives its signature loses
  its source and has to be opened anew. A presigned address is refused by the
  storage ([ADR 0005](decisions/0005-network-reading.md)), so removing this
  needs re-signing on 401/403 — an exchange between `network` and
  `data-provider`.
- **Sizes.** `MAX_SOURCE_SIDE` bounds every raster at describe, and
  `FULL_DECODE_BUDGET` bounds what a whole-frame decoder may produce — JPEG
  at the finest scale that fits, interlaced PNG, GIF, BMP, WebP
  (`veldmodules/image-tiler/src/adapters/mod.rs`); the detail limit is where
  the level table stops fitting.

## The screen

The rules of panes, tabs and highlight are in
[screen](architecture/screen.md).

- **There is nothing under the rasters.** The globe has no base layer — no
  mosaic, no terrain — so away from the overlays a close view is the blank
  sphere with the graticule every `GRID_STEP_DEG` degrees
  (`veldmodules/globe/src/mesh.rs`). Removing it needs a base layer.
- **One globe.** The drawing module is one — one camera, one render target,
  one set of overlays — so the globe tab is a singleton and a second cannot
  be opened.
- **Tabs are not reordered inside their pane.** A drop on a strip appends the
  tab (`State::move_to` in `veldmodules/data-browser/src/state/mod.rs` puts
  it after the pane's last), and a drop into its own pane only activates it:
  tab order is a list, and inserting between neighbours needs drop zones
  between tabs that the markup does not have.
- **Selection is per list.** Each tab keeps its own checkboxes (`selected` in
  `veldmodules/data-browser/src/state/listing.rs`): the same scene checked in
  a search is unchecked in the catalogue, and a batch action takes the
  selection of its own list, not everything checked in the window. This is
  what keeps a selection alive across a new result set, takes it away with
  its tab and counts it in the tab's header. Outline and show are properties
  of the application and read the same in every list.
- **The ribbon lives on the outline.** A scene lying as a raster stays
  selected and is named in the strip under the globe, but once its outline is
  removed by hand nothing draws the ribbon around it (`forget_gone` in
  `veldmodules/data-browser/src/handlers/outline.rs`); outlining it again
  from any list brings the ribbon back.
- **The cursor does not change over a pane border.** `ui-service` computes
  iced's `mouse_interaction`, but the runner owns the window and nothing
  carries the interaction to it; that a border can be grabbed is seen only by
  its highlight under the cursor. Removing it needs a cursor topic from
  `ui-service` to the runner.
- **No clipboard.** `ui-service` runs iced with `clipboard::Null`
  (`veldmodules/ui-service/src/handlers.rs`): a wasm module cannot reach the
  system clipboard, so a row's menu cannot copy a path; it can reveal the
  file in the file manager (`on_reveal` in `data-library`). Removing it needs
  a clipboard on the runner's side, reached over the bus.
- **Time is shown in UTC.** `wasm32-wasip1` has no time zone — neither an
  offset nor a rule base — and a module has nowhere to take one from
  (`veldmodules/data-browser/src/components/format.rs`).
- **A scenario names only what `ui-service` draws**; the globe, the canvas
  and the dividers are addressed by pixels — [scenario runs](operations/ui-tests.md).

## The platform

- **One window.** The desktop runner takes exactly one declared window: none
  or several declared is an error at start
  (`veldcore/platform/host/runners/desktop/src/main.rs`). How a window is
  declared is in [configuration](operations/configuration.md).
- **Vulkan only.** The runner builds its wgpu instance over
  `Backends::VULKAN`, with non-conformant adapters allowed (Dozen — Vulkan
  over DX12 under WSL). `setup::init_wgpu`
  (`veldcore/platform/host/core/src/setup.rs`) prefers a hardware adapter and
  skips software ones (llvmpipe, anything named "software", Microsoft's WARP);
  finding none, it asks the same Vulkan instance for a fallback adapter — a
  software Vulkan driver such as lavapipe, if one is installed, draws slowly.
  Without any Vulkan driver the host does not start; DX12 and Metal are
  never tried. Removing it needs the instance built over `Backends::PRIMARY`
  in the runner.
