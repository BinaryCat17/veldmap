# Resources

A resource is something that lives on the host side and is addressed by
`ResourceHandle { id, size }`. Identity, ownership and release are the same for
every resource; the content splits in two by whether reading at an offset
makes sense.

**Bytes** (`ResourcePayload::Data` in `veldcore/platform/host/core/src/memory.rs`)
have `read(offset, size)` and `write`:

| Carrier | What it is |
|---|---|
| `Cpu` | plain host memory |
| `Range` | a file on disk or a remote object read by HTTP range requests, one `RangeSource` for both — the reader cannot tell them apart |
| `Buffer` | a GPU buffer |

**Opaque** (`ResourcePayload::Gpu`) have no byte range behind them: a texture,
a texture view, a sampler, a shader, a bind group and its layout, a pipeline.
A texture is opaque rather than bytes because it cannot be read at an offset —
a GPU-to-CPU copy would stall the pipeline; an image is uploaded into it whole,
by a separate call (`upload_image`), not written at an offset.

## Reading

A read copies only the requested range into the caller's memory and never
hands out a pointer: that is the access boundary and the protection against
races. Reading past the end is not an error but a short, possibly empty,
answer, as with a file: readers go in windows, and the last window is almost
always short. The rule is one for all carriers and is checked in the host's
`MemoryManager::read`, not in each carrier.

On the module side `veldsdk::ResourceReader` (`veldcore/sdk/rust/src/resource.rs`)
turns that into ordinary `Read + Seek + BufRead` in windows of `WINDOW` bytes,
fit for any parser; a gigabyte file is never raised into memory whole. A
window starts where the read starts, not on an aligned boundary: a seek
forward inside the window costs nothing, a seek back before its start costs a
new window from the new position. Its window is the only buffer — it
implements `BufRead` itself rather than through `BufReader`, which would copy
the window into a second buffer. Two kinds of reader bypass it: NetCDF,
because HDF5 walks the file at absolute offsets, and the modules that read a
small file they wrote themselves — a tile body, a sidecar, the window layout —
through `read_whole`, which takes a ceiling named by the caller, refuses
anything larger and frees the resource on every outcome. Bytes go the other
way through `region_of`: a host region holding them, owned by the caller,
whose handle travels in a request to a platform service — a file write
through `fs/on_write` — and is freed by the reply.

For a remote object a window becomes an HTTP request, and the price of one is
twofold: the request itself and the bytes per megabyte. The network module
(`veldcore/platform/host/modules/network/src/range.rs`) keeps a pool of
blocks of `BLOCK` bytes — the size of a JPEG 2000 probe and of a fingerprint
edge, so an order buys the blocks under it and nothing beyond, and a probe
from an arbitrary offset touches two. A read takes the blocks it still needs
in one request, however long it is, so the small block costs a long window
nothing. A sequential pass grows on its own: a miss that lands exactly where
the previous request ended, after the reader has read that request's last
block to its end, doubles the fetch up to `READAHEAD`; a jump elsewhere, or a
request left unread — a chain of probes at tile-part headers — resets it to
what the read names (`Readahead`, [ADR 0005](../decisions/0005-network-reading.md)).
The readahead belongs to the object, not to the opening: a preview and a
globe layer of the same file continue one pass. A reader that knows what it
is about to read says so instead of being guessed at: `veld_resource_prefetch`
names ranges, the network fetches the missing blocks under them as runs of
adjacent blocks, no run longer than `READAHEAD` and no order larger than
`PREFETCH_CAP`, `IN_FLIGHT` requests at a time, into the same pool; the reads
that follow are hits, and the readahead is not touched. The chunk grid driver
of the tiler orders the chunks under the tiles it was asked for before it
reads them (`Chunked::prefetch`): a TIFF by the offsets and byte counts of
its catalogue, a JPEG 2000 with a TLM by the probes of its tile-parts and,
once the excerpts are assembled from those probes, by the file pieces of
every tile asked for in a second order. A single read larger than
the instance's memory is refused by the host before it allocates
(`INSTANCE_MEMORY_LIMIT`). The pool is one per process with a ceiling of
`POOL_LIMIT`, evicting the oldest block whoever owns it; not per resource,
because a scene opens as many resources as its layers, and a per-resource
ceiling would multiply invisibly; and not "your own first", because under
pressure an active reader would throw away what it is about to reread while
an idle neighbour keeps its blocks untouched. A failed range is asked again
up to `ATTEMPTS` times with a pause that doubles from `RETRY_PAUSE`: a show
lives for minutes and goes to the network hundreds of times, connections do
break, and without a retry one break would cost a whole pass of the producer
and, with it, cells the consumer no longer asks for. Only what can pass on
retry is retried — a definite refusal or a foreign path comes back the same,
and a pause before every block would stretch one message into minutes. An
expired signature is the one refusal that is answered rather than repeated:
the object is signed again through the provider and the same request goes
once more with the new headers (ADR 0005). A progress line goes to the log
every `REPORT_STEP` bytes and on close.

