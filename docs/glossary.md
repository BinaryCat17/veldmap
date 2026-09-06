# Glossary

One English name per term. The Russian word is the one used in code comments
and in the logs. Where one word carries two senses in the tree, both are given.

| English | Russian | Meaning |
|---|---|---|
| bus | шина | the host's event bus: a module publishes to a topic, and it is delivered to every subscriber's handler, or to the one addressee of a targeted topic |
| topic | топик | a named event; declared only in a module's `schema.yaml`, never as a string in code |
| exchange | обмен | a request together with the terminal reply it must end with |
| terminal reply | терминальный ответ | the one reply that closes an exchange; a requester gets exactly one |
| requester | заказчик | the module that published a request and waits for its terminal reply |
| host-settled reply | договорённый хостом ответ | a terminal reply the host publishes for an executor that died or was killed; its payload is empty |
| sentence | приговор | in the host: the delivery marked to be struck, so that a running handler is killed; in the consumer of tiles: a cell of a failed pass is not asked for again at this level until a later pass over the same source succeeds, and a second failure in a row is final |
| trap | трап | a wasm fault; the host rebuilds the instance, and everything in its memory is gone |
| resource | ресурс | a host-owned object addressed by id: bytes to read, a buffer, a texture (`ResourceHandle`) |
| lease | аренда | a resource's owner plus its lists of readers and writers; the host checks them on every call; ownership is transferred, grants are not revoked |
| carrier | носитель | what stands behind a byte resource: a file or a remote object behind `RangeSource`, or bytes held in host memory; on the globe, also the tile a cell is drawn with — its own or a piece of the nearest ancestor |
| reader window | окно читателя | the slice `ResourceReader` fetches per host call (`WINDOW`) |
| pool block | блок пула | the smallest unit the network service fetches and keeps in its pool (`BLOCK`): the size of a probe; a read takes the blocks it needs in one request, and a pass grows past that by readahead |
| fingerprint | отпечаток | the identity of raster bytes for the tile cache: length, head, tail, plus tile side and decode revision |
| pyramid, level, tile | пирамида, уровень, тайл | level 0 is native resolution and each level halves it; a tile is `TILE` pixels square |
| overview | копия | a reduced copy of the raster stored inside the same TIFF |
| chunk | чанк | the smallest readable unit of a raster file: a TIFF tile or strip, a JPEG 2000 codestream tile, an HDF5 chunk; the tiler's grid chunk of a NetCDF variable is a row window |
| variable | величина | one measured quantity of a NetCDF file: a dataset whose plane of samples the tiler can show; a file holds many, the tiler shows one, chosen by CF rules and named to the viewer (`Described.variable`), and lists the others it could show (`Described.variables`); the preview tab may name one of them back (`DescribeRequest.variable`) |
| row window | окно строк | the grid chunk of a NetCDF variable: `rows` rows across the full width, read by the HDF5 reader as a region along the plane's row axis — a bundle of the file's chunks up to a tile, or the whole plane when the file's chunk spans the height |
| tile-part, excerpt | тайл-парт, выдержка | a tile-part is the unit a JPEG 2000 codestream is read in — a tile's data, or a slice of it, behind its own SOT header; an excerpt is the stream the tiler hands the decoder for one tile: the main header, that tile's tile-parts cut at the wanted resolution, and EOC |
| pass | проход | one sequential read of a source that builds levels through the cascade |
| pointwise reading | точечное чтение | reading only the chunks under the requested tiles |
| cascade | каскад | the tiler's downscaling chain that turns rows of its base level — level 0, or the level a decoder produced — into tiles of every coarser level |
| ladder, step | лестница, ступень | the consumer's order of levels from coarse to fine, a step being one level asked for; in the tiler, also the halving of sizes from level to level |
| detail limit | предел детали | the finest level a source will ever serve — the first row of the level table that fits (`Meta::finest` on the consumer's side) |
| level table | таблица уровней | one row per pyramid level: how it is served (pointwise or by a pass from some level), what a step costs, the memory peak, whether it fits; it travels in the description as `Described.levels`, and the consumer derives the ladder, the pointwise levels and the detail limit from it |
| peak | пик | the memory a piece of work needs at its highest, as named terms summed against the instance's free memory |
| appetite cap | потолок аппетита | the most cells a consumer orders at once: its share of the video memory budget, never above the cache's `MAX_QUERY_TILES` |
| binding | привязка | how raster pixels map to the Earth; four kinds, from the catalogue's footprint up to a lattice of tie points |
| seating, footing | посадка, отступ | seating: how a coordinate lattice relates to the raster it binds — the sample offset and step; footing: the retreat of the lattice from a ragged edge of the coordinates so that it stays rectangular |
| lift | вынос | the one height above the ellipsoid at which everything lying on the surface is drawn (`SURFACE_LIFT_M`) |
| warp mesh | варп-сетка | the mesh a raster cell is drawn with on the globe: quads whose corners go through the binding, dense enough that their sag stays under the lift |
| ribbon, hatching | лента, штриховка | on the globe: the band of screen width drawn around the selected outline; the fan of strokes filling a scene's place while its raster is on the way |
| seam, run | шов, пробег | of a band outline: where the ring's longitude sweep crosses zero, and the stretch between two such crossings — a band has no sides, and its outline is drawn as the loops the runs close |
| ball, probe | шар, проба | the visibility test of a cell: a bounding ball against the frame's planes and the horizon rejects cheaply, nine probe points through the binding decide the rest |
| scene, product, part | снимок, продукт, часть | what a person sees as one acquisition; what the catalogue lists; the products folded into one scene |
| quicklook | квиклук | the small preview raster of a scene |
| pane | панель | a place on the screen that holds tabs; the screen is a tree of panes |
| sidecar | сидкар | the `.origin` file the library writes next to a downloaded file: where it came from, which scene it belongs to, how many files the scene has |
