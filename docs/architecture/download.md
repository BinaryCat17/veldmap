# Downloading

How a file gets from the provider's storage to disk, what stays on disk when
it stops, and how the library tells what it has. The owner is `data-library`
(`veldmodules/data-library/src/download.rs`, `storage.rs`, `catalog.rs`); the
byte transfer is the network service
(`veldcore/platform/host/modules/network/src/download.rs`). The accounting of
the operation is [the bus page](bus-and-schema.md), killing it is
[the tasks page](tasks.md).

## The pipeline

```
data-browser  --on_download-->  data-library
data-library  --on_sign------>  data-provider   (sign the address)
data-provider --on_signed---->  data-library
data-library  --on_fs_download->  network       (the owner of the task is data-library)
network       --on_fs_download_progress-->  data-library
network       --on_fs_download_result---->  data-library
data-library  --on_state----->  data-browser
```

One operation has one name along the whole path: the library's correlation is
the id of the request to the provider, of the request to network, and the name
of the operation at the platform. The platform makes the publisher of
`network/on_fs_download` the owner — the library — so the library is who kills
it. In the download itself the provider takes part in one step, signing the
address; the storage layout does not leave the library, which substitutes the
path on disk after the signature. (The library asks the provider one more
thing, outside any download: the scene boundary for files whose sidecar has
none — see below.) A signing reply that the host settled for a dead provider
arrives empty, and empty would read as "signed": the library asks
`veldsdk::reply::undelivered` before trusting it, as every consumer of a
terminal reply does.

No more than `AT_ONCE` downloads run at once; the rest wait for a place and
start as it frees, in the order of the clicks. The ceiling is for the other
side, not for us: a batch "download" on a page of scenes is hundreds of
files from one click, and the storage answers such a thing with refusals. A
waiting entry is visible in the list alongside the running ones — with what it
already has on disk, or with zero for a re-download: from the click it is the
person's intention, and hiding it until it starts would lose the click from
view. The wait for the signature looks the same, being the same pause, only
shorter. A finished file is removed by a re-download at the moment of start,
not of the click: the queue stands between them, and a file removed early
would leave the person without the data and without the download for the
whole wait.

There are two stops, told apart by what stays on disk: `on_pause` leaves the
`.part` as the point of resumption, `on_delete` leaves nothing. Both kill the
operation from the library itself — it is the owner — and both get the same
`on_fs_download_result` for it, only empty. On the network side the file is
written as `<destination>.part` and renamed on success only; a break — an
error, a kill, which drops the future — leaves the `.part` as it is, and the
next request for the same destination sees its size and asks the server from
there with a `Range` header instead of starting over. The suffix is named on
both sides (`PART_SUFFIX` in the network service and in the library's
`storage.rs`) and held equal by `buildgen/tests/test_wire_pairs.py`.

## What lies on disk

Only the library knows the storage layout: outward go derived catalogue
entries, without paths and service suffixes. It rests on one invariant: **one
file lies under one name on disk** — either the finished one or the `.part`,
never both. Otherwise the entry would be derived twice, and which of the two
truths is shown would be decided by the order of the directory walk. Only a
re-download of a finished file can break the invariant, so it removes the file
before starting (`download::on_download`), and the fold of a scene by name
(`catalog::on_list_result`) names a winner even for a pair put there past the
application.

The name of an entry is a path, not the last segment of the key: for a file
from a scene it is the scene's name and the path inside it, and on disk it
lies just so — the scene as a folder. Otherwise the invariant would rest on
luck: `quick-look.png` and `manifest.safe` exist in every product, and a
second such file would mean not a second file but the same one — the one
downloaded second would remove the first. The library's directory is therefore
listed recursively (`FsListRequest.recursive`); directories themselves are not
entries and do not come back.

Which scene a file belongs to the library does not derive, it remembers: the
scene boundary is the provider's storage layout (`s3::product_root`), it
arrives with the download request (`DownloadRequest.product`), survives a
restart in the sidecar, and it is what puts the file into the scene's folder.
A file whose sidecar names no scene — downloaded past the application, or
before the boundary was written — is asked about, not guessed at: the library
sends such identifiers to `data_provider/on_product_roots` after a listing
and writes the answer into the sidecars (`catalog.rs`), because a second
carrier of the bucket layout would drift from the first on the next mission.
The sidecar is the `.origin` file next to the downloaded one — a short json
the library writes itself (`veldsdk::resource::region_of` into
`fs/on_write`), read back whole under `SIDECAR_CAP`: anything larger under
that name is not ours. Folding the files of one scene into one row is the
display's business: the library keeps account of files, that is its subject,
and "a scene" is what they are shown as (`downloaded_rows` in
`veldmodules/data-browser/src/components/row.rs`).

A scene is called whole only by a walk. How many of its files are finished is
seen from the entries; how many there should be is not: perhaps a few of its
files were fetched, and "on disk" for such a scene would mean "on disk is all
that was fetched". The number is obtained by whoever downloads the scene
— it walks the storage anyway to queue the files — and at the end of the walk
tells the library (`data_library/on_snapshot`), which puts it into the sidecars
of the scene's files next to `product`. From then on the display compares the
finished count with it and goes to the storage no more: the scene loses
"download whole", the row shows "N on disk" (`LibraryEntry.siblings`). A walk
that broke off names no number: what was listed is not the whole scene.

## Derived, not accumulated

The library's state is derived from three independent sources of fact: the
disk snapshot (`fs/on_list`), the sidecars (where each file came from) and the
downloads in flight. None is patched optimistically: the snapshot is reread on
every terminal event. Materialised rows would be edited from four places, and
every missed patch would lie in the interface until the next listing. The
tests of `catalog.rs` hold the fold on a handmade listing.
