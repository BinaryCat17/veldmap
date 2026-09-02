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
| pool block | блок пула | the smallest unit the network service fetches and keeps in its pool (`BLOCK`); a request grows past it by readahead |
| fingerprint | отпечаток | the identity of raster bytes for the tile cache: length, head, tail, plus tile side and decode revision |
| pyramid, level, tile | пирамида, уровень, тайл | level 0 is native resolution and each level halves it; a tile is `TILE` pixels square |
| overview | копия | a reduced copy of the raster stored inside the same TIFF |
| chunk | чанк | the smallest readable unit of a raster file: a TIFF tile or strip, an HDF5 chunk |
| pass | проход | one sequential read of a source that builds levels through the cascade |
| pointwise reading | точечное чтение | reading only the chunks under the requested tiles |
| cascade | каскад | the tiler's downscaling chain that turns rows of its base level — level 0, or the level a decoder produced — into tiles of every coarser level |
| ladder, step | лестница, ступень | the consumer's order of levels from coarse to fine, a step being one level asked for; in the tiler, also the halving of sizes from level to level |
| detail limit | предел детали | `finest`: the finest level a source will ever serve |
| binding | привязка | how raster pixels map to the Earth; four kinds, from the catalogue's footprint up to a lattice of tie points |
| scene, product, part | снимок, продукт, часть | what a person sees as one acquisition; what the catalogue lists; the products folded into one scene |
| quicklook | квиклук | the small preview raster of a scene |
| pane | панель | a place on the screen that holds tabs; the screen is a tree of panes |
| sidecar | сидкар | the `.origin` file the library writes next to a downloaded file: where it came from, which scene it belongs to, how many files the scene has |