**Blocks belong to the object, not to the opening.** The same file is opened
several times — preview, the same raster on the globe, a second layer — and
every opening would fetch the same bytes. The pool's ownership key is
therefore the object: the address without its query, the length, and a
validator (`ETag`, or `Last-Modified` without it) read by the same probe that
learns the size. The signature of a storage object lives in the headers
(`s3::object`; the storage refuses a presigned address, [ADR 0005](../decisions/0005-network-reading.md)),
so two openings of one object share the address string as well as the key.
The key drops the query and the log prints addresses without it because a
query is not part of an object's identity, and on another server it may carry
a key or a signature; length and validator keep a re-uploaded object from
inheriting foreign blocks. A server that gives neither
gets no shared key: guessing identity by path and length would cost a swapped
middle of a file. Blocks do not outlive their last reader: the key counts
references, and closing the last opening drops them all, as any closed
resource.

Shutdown is handled by the read itself: it lives under the synchronous memory
ABI, not under a task, and nothing can cancel it from outside — at exit it may
well be in flight. It must neither go to the network again nor play a request
out: the runtime tears its timers down before it waits for the blocking
threads, and polling the request's timer after that panics inside tokio — with
`panic = "abort"`, the end of the host. So the runner announces the shutdown
before dropping the runtime (`veldmap_host_core::Shutdown`): a retry checks the
flag right before going to the network, and a request in flight waits for the
announcement alongside the server's answer and is dropped on it, unpolled
(`range::awaited`). Dropping is not enough for a thread preempted in the middle
of polling its request — it finishes that poll when it runs again — so requests
in flight are counted, and the runner waits for them to return, up to two
seconds, before dropping the runtime (`Shutdown::settled`).

## Lease

Every resource has a lease: an owner, a list of readers and a list of writers
(`veldcore/platform/host/core/src/registry.rs`). The owner may transfer
ownership (`transfer`), grant reading or writing (`grant_read`,
`grant_write`) and free the resource (`free`); the host checks the lease on
every call. A grant is not revoked: it lives as long as the resource, and
taking it from a reader would be the same as freeing the resource. The one
thing that drops grants at once is `transfer`: the resource has a new owner,
and the previous owner's lists no longer apply, so the new owner grants anew —
that is how a surface is delegated, ownership first, grants after. The right
to write includes the right to free; nothing separates them, since any
condition that temporarily closed writing would close the owner's own release.

## "Open me this"

Four services answer an open request with the same `core.ResourceOpened`
(`veldcore/interface/`): `fs` and `network` on the platform side,
`data-provider` and `data-library` by their hands. Ownership of the opened
resource passes to the requester. The shared part of the exchange lives in
`veldsdk::resource`: `requester`, `accept`, `hand_off`, `opened`, `relay`,
`discard`, `release`; so does the whole discipline of ownership — the RAII
wrapper `OwnedResource`, and `grant_read_or_free` / `grant_write_or_free`,
which free the resource when delegation is refused. The native `fs` and
`network` cannot reach the SDK; their mirror in `veldcore/platform/host/util/`
includes the same file for the layout of the answer
(`veldcore/sdk/rust/src/resource/opened.rs`, through `#[path]`), and keeps of
its own only the host's `(id, len)` form. `accept` reads a host-settled reply
as the host's refusal, not as "no handle": the rule is
`veldsdk::reply::undelivered`, one for every reply whose empty form would read
as success.

`discard` and `release` are opposites. `discard` answers a **foreign** reply
and does not touch the resource: the bus delivers a reply to every subscriber
of a shared topic, an unknown correlation is the norm, and the resource in
such a reply may already be ours by then. `release` frees **our** resource
that found no use: the tab was closed, the request was superseded.

## The host in native tests

Unit tests run natively, where there is no host; the SDK's ABI stubs then
answer as a host that has run out of everything. `veldsdk::fake`
(`veldcore/sdk/rust/src/fake.rs`) is the host for a test: `install` raises it
on the current thread, `mount` puts bytes in as a resource owned by the
module, and the journals — `reads` (one entry per window asked), `published`
(topic, payload, correlation, addressee), `logged`, `leaked` (what nobody
freed), `owner` and `readers` — show what the module did. Answers are encoded
by the same file the host uses (`veldcore/sdk/rust/src/abi/wire.rs`,
included into the host through `#[path]`), so the fake proves the SDK's
agreement with the host, not with itself; the bytes of an answer live in an
arena that stands in for the guest's linear memory, because a pointer packed
into the answer must fit the wasm word. What the fake does not share with the
host is the rules themselves — the short read past the end, the growing
`Cpu` write, the lease — those are its own copy, worded like the host's and
kept in agreement by tests, not by a file. The event context of the SDK is
thread-local natively for the same reason the fake is: tests run on threads.
